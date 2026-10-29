// SPDX-License-Identifier: Apache-2.0

//! Replica synchronization protocol.
//!
//! A replica copies a bucket from other providers up to a commitment the
//! chain holds, in three steps:
//! 1. Leaf phase: fetch the MMR leaves of the commitment and check them
//!    against its root (see [`crate::leaves`]).
//! 2. Chunk phase: walk the chunk tree under every missing data root top
//!    down, fetching the missing nodes in pages and checking each one against
//!    its hash.
//! 3. Install the leaves with `import_commitment`.
//!
//! A source can be a primary or another replica: primaries gate reads on
//! private buckets (a replica is not a bucket member, so an honest primary
//! refuses it — the design's "primary gate"), while replicas serve everyone,
//! so a private bucket's replicas seed further replicas.
//!
//! On-chain confirmation of what was synced is the coordinator's
//! (`coordinator::ReplicaSyncCoordinator::confirm_on_chain`).

use crate::leaves::fetch_leaves;
use crate::source::{NodePage, SourceClient, SourceSet};
use crate::Error;
use provider_storage::{ChunkTreeNode, StorageBackend};
use sp_core::H256;
use std::collections::{BTreeMap, HashMap, HashSet};
use std::sync::Arc;
use std::time::Duration;
use storage_primitives::{BucketId, Commitment};

/// Timeout of one request to a source. A page holds at most a few MiB, so a
/// source that takes longer is not serving.
const REQUEST_TIMEOUT: Duration = Duration::from_secs(30);

/// Hashes in one `/fetch_nodes` request. Small enough that a request stays
/// about 2 KB. A source may answer fewer nodes because of its page limits;
/// the rest come back as `next` and are requested again.
const REQUEST_HASHES: usize = 64;

/// Replica synchronization manager.
pub struct ReplicaSync {
    storage: Arc<dyn StorageBackend>,
    client: SourceClient,
}

impl ReplicaSync {
    pub fn new(storage: Arc<dyn StorageBackend>) -> Self {
        Self {
            storage,
            client: SourceClient::new(REQUEST_TIMEOUT),
        }
    }

    /// Copy the bucket's data from `sources` up to `target`, a commitment the
    /// chain holds, and install its leaves.
    ///
    /// The local bucket must exist, and its quota limits the nodes stored. A
    /// source that fails is dropped from `sources`. If every source fails, the
    /// call returns `NoSourceLeft`. The nodes stored so far remain, and a later
    /// call skips them.
    pub async fn sync_to_target(
        &self,
        bucket_id: BucketId,
        target: &Commitment,
        sources: &mut SourceSet,
    ) -> Result<(), Error> {
        let leaves = fetch_leaves(
            self.storage.as_ref(),
            &self.client,
            sources,
            bucket_id,
            target,
        )
        .await?;

        let data_roots: Vec<H256> = leaves.iter().map(|leaf| leaf.data_root).collect();
        TreeWalk::new(self.storage.as_ref(), bucket_id, &data_roots)
            .run(&self.client, sources)
            .await?;

        self.storage.import_commitment(bucket_id, target, leaves)?;
        Ok(())
    }
}

/// The chunk phase: fetch the nodes under the data roots that are not stored
/// yet.
///
/// The walk requests the deepest level with a pending hash first, so a subtree
/// finishes before the next one starts and the pending hashes stay few. A chunk
/// is stored as soon as it arrives. An internal node is stored once both of its
/// children are stored, so a stored node always has its whole subtree stored.
/// That lets the walk skip any child that is already stored, and lets a repeated
/// sync start where an interrupted one stopped.
struct TreeWalk<'a> {
    storage: &'a dyn StorageBackend,
    bucket_id: BucketId,
    /// Hashes to request, by level: the distance below the data root.
    frontier: BTreeMap<usize, Vec<H256>>,
    /// The hashes in `frontier` or `waiting`, so each is requested once.
    queued: HashSet<H256>,
    /// Internal nodes fetched but not stored, with their children.
    waiting: HashMap<H256, (H256, H256)>,
    /// The nodes in `waiting` that wait for a child, by child.
    parents: HashMap<H256, Vec<H256>>,
    /// The sources that reported a hash as missing.
    missing_on: HashMap<H256, HashSet<String>>,
}

