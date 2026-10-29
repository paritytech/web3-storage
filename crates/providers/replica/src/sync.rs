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
use std::collections::{BTreeMap, HashMap, HashSet, VecDeque};
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
    /// The local bucket must exist, and its quota limits the nodes stored.
    ///
    /// Each phase starts with all of `sources`. A source that fails in the
    /// leaf phase can still serve nodes: a primary of a private bucket answers
    /// 401 to `/mmr_leaves` but serves `/fetch_nodes`. A source that fails in
    /// the chunk phase is dropped from `sources`. If every source fails, the
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
            &mut sources.clone(),
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
    frontier: BTreeMap<usize, VecDeque<H256>>,
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
                walk.frontier.entry(0).or_default().push_back(root);
            }
        }
        walk
    }

    async fn run(mut self, client: &SourceClient, sources: &mut SourceSet) -> Result<(), Error> {
        while let Some(mut deepest) = self.frontier.last_entry() {
            let level = *deepest.key();
            let Some((source, batch)) = take_batch(deepest.get_mut(), &self.missing_on, sources)?
            else {
                deepest.remove();
                continue;
            };
            if deepest.get().is_empty() {
                deepest.remove();
            }
            match client.fetch_nodes(&source, self.bucket_id, &batch).await {
                Ok(page) => self.accept(level, &source, page)?,
                Err(e) => {
                    tracing::warn!(
                        bucket_id = self.bucket_id,
                        source,
                        "node page failed, trying the next source: {e}"
                    );
                    sources.drop_source(&source);
                    self.requeue(level, batch);
                }
            }
        }
        Ok(())
    }

    /// Put hashes that were taken but not fetched back at the end of `level`.
    fn requeue(&mut self, level: usize, hashes: impl IntoIterator<Item = H256>) {
        let mut hashes = hashes.into_iter().peekable();
        if hashes.peek().is_some() {
            self.frontier.entry(level).or_default().extend(hashes);
        }
    }

    /// Store what a source returned for a request at `level`. Hashes it lists
    /// as missing go back to the level and are not requested from it again.
    /// Hashes it did not process go back to the level.
    fn accept(&mut self, level: usize, source: &str, page: NodePage) -> Result<(), Error> {
        for hash in &page.missing {
            self.missing_on
                .entry(*hash)
                .or_default()
                .insert(source.to_owned());
        }
        self.requeue(level, page.missing.into_iter().chain(page.next));

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
                    self.frontier.entry(level + 1).or_default().push_back(child);
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

/// Take up to [`REQUEST_HASHES`] hashes from the front of `queue` and the
/// source to request them from: the first source that has not reported the
/// first hash as missing. Hashes that source reported as missing go to the back
/// of the queue. `None` for an empty queue. Fails with `NoSourceLeft` when every
/// source reported the first hash as missing.
fn take_batch(
    queue: &mut VecDeque<H256>,
    missing_on: &HashMap<H256, HashSet<String>>,
    sources: &SourceSet,
) -> Result<Option<(String, Vec<H256>)>, Error> {
    let reported_missing = |hash: &H256, source: &str| {
        missing_on
            .get(hash)
            .is_some_and(|sources| sources.contains(source))
    };
    let Some(first) = queue.front() else {
        return Ok(None);
    };
    let source = sources
        .iter()
        .find(|source| !reported_missing(first, source))
        .ok_or(Error::NoSourceLeft { what: "nodes" })?
        .to_owned();

    let mut batch = Vec::new();
    let mut skipped = Vec::new();
    for _ in 0..REQUEST_HASHES {
        let Some(hash) = queue.pop_front() else {
            break;
        };
        if reported_missing(&hash, &source) {
            skipped.push(hash);
        } else {
            batch.push(hash);
        }
    }
    queue.extend(skipped);
    Ok(Some((source, batch)))
}
