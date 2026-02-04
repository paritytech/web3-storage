//! Disk-based persistent storage backend using RocksDB.
//!
//! This provides the same interface as the in-memory storage but persists
//! all data to disk for production use.

use crate::error::Error;
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

/// A stored node (chunk or internal node).
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct StoredNode {
    /// The raw data
    pub data: Vec<u8>,
    /// Child hashes for internal nodes
    pub children: Option<Vec<H256>>,
}

/// Bucket state managed by this provider.
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
            .map_err(|e| Error::Storage(format!("Failed to open RocksDB: {}", e)))?;

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
        if self.db.get_cf(&cf, &key).map_err(|e| Error::Storage(e.to_string()))?.is_some() {
            return Ok(()); // Already exists
        }

        let bucket = BucketState::new(max_bytes);
        let value = bincode::serialize(&bucket)
            .map_err(|e| Error::Serialization(e.to_string()))?;

        self.db
            .put_cf(&cf, &key, &value)
            .map_err(|e| Error::Storage(e.to_string()))?;

        Ok(())
    }

    /// Get bucket state.
    pub fn get_bucket(&self, bucket_id: BucketId) -> Option<BucketState> {
        let cf = self.db.cf_handle(CF_BUCKETS)?;
        let key = bucket_id.to_le_bytes();
        let value = self.db.get_cf(&cf, &key).ok()??;
        bincode::deserialize(&value).ok()
    }

    /// Update bucket state.
    fn update_bucket(&self, bucket_id: BucketId, bucket: &BucketState) -> Result<(), Error> {
        let cf = self
            .db
            .cf_handle(CF_BUCKETS)
            .ok_or_else(|| Error::Storage("Buckets CF not found".to_string()))?;

        let key = bucket_id.to_le_bytes();
        let value = bincode::serialize(bucket)
            .map_err(|e| Error::Serialization(e.to_string()))?;

        self.db
            .put_cf(&cf, &key, &value)
            .map_err(|e| Error::Storage(e.to_string()))?;

        Ok(())
    }

    /// List all buckets.
    pub fn list_buckets(&self) -> Vec<BucketSummary> {
        let cf = match self.db.cf_handle(CF_BUCKETS) {
            Some(cf) => cf,
            None => return vec![],
        };

        let iter = self.db.iterator_cf(&cf, rocksdb::IteratorMode::Start);
        let mut summaries = Vec::new();

        for item in iter {
            if let Ok((key, value)) = item {
                if key.len() == 8 {
                    let bucket_id = u64::from_le_bytes(key[..8].try_into().unwrap());
                    if let Ok(state) = bincode::deserialize::<BucketState>(&value) {
                        summaries.push(BucketSummary {
                            bucket_id,
                            mmr_root: format!("0x{}", hex::encode(state.mmr_root.as_bytes())),
                            start_seq: state.start_seq,
                            leaf_count: state.leaf_count(),
                        });
                    }
                }
            }
        }

        summaries
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

            for child_hash in child_hashes {
                let key = child_hash.as_bytes();
                if self.db.get_cf(&cf_nodes, key).map_err(|e| Error::Storage(e.to_string()))?.is_none() {
                    return Err(Error::ChildrenMissing(vec![
                        format!("0x{}", hex::encode(child_hash.as_bytes()))
                    ]));
                }
            }
        }

        // Check quota
        let mut bucket = self.get_bucket(bucket_id)
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
        if self.db.get_cf(&cf_nodes, key).map_err(|e| Error::Storage(e.to_string()))?.is_none() {
            let data_len = data.len() as u64;
            let node = StoredNode { data, children };
            let value = bincode::serialize(&node)
                .map_err(|e| Error::Serialization(e.to_string()))?;

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
        bincode::deserialize(&value).ok()
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
            if self.db.get_cf(&cf_nodes, key).map_err(|e| Error::Storage(e.to_string()))?.is_none() {
                return Err(Error::RootNotFound(format!(
                    "0x{}",
                    hex::encode(root.as_bytes())
                )));
            }
        }

        // Get bucket and update MMR
        let mut bucket = self.get_bucket(bucket_id)
            .ok_or(Error::BucketNotFound(bucket_id))?;

        let start_seq = bucket.start_seq.saturating_add(bucket.leaf_count());
        let mut leaf_indices = Vec::new();
        let mut mmr = crate::mmr::Mmr::new();

        // Rebuild MMR from existing leaves
        for leaf in &bucket.leaves {
            mmr.push(blake2_256(&leaf.encode()));
        }

        // Add new leaves
        for data_root in data_roots {
            let leaf = MmrLeaf {
                data_root,
                data_size: 0, // Would calculate from node tree
                total_size: 0, // Would track cumulative
            };
            let leaf_hash = blake2_256(&leaf.encode());
            let leaf_idx = mmr.push(leaf_hash);
            leaf_indices.push(leaf_idx);
            bucket.leaves.push(leaf);
        }

        bucket.mmr_root = mmr.root();

        // Update bucket
        self.update_bucket(bucket_id, &bucket)?;

        Ok((bucket.mmr_root, start_seq, leaf_indices))
    }

    /// Get chunk at a specific index within a data root.
    pub fn get_chunk_at_index(
        &self,
        _data_root: H256,
        _chunk_index: u64,
    ) -> Result<(Vec<u8>, Vec<H256>), Error> {
        // Simplified implementation
        Err(Error::Storage("Not implemented".to_string()))
    }

    /// Delete data before a given sequence number.
    pub fn delete_before(
        &self,
        bucket_id: BucketId,
        new_start_seq: u64,
    ) -> Result<(H256, u64, u64), Error> {
        let mut bucket = self.get_bucket(bucket_id)
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
    ) -> Result<(MmrLeaf, Vec<H256>), Error> {
        let bucket = self.get_bucket(bucket_id)
            .ok_or(Error::BucketNotFound(bucket_id))?;

        let leaf = bucket
            .leaves
            .get(leaf_index as usize)
            .ok_or(Error::Storage("Leaf not found".to_string()))?;

        // Build MMR and generate proof
        let mut mmr = crate::mmr::Mmr::new();
        for l in &bucket.leaves {
            mmr.push(blake2_256(&l.encode()));
        }

        let proof = mmr.proof(leaf_index)
            .ok_or(Error::Storage("Failed to generate proof".to_string()))?;

        Ok((leaf.clone(), proof.peaks))
    }

    /// Get MMR peaks.
    pub fn get_mmr_peaks(&self, bucket_id: BucketId) -> Result<(H256, Vec<H256>), Error> {
        let bucket = self.get_bucket(bucket_id)
            .ok_or(Error::BucketNotFound(bucket_id))?;

        let mut mmr = crate::mmr::Mmr::new();
        for leaf in &bucket.leaves {
            mmr.push(blake2_256(&leaf.encode()));
        }

        Ok((mmr.root(), mmr.peaks()))
    }
}
