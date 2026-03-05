//! Disk-based persistent storage backend using RocksDB.
//!
//! This provides the same interface as the in-memory storage but persists
//! all data to disk for production use.

use crate::error::Error;
use crate::storage::StoredNode;
use crate::types::*;
use codec::Encode;
use rocksdb::{Options, DB};
use sp_core::H256;
use std::path::Path;
use std::sync::Arc;
use storage_primitives::{blake2_256, BucketId, MmrLeaf};

/// Column families for organizing data
const CF_NODES: &str = "nodes";
const CF_BUCKETS: &str = "buckets";
const CF_ROOT_TO_BUCKET: &str = "root_to_bucket";

/// Bucket state managed by this provider (serialized to disk).
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct BucketState {
    /// Current MMR root
    pub mmr_root: H256,
    /// Start sequence number
    pub start_seq: u64,
    /// MMR leaves
    pub leaves: Vec<MmrLeaf>,
    /// Quota used in bytes
    pub used_bytes: u64,
    /// Maximum quota for this bucket
    pub max_bytes: u64,
}

impl BucketState {
    pub fn new(max_bytes: u64) -> Self {
        Self {
            mmr_root: H256::zero(),
            start_seq: 0,
            leaves: Vec::new(),
            used_bytes: 0,
            max_bytes,
        }
    }

    pub fn leaf_count(&self) -> u64 {
        self.leaves.len() as u64
    }
}

/// Disk-based storage backend using RocksDB.
pub struct DiskStorage {
    db: Arc<DB>,
}

impl DiskStorage {
    /// Create a new disk storage instance.
    pub fn new(path: impl AsRef<Path>) -> Result<Self, Error> {
        let mut opts = Options::default();
        opts.create_if_missing(true);
        opts.create_missing_column_families(true);

        // Define column families
        let cf_names = vec![CF_NODES, CF_BUCKETS, CF_ROOT_TO_BUCKET];

        let db = DB::open_cf(&opts, path, &cf_names)
            .map_err(|e| Error::Storage(format!("Failed to open RocksDB: {e}")))?;

        Ok(Self { db: Arc::new(db) })
    }

    /// Initialize a bucket with the given quota.
    pub fn init_bucket(&self, bucket_id: BucketId, max_bytes: u64) -> Result<(), Error> {
        let cf = self
            .db
            .cf_handle(CF_BUCKETS)
            .ok_or_else(|| Error::Storage("Buckets CF not found".to_string()))?;

        // Check if bucket already exists
        let key = bucket_id.to_le_bytes();
        if self
            .db
            .get_cf(&cf, key)
            .map_err(|e| Error::Storage(e.to_string()))?
            .is_some()
        {
            return Ok(()); // Already exists
        }

        let bucket = BucketState::new(max_bytes);
        let value = bincode::serialize(&bucket).map_err(|e| Error::Serialization(e.to_string()))?;

        self.db
            .put_cf(&cf, key, &value)
            .map_err(|e| Error::Storage(e.to_string()))?;

        Ok(())
    }

    /// Get bucket state (internal, returns full BucketState).
    pub fn get_bucket(&self, bucket_id: BucketId) -> Option<BucketState> {
        let cf = self.db.cf_handle(CF_BUCKETS)?;
        let key = bucket_id.to_le_bytes();
        let value = self.db.get_cf(&cf, key).ok()??;
        match bincode::deserialize(&value) {
            Ok(state) => Some(state),
            Err(e) => {
                tracing::warn!(bucket_id, error = %e, "Failed to deserialize bucket state");
                None
            }
        }
    }

    /// Update bucket state.
    fn update_bucket(&self, bucket_id: BucketId, bucket: &BucketState) -> Result<(), Error> {
        let cf = self
            .db
            .cf_handle(CF_BUCKETS)
            .ok_or_else(|| Error::Storage("Buckets CF not found".to_string()))?;

        let key = bucket_id.to_le_bytes();
        let value = bincode::serialize(bucket).map_err(|e| Error::Serialization(e.to_string()))?;

        self.db
            .put_cf(&cf, key, &value)
            .map_err(|e| Error::Storage(e.to_string()))?;

        Ok(())
    }