impl<'a> TreeWalk<'a> {
    fn new(storage: &'a dyn StorageBackend, bucket_id: BucketId, data_roots: &[H256]) -> Self {
        let mut walk = Self {
            storage,
            bucket_id,
            frontier: BTreeMap::new(),
            queued: HashSet::new(),
            waiting: HashMap::new(),
            parents: HashMap::new(),
            missing_on: HashMap::new(),
        };
        let (_, absent) = storage.check_exists(bucket_id, data_roots);
        for root in absent {
            if walk.queued.insert(root) {
                walk.frontier.entry(0).or_default().push(root);
            }
        }
        walk
    }

    async fn run(mut self, client: &SourceClient, sources: &mut SourceSet) -> Result<(), Error> {
        while !self.frontier.is_empty() {
            let Some((source, level, hashes)) = self.next_batch(sources) else {
                return Err(Error::NoSourceLeft { what: "nodes" });
            };
            match client.fetch_nodes(&source, self.bucket_id, &hashes).await {
                Ok(page) => self.accept(level, &source, page)?,
                Err(e) => {
                    tracing::warn!(
                        bucket_id = self.bucket_id,
                        source,
                        "node page failed, trying the next source: {e}"
                    );
                    sources.drop_source(&source);
                }
            }
        }
        Ok(())
    }

    /// The deepest level with a hash some source can still serve, and the
    /// first such source with up to [`REQUEST_HASHES`] of those hashes. `None`
    /// when no source can serve any pending hash.
    fn next_batch(&self, sources: &SourceSet) -> Option<(String, usize, Vec<H256>)> {
        for (&level, hashes) in self.frontier.iter().rev() {
            for source in sources.iter() {
                let batch: Vec<H256> = hashes
                    .iter()
                    .filter(|hash| {
                        !self
                            .missing_on
                            .get(hash)
                            .is_some_and(|missing| missing.contains(source))
                    })
                    .take(REQUEST_HASHES)
                    .copied()
                    .collect();
                if !batch.is_empty() {
                    return Some((source.to_owned(), level, batch));
                }
            }
        }
        None
    }

    /// Store what a source returned for a request at `level`. Hashes it lists
    /// as missing stay pending and are not requested from it again. Hashes it
    /// did not process stay pending.
    fn accept(&mut self, level: usize, source: &str, page: NodePage) -> Result<(), Error> {
        for hash in page.missing {
            self.missing_on
                .entry(hash)
                .or_default()
                .insert(source.to_owned());
        }

        let fetched: HashSet<H256> = page.nodes.iter().map(|(hash, _)| *hash).collect();
        if let Some(hashes) = self.frontier.get_mut(&level) {
            hashes.retain(|hash| !fetched.contains(hash));
            if hashes.is_empty() {
                self.frontier.remove(&level);
            }
        }

        for (hash, node) in page.nodes {
            let ChunkTreeNode::Internal { left, right } = node else {
                self.store(hash, node)?;
                continue;
            };
            let (_, absent) = self.storage.check_exists(self.bucket_id, &[left, right]);
            if absent.is_empty() {
                self.store(hash, node)?;
                continue;
            }
            self.waiting.insert(hash, (left, right));
            for child in absent {
                self.parents.entry(child).or_default().push(hash);
                if self.queued.insert(child) {
                    self.frontier.entry(level + 1).or_default().push(child);
                }
            }
        }
        Ok(())
    }

    /// Store `node`, then every waiting parent whose children are now stored,
    /// and so on up the tree.
    fn store(&mut self, hash: H256, node: ChunkTreeNode) -> Result<(), Error> {
        let mut stored = vec![(hash, node)];
        while let Some((hash, node)) = stored.pop() {
            self.storage.store_node(self.bucket_id, hash, node)?;
            self.queued.remove(&hash);
            self.missing_on.remove(&hash);
            for parent in self.parents.remove(&hash).unwrap_or_default() {
                let Some(&(left, right)) = self.waiting.get(&parent) else {
                    continue;
                };
                let (_, absent) = self.storage.check_exists(self.bucket_id, &[left, right]);
                if absent.is_empty() {
                    self.waiting.remove(&parent);
                    stored.push((parent, ChunkTreeNode::Internal { left, right }));
                }
            }
        }
        Ok(())
    }
}
