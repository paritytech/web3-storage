// SPDX-License-Identifier: Apache-2.0

//! RocksDB-backed [`StorageBackend`]: persists the [`types`] records to disk.
//!
//! Only the engine lives here - the record shapes it reads and writes are
//! defined in [`types`], which owns their on-disk encoding.
//!
//! [`types`]: super::types

use super::{BucketInfo, BucketState, BucketStats, BucketSummary, StorageBackend, StoredNode};
use crate::error::Error;
use codec::{DecodeAll, Encode};
use rocksdb::{Options, DB};
use sp_core::H256;
use std::path::Path;
use std::sync::Arc;
use storage_primitives::{blake2_256, BucketId, MmrLeaf};

/// Column families for organizing data
const CF_NODES: &str = "nodes";
const CF_BUCKETS: &str = "buckets";
const CF_ROOT_TO_BUCKET: &str = "root_to_bucket";
/// Small metadata values. Currently unused — retained so existing databases
/// still open; a previous version stored the negotiation nonce counter here.
const CF_METADATA: &str = "metadata";

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
        let cf_names = vec![CF_NODES, CF_BUCKETS, CF_ROOT_TO_BUCKET, CF_METADATA];

        let db = DB::open_cf(&opts, path, &cf_names)?;

        Ok(Self { db: Arc::new(db) })
    }

    /// Initialize a bucket with the given quota.
    pub fn init_bucket(&self, bucket_id: BucketId, max_bytes: u64) -> Result<(), Error> {
        let cf = self
            .db
            .cf_handle(CF_BUCKETS)
            .ok_or(Error::ColumnFamilyMissing(CF_BUCKETS))?;

        // Check if bucket already exists
        let key = bucket_id.to_le_bytes();
        if self.db.get_cf(&cf, key)?.is_some() {
            return Ok(()); // Already exists
        }

        let bucket = BucketState::new(max_bytes);
        let value = bucket.encode();

        self.db.put_cf(&cf, key, &value)?;

        Ok(())
    }

    /// Get bucket state (internal, returns full BucketState).
    fn get_bucket(&self, bucket_id: BucketId) -> Option<BucketState> {
        let cf = self.db.cf_handle(CF_BUCKETS)?;
        let key = bucket_id.to_le_bytes();
        let value = self.db.get_cf(&cf, key).ok()??;
        match BucketState::decode_all(&mut &value[..]) {
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
            .ok_or(Error::ColumnFamilyMissing(CF_BUCKETS))?;

        let key = bucket_id.to_le_bytes();
        let value = bucket.encode();

        self.db.put_cf(&cf, key, &value)?;

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
                match BucketState::decode_all(&mut &value[..]) {
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
                .ok_or(Error::ColumnFamilyMissing(CF_NODES))?;

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
            .ok_or(Error::ColumnFamilyMissing(CF_NODES))?;

        let key = expected_hash.as_bytes();
        if self.db.get_cf(&cf_nodes, key)?.is_none() {
            let data_len = data.len() as u64;
            let node = StoredNode { data, children };
            let value = node.encode();

            self.db.put_cf(&cf_nodes, key, &value)?;

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
        match StoredNode::decode_all(&mut &value[..]) {
            Ok(node) => Some(node),
            Err(e) => {
                tracing::warn!(hash = %format!("0x{}", hex::encode(hash.as_bytes())), error = %e, "Failed to deserialize node");
                None
            }
        }
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
            .ok_or(Error::ColumnFamilyMissing(CF_NODES))?;

        for root in &data_roots {
            let key = root.as_bytes();
            if self.db.get_cf(&cf_nodes, key)?.is_none() {
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

impl StorageBackend for DiskStorage {
    fn init_bucket(&self, bucket_id: BucketId, max_bytes: u64) -> Result<(), Error> {
        self.init_bucket(bucket_id, max_bytes)
    }

    fn get_bucket(&self, bucket_id: BucketId) -> Option<BucketInfo> {
        let state = DiskStorage::get_bucket(self, bucket_id)?;
        Some(BucketInfo {
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
    ) -> Result<(), Error> {
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
    ) -> Result<(H256, u64, Vec<u64>), Error> {
        self.commit(bucket_id, data_roots)
    }

    fn delete_before(
        &self,
        bucket_id: BucketId,
        new_start_seq: u64,
    ) -> Result<(H256, u64, u64), Error> {
        self.delete_before(bucket_id, new_start_seq)
    }

    fn get_mmr_proof(
        &self,
        bucket_id: BucketId,
        leaf_index: u64,
    ) -> Result<storage_primitives::MmrProof, Error> {
        self.get_mmr_proof(bucket_id, leaf_index)
    }

    fn get_mmr_peaks(&self, bucket_id: BucketId) -> Result<(H256, Vec<H256>), Error> {
        self.get_mmr_peaks(bucket_id)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    /// Pins the raw keys and values this engine writes. A failure here means
    /// existing provider databases can no longer be read - a storage
    /// version/migration is required
    /// (see <https://github.com/paritytech/web3-storage/issues/375>).
    ///
    /// The record encodings themselves are pinned in [`super::types`].
    #[test]
    fn on_disk_bytes() {
        // Raw keys and values as written through the public API.
        assert_eq!(
            [CF_NODES, CF_BUCKETS, CF_ROOT_TO_BUCKET, CF_METADATA],
            ["nodes", "buckets", "root_to_bucket", "metadata"],
            "column-family names locate every record on disk",
        );

        let dir = TempDir::new().unwrap();
        let storage = DiskStorage::new(dir.path()).unwrap();

        // CF_BUCKETS: key = bucket_id as u64 little-endian, value = SCALE(BucketState).
        let bucket_id: BucketId = 0x0102030405060708;
        storage.init_bucket(bucket_id, 1_000).unwrap();
        let cf = storage.db.cf_handle(CF_BUCKETS).unwrap();
        let key = hex::decode("0807060504030201").unwrap();
        let raw = storage
            .db
            .get_cf(&cf, key)
            .unwrap()
            .expect("bucket must be stored under the little-endian bucket_id key");
        assert_eq!(
            hex::encode(&raw),
            // BucketState::new(1_000): zero root, no leaves, max_bytes = 1_000
            "0000000000000000000000000000000000000000000000000000000000000000\
             0000000000000000\
             00\
             0000000000000000\
             e803000000000000"
        );

        // CF_NODES: key = blake2_256(data), value = SCALE(StoredNode).
        let data = vec![1u8, 2, 3, 4, 5];
        let hash = blake2_256(&data);
        storage.store_node(bucket_id, hash, data, None).unwrap();
        let cf = storage.db.cf_handle(CF_NODES).unwrap();
        let raw = storage.db.get_cf(&cf, hash.as_bytes()).unwrap().unwrap();
        assert_eq!(hex::encode(&raw), "14010203040500");

        // CF_METADATA: retained for existing databases to reopen into, but
        // nothing writes to it any more.
        let cf = storage.db.cf_handle(CF_METADATA).unwrap();
        assert!(
            storage.db.iterator_cf(&cf, rocksdb::IteratorMode::Start).next().is_none(),
            "metadata column family must be empty"
        );
    }

    #[test]
    fn new_wraps_rocksdb_open_failure() {
        // A regular file where RocksDB expects a directory: `DB::open_cf` must
        // fail, and that failure must surface as `Error::RocksDb`.
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("not_a_directory");
        std::fs::write(&path, b"not a rocksdb database").unwrap();

        let Err(err) = DiskStorage::new(&path) else {
            panic!("opening a non-directory path must fail");
        };
        assert!(
            matches!(err, Error::RocksDb(_)),
            "expected Error::RocksDb, got {err}"
        );
    }
}