    /// Iterate over all buckets, applying a mapping function to each.
    fn iter_buckets<T>(&self, f: impl Fn(BucketId, &BucketState) -> T) -> Vec<T> {
        let cf = match self.db.cf_handle(CF_BUCKETS) {
            Some(cf) => cf,
            None => return vec![],
        };

        self.db
            .iterator_cf(&cf, rocksdb::IteratorMode::Start)
            .flatten()
            .filter_map(|(key, value)| {
                if key.len() != 8 {
                    return None;
                }
                let bucket_id = u64::from_le_bytes(key[..8].try_into().unwrap());
                match bincode::deserialize::<BucketState>(&value) {
                    Ok(state) => Some(f(bucket_id, &state)),
                    Err(e) => {
                        tracing::warn!(bucket_id, error = %e, "Failed to deserialize bucket state");
                        None
                    }
                }
            })
            .collect()
    }

    /// List all buckets.
    pub fn list_buckets(&self) -> Vec<BucketSummary> {
        self.iter_buckets(|bucket_id, state| BucketSummary {
            bucket_id,
            mmr_root: format!("0x{}", hex::encode(state.mmr_root.as_bytes())),
            start_seq: state.start_seq,
            leaf_count: state.leaf_count(),
        })
    }

    /// Get storage statistics per bucket.
    pub fn get_bucket_stats(&self) -> Vec<BucketStats> {
        self.iter_buckets(|bucket_id, state| BucketStats {
            bucket_id,
            leaf_count: state.leaf_count(),
            node_count: 0, // Would need per-bucket tracking
            bytes_stored: state.used_bytes,
        })
    }

    /// Get total node count.
    pub fn total_nodes(&self) -> u64 {
        let cf = match self.db.cf_handle(CF_NODES) {
            Some(cf) => cf,
            None => return 0,
        };
        self.db
            .iterator_cf(&cf, rocksdb::IteratorMode::Start)
            .flatten()
            .count() as u64
    }

    /// Get total bytes stored across all buckets.
    ///
    /// Sums per-bucket `used_bytes` from CF_BUCKETS instead of scanning all nodes,
    /// since each bucket already tracks its byte usage.
    pub fn total_bytes(&self) -> u64 {
        self.iter_buckets(|_, state| state.used_bytes)
            .into_iter()
            .sum()
    }

    /// Store a node (chunk or internal node).
    pub fn store_node(
        &self,
        bucket_id: BucketId,
        expected_hash: H256,
        data: Vec<u8>,
        children: Option<Vec<H256>>,
    ) -> Result<(), Error> {
        // Verify hash
        let actual_hash = blake2_256(&data);
        if actual_hash != expected_hash {
            return Err(Error::InvalidHash {
                expected: format!("0x{}", hex::encode(expected_hash.as_bytes())),
                actual: format!("0x{}", hex::encode(actual_hash.as_bytes())),
            });
        }

        // If internal node, verify children exist
        if let Some(ref child_hashes) = children {
            let cf_nodes = self
                .db
                .cf_handle(CF_NODES)
                .ok_or_else(|| Error::Storage("Nodes CF not found".to_string()))?;

            let missing: Vec<String> = child_hashes
                .iter()
                .filter(|h| {
                    **h != H256::zero()
                        && self
                            .db
                            .get_cf(&cf_nodes, h.as_bytes())
                            .ok()
                            .flatten()
                            .is_none()
                })
                .map(|h| format!("0x{}", hex::encode(h.as_bytes())))
                .collect();

            if !missing.is_empty() {
                return Err(Error::ChildrenMissing(missing));
            }
        }

        // Check quota
        let mut bucket = self
            .get_bucket(bucket_id)
            .ok_or(Error::BucketNotFound(bucket_id))?;

        let new_size = bucket.used_bytes.saturating_add(data.len() as u64);
        if new_size > bucket.max_bytes {
            return Err(Error::QuotaExceeded {
                used: bucket.used_bytes,
                max: bucket.max_bytes,
            });
        }

        // Store node
        let cf_nodes = self
            .db
            .cf_handle(CF_NODES)
            .ok_or_else(|| Error::Storage("Nodes CF not found".to_string()))?;

        let key = expected_hash.as_bytes();
        if self
            .db
            .get_cf(&cf_nodes, key)
            .map_err(|e| Error::Storage(e.to_string()))?
            .is_none()
        {
            let data_len = data.len() as u64;
            let node = StoredNode { data, children };
            let value =
                bincode::serialize(&node).map_err(|e| Error::Serialization(e.to_string()))?;

            self.db
                .put_cf(&cf_nodes, key, &value)
                .map_err(|e| Error::Storage(e.to_string()))?;

            // Update quota
            bucket.used_bytes = bucket.used_bytes.saturating_add(data_len);
            self.update_bucket(bucket_id, &bucket)?;
        }

        Ok(())
    }

