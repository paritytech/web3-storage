// SPDX-License-Identifier: Apache-2.0

//! In-memory Layer 0 for the crate's tests.

use crate::format::{compute_cid, Cid};
use crate::tree::{BlobLength, BlobStore, Tree};
use crate::{FsClientError, Result};
use std::collections::BTreeMap;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Mutex;
use storage_primitives::{mmr, BucketId, Commitment, MmrLeaf, MmrProof, LEAF_VERSION};

pub(crate) const BUCKET: BucketId = 7;

/// Entry `mtime` of every write through [`tree`], so CIDs are reproducible.
pub(crate) const MTIME: u64 = 1_700_000_000;

/// Blobs by CID, the committed `data_roots` in order (one MMR leaf each), and
/// the number of blob reads.
#[derive(Default)]
pub(crate) struct MemoryStore {
    pub(crate) blobs: Mutex<BTreeMap<Cid, Vec<u8>>>,
    pub(crate) commits: Mutex<Vec<Vec<Cid>>>,
    pub(crate) reads: AtomicUsize,
    /// Replaces the bytes returned for this CID.
    pub(crate) tampered: Mutex<Option<(Cid, Vec<u8>)>>,
    /// Committed right after the next `commitment` answer, as another writer
    /// would.
    pub(crate) commit_after_commitment: Mutex<Option<Vec<u8>>>,
}

impl MemoryStore {
    pub(crate) fn commits(&self) -> Vec<Vec<Cid>> {
        self.commits.lock().unwrap().clone()
    }

    /// Store `data` and commit it as one leaf, as a writer other than the
    /// tree would.
    pub(crate) fn raw_commit(&self, data: &[u8]) {
        let cid = compute_cid(data);
        self.blobs.lock().unwrap().insert(cid, data.to_vec());
        self.commits.lock().unwrap().push(vec![cid]);
    }

    fn leaves(&self) -> Vec<MmrLeaf> {
        self.commits
            .lock()
            .unwrap()
            .iter()
            .flatten()
            .map(|&data_root| MmrLeaf {
                leaf_version: LEAF_VERSION,
                data_root,
                data_size: 0,
                total_size: 0,
            })
            .collect()
    }
}

#[async_trait::async_trait]
impl BlobStore for MemoryStore {
    async fn put_blob(&self, _bucket_id: BucketId, data: &[u8]) -> Result<Cid> {
        let cid = compute_cid(data);
        self.blobs.lock().unwrap().insert(cid, data.to_vec());
        Ok(cid)
    }

    async fn get_blob(
        &self,
        _bucket_id: BucketId,
        cid: Cid,
        _length: BlobLength,
    ) -> Result<Vec<u8>> {
        self.reads.fetch_add(1, Ordering::Relaxed);
        if let Some((tampered, bytes)) = self.tampered.lock().unwrap().clone() {
            if tampered == cid {
                return Ok(bytes);
            }
        }
        self.blobs
            .lock()
            .unwrap()
            .get(&cid)
            .cloned()
            .ok_or_else(|| FsClientError::StorageClient(format!("missing blob {cid:?}")))
    }

    async fn commit(&self, _bucket_id: BucketId, data_roots: Vec<Cid>) -> Result<()> {
        let blobs = self.blobs.lock().unwrap();
        assert!(data_roots.iter().all(|c| blobs.contains_key(c)));
        self.commits.lock().unwrap().push(data_roots);
        Ok(())
    }

    async fn commitment(&self, _bucket_id: BucketId) -> Result<Option<Commitment>> {
        let leaves = self.leaves();
        let commitment = (!leaves.is_empty()).then(|| Commitment {
            mmr_root: mmr::root(&leaves).unwrap(),
            start_seq: 0,
            leaf_count: leaves.len() as u64,
        });
        if let Some(data) = self.commit_after_commitment.lock().unwrap().take() {
            self.raw_commit(&data);
        }
        Ok(commitment)
    }

    async fn mmr_proof(&self, _bucket_id: BucketId, leaf_index: u64) -> Result<MmrProof> {
        Ok(mmr::proof(&self.leaves(), leaf_index)
            .unwrap()
            .expect("no leaf at this index")
            .1)
    }
}

/// The tree of [`BUCKET`] in `store`, with the fixed clock [`MTIME`].
pub(crate) fn tree(store: &MemoryStore) -> Tree<&MemoryStore> {
    Tree::with_clock(store, BUCKET, || MTIME)
}