    /// Get a node by hash.
    pub fn get_node(&self, hash: &H256) -> Option<StoredNode> {
        let cf = self.db.cf_handle(CF_NODES)?;
        let key = hash.as_bytes();
        let value = self.db.get_cf(&cf, key).ok()??;
        match bincode::deserialize(&value) {
            Ok(node) => Some(node),
            Err(e) => {
                tracing::warn!(hash = %format!("0x{}", hex::encode(hash.as_bytes())), error = %e, "Failed to deserialize node");
                None
            }
        }
    }

    /// Calculate the size of a content tree by traversing stored nodes.
    fn calculate_tree_size(&self, root: H256) -> u64 {
        let mut size = 0u64;
        let mut stack = vec![root];

        while let Some(hash) = stack.pop() {
            if let Some(node) = self.get_node(&hash) {
                if let Some(ref children) = node.children {
                    stack.extend(children.iter().copied());
                } else {
                    size = size.saturating_add(node.data.len() as u64);
                }
            }
        }

        size
    }

    /// Check which hashes exist.
    pub fn check_exists(&self, _bucket_id: BucketId, hashes: &[H256]) -> (Vec<H256>, Vec<H256>) {
        let cf = match self.db.cf_handle(CF_NODES) {
            Some(cf) => cf,
            None => return (vec![], hashes.to_vec()),
        };

        let mut exists = Vec::new();
        let mut missing = Vec::new();

        for hash in hashes {
            let key = hash.as_bytes();
            if self.db.get_cf(&cf, key).ok().flatten().is_some() {
                exists.push(*hash);
            } else {
                missing.push(*hash);
            }
        }

        (exists, missing)
    }

    /// Commit data roots to the bucket's MMR.
    pub fn commit(
        &self,
        bucket_id: BucketId,
        data_roots: Vec<H256>,
    ) -> Result<(H256, u64, Vec<u64>), Error> {
        // Verify all roots exist
        let cf_nodes = self
            .db
            .cf_handle(CF_NODES)
            .ok_or_else(|| Error::Storage("Nodes CF not found".to_string()))?;

        for root in &data_roots {
            let key = root.as_bytes();
            if self
                .db
                .get_cf(&cf_nodes, key)
                .map_err(|e| Error::Storage(e.to_string()))?
                .is_none()
            {
                return Err(Error::RootNotFound(format!(
                    "0x{}",
                    hex::encode(root.as_bytes())
                )));
            }
        }

        // Get bucket and update MMR
        let mut bucket = self
            .get_bucket(bucket_id)
            .ok_or(Error::BucketNotFound(bucket_id))?;

        let start_seq = bucket.start_seq;
        let mut leaf_indices = Vec::new();
        let mut mmr = crate::mmr::Mmr::new();

        // Rebuild MMR from existing leaves
        for leaf in &bucket.leaves {
            mmr.push(blake2_256(&leaf.encode()));
        }

        // Add new leaves
        let start_index = bucket.leaves.len() as u64;
        for (i, data_root) in data_roots.iter().enumerate() {
            leaf_indices.push(start_index + i as u64);

            // Calculate data size by traversing the stored node tree
            let data_size = self.calculate_tree_size(*data_root);
            let total_size = bucket
                .leaves
                .last()
                .map(|l| l.total_size)
                .unwrap_or(0)
                .saturating_add(data_size);

            let leaf = MmrLeaf {
                data_root: *data_root,
                data_size,
                total_size,
            };
            let leaf_hash = blake2_256(&leaf.encode());
            mmr.push(leaf_hash);
            bucket.leaves.push(leaf);
        }

        bucket.mmr_root = mmr.root();

        // Update bucket
        self.update_bucket(bucket_id, &bucket)?;

        Ok((bucket.mmr_root, start_seq, leaf_indices))
    }

    /// Delete data before a given sequence number.
    pub fn delete_before(
        &self,
        bucket_id: BucketId,
        new_start_seq: u64,
    ) -> Result<(H256, u64, u64), Error> {
        let mut bucket = self
            .get_bucket(bucket_id)
            .ok_or(Error::BucketNotFound(bucket_id))?;

        // Remove leaves before new_start_seq
        let to_remove = (new_start_seq - bucket.start_seq) as usize;
        if to_remove > 0 && to_remove <= bucket.leaves.len() {
            bucket.leaves.drain(0..to_remove);
            bucket.start_seq = new_start_seq;

            // Recalculate MMR
            let mut mmr = crate::mmr::Mmr::new();
            for leaf in &bucket.leaves {
                mmr.push(blake2_256(&leaf.encode()));
            }
            bucket.mmr_root = mmr.root();

            self.update_bucket(bucket_id, &bucket)?;
        }

        Ok((bucket.mmr_root, bucket.start_seq, bucket.leaf_count()))
    }

    /// Get MMR proof for a leaf.
    pub fn get_mmr_proof(
        &self,
        bucket_id: BucketId,
        leaf_index: u64,
    ) -> Result<storage_primitives::MmrProof, Error> {
        let bucket = self
            .get_bucket(bucket_id)
            .ok_or(Error::BucketNotFound(bucket_id))?;

        let leaf = bucket
            .leaves
            .get(leaf_index as usize)
            .ok_or(Error::NodeNotFound(format!("leaf_{leaf_index}")))?
            .clone();

        // Build MMR and generate proof
        let mut mmr = crate::mmr::Mmr::new();
        for l in &bucket.leaves {
            mmr.push(blake2_256(&l.encode()));
        }

        let (siblings, path, peaks) = mmr
            .proof_with_path(leaf_index)
            .ok_or(Error::NodeNotFound(format!("mmr_proof_{leaf_index}")))?;

        Ok(storage_primitives::MmrProof {
            peaks,
            leaf,
            leaf_proof: storage_primitives::MerkleProof { siblings, path },
        })
    }

    /// Get MMR peaks.
    pub fn get_mmr_peaks(&self, bucket_id: BucketId) -> Result<(H256, Vec<H256>), Error> {
        let bucket = self
            .get_bucket(bucket_id)
            .ok_or(Error::BucketNotFound(bucket_id))?;

        let mut mmr = crate::mmr::Mmr::new();
        for leaf in &bucket.leaves {
            mmr.push(blake2_256(&leaf.encode()));
        }

        Ok((mmr.root(), mmr.peaks()))
    }
}

impl crate::StorageBackend for DiskStorage {
    fn init_bucket(&self, bucket_id: BucketId, max_bytes: u64) -> Result<(), crate::Error> {
        self.init_bucket(bucket_id, max_bytes)
    }

    fn get_bucket(&self, bucket_id: BucketId) -> Option<crate::BucketInfo> {
        let state = DiskStorage::get_bucket(self, bucket_id)?;
        Some(crate::BucketInfo {
            mmr_root: state.mmr_root,
            start_seq: state.start_seq,
            leaf_count: state.leaf_count(),
        })
    }

    fn list_buckets(&self) -> Vec<BucketSummary> {
        self.list_buckets()
    }

    fn get_bucket_stats(&self) -> Vec<BucketStats> {
        self.get_bucket_stats()
    }

    fn total_nodes(&self) -> u64 {
        self.total_nodes()
    }

    fn total_bytes(&self) -> u64 {
        self.total_bytes()
    }

    fn store_node(
        &self,
        bucket_id: BucketId,
        expected_hash: H256,
        data: Vec<u8>,
        children: Option<Vec<H256>>,
    ) -> Result<(), crate::Error> {
        self.store_node(bucket_id, expected_hash, data, children)
    }

    fn get_node(&self, hash: &H256) -> Option<StoredNode> {
        self.get_node(hash)
    }

    fn check_exists(&self, bucket_id: BucketId, hashes: &[H256]) -> (Vec<H256>, Vec<H256>) {
        self.check_exists(bucket_id, hashes)
    }

    fn commit(
        &self,
        bucket_id: BucketId,
        data_roots: Vec<H256>,
    ) -> Result<(H256, u64, Vec<u64>), crate::Error> {
        self.commit(bucket_id, data_roots)
    }

    fn delete_before(
        &self,
        bucket_id: BucketId,
        new_start_seq: u64,
    ) -> Result<(H256, u64, u64), crate::Error> {
        self.delete_before(bucket_id, new_start_seq)
    }

    fn get_mmr_proof(
        &self,
        bucket_id: BucketId,
        leaf_index: u64,
    ) -> Result<storage_primitives::MmrProof, crate::Error> {
        self.get_mmr_proof(bucket_id, leaf_index)
    }

    fn get_mmr_peaks(&self, bucket_id: BucketId) -> Result<(H256, Vec<H256>), crate::Error> {
        self.get_mmr_peaks(bucket_id)
    }
}
