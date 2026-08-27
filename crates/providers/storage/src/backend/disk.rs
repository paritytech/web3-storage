// SPDX-License-Identifier: Apache-2.0

//! Disk-based persistent storage backend using RocksDB.
//!
//! This provides the same interface as the in-memory storage but persists
//! all data to disk for production use.

use super::{BucketInfo, BucketStats, BucketSummary, StorageBackend, StoredNode};
use crate::error::Error;
use crate::nonce::NonceStore;
use codec::Encode;
use rocksdb::{Options, DB};
use sp_core::H256;
use std::path::Path;
use std::sync::{Arc, Mutex};
use storage_primitives::{blake2_256, BucketId, MmrLeaf};

/// Column families for organizing data
const CF_NODES: &str = "nodes";
const CF_BUCKETS: &str = "buckets";
const CF_ROOT_TO_BUCKET: &str = "root_to_bucket";
/// Small metadata values (e.g. the nonce counter highest sequence nonce).
const CF_METADATA: &str = "metadata";

/// RocksDB key for the persisted nonce counter highest sequence nonce.
const KEY_NONCE: &[u8] = b"nonce_counter";

/// Bucket state managed by this provider (serialized to disk).
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
struct BucketState {
    mmr_root: H256,
    start_seq: u64,
    leaves: Vec<MmrLeaf>,
    used_bytes: u64,
    max_bytes: u64,
}

impl BucketState {
    fn new(max_bytes: u64) -> Self {
        Self {
            mmr_root: H256::zero(),
            start_seq: 0,
            leaves: Vec::new(),
            used_bytes: 0,
            max_bytes,
        }
    }

    fn leaf_count(&self) -> u64 {
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
        let cf_names = vec![CF_NODES, CF_BUCKETS, CF_ROOT_TO_BUCKET, CF_METADATA];

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
    ///
    /// `Ok(None)` means the bucket genuinely does not exist. `Err` means the
    /// backend itself failed (a RocksDB read error, or a stored record that
    /// could not be decoded) - the bucket may still be there.
    fn get_bucket(&self, bucket_id: BucketId) -> Result<Option<BucketState>, Error> {
        let cf = self
            .db
            .cf_handle(CF_BUCKETS)
            .ok_or_else(|| Error::Storage("Buckets CF not found".to_string()))?;
        let key = bucket_id.to_le_bytes();
        let value = match self
            .db
            .get_cf(&cf, key)
            .map_err(|e| Error::Storage(e.to_string()))?
        {
            Some(v) => v,
            None => return Ok(None),
        };
        match bincode::deserialize(&value) {
            Ok(state) => Ok(Some(state)),
            Err(e) => Err(Error::Serialization(e.to_string())),
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
            .get_bucket(bucket_id)?
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
    ///
    /// `Ok(None)` means the node genuinely does not exist. `Err` means the
    /// backend itself failed - the node may still be there.
    pub fn get_node(&self, hash: &H256) -> Result<Option<StoredNode>, Error> {
        let cf = self
            .db
            .cf_handle(CF_NODES)
            .ok_or_else(|| Error::Storage("Nodes CF not found".to_string()))?;
        let key = hash.as_bytes();
        let value = match self
            .db
            .get_cf(&cf, key)
            .map_err(|e| Error::Storage(e.to_string()))?
        {
            Some(v) => v,
            None => return Ok(None),
        };
        match bincode::deserialize(&value) {
            Ok(node) => Ok(Some(node)),
            Err(e) => Err(Error::Serialization(e.to_string())),
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
            .get_bucket(bucket_id)?
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
            .get_bucket(bucket_id)?
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
            .get_bucket(bucket_id)?
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
            .get_bucket(bucket_id)?
            .ok_or(Error::BucketNotFound(bucket_id))?;

        let mut mmr = crate::mmr::Mmr::new();
        for leaf in &bucket.leaves {
            mmr.push(blake2_256(&leaf.encode()));
        }

        Ok((mmr.root(), mmr.peaks()))
    }

    /// Return a nonce store backed by this DB's metadata column family.
    ///
    /// The returned [`DiskNonceStore`] shares the open [`DB`] handle so there
    /// is no second DB to manage. Pass it to the negotiation nonce counter so
    /// the replay watermark survives restarts.
    pub fn nonce_store(&self) -> Arc<dyn NonceStore> {
        Arc::new(DiskNonceStore::new(self.db.clone()))
    }
}

impl StorageBackend for DiskStorage {
    fn init_bucket(&self, bucket_id: BucketId, max_bytes: u64) -> Result<(), Error> {
        self.init_bucket(bucket_id, max_bytes)
    }

    fn get_bucket(&self, bucket_id: BucketId) -> Result<Option<BucketInfo>, Error> {
        let state = DiskStorage::get_bucket(self, bucket_id)?;
        Ok(state.map(|state| BucketInfo {
            mmr_root: state.mmr_root,
            start_seq: state.start_seq,
            leaf_count: state.leaf_count(),
        }))
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

    fn get_node(&self, hash: &H256) -> Result<Option<StoredNode>, Error> {
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

// ─── NonceStore ───────────────────────────────────────────────────────────────

/// Monotonic nonce-counter persistence
///
/// Holds a shared reference to the open DB handle (same instance as
/// [`DiskStorage`]). All writes are monotonic: a call with a value lower than
/// the currently-persisted highest sequence nonce is silently ignored.
///
/// # Durability guarantee
///
/// Writes use default `WriteOptions` (`sync = false`): RocksDB appends to the
/// in-memory WAL but does **not** fsync to disk. This guarantees that the
/// highest sequence nonce survives a **clean process restart** (OS flushes the page
/// cache on normal shutdown). It does **not** guarantee survival of a
/// power-loss or kernel panic. In the latter case the last persisted nonce
/// may be lost; the counter falls back to `max(chain_hsn + 1, 1)` as it did
/// before this persistence layer was added, which is still safe — the chain's
/// replay window rejects any duplicate redemption.
pub struct DiskNonceStore {
    db: Arc<DB>,
    /// Monotonicity guard: always holds the highest value written so far,
    /// so concurrent `persist` calls can cheaply skip stale lower writes.
    watermark: Mutex<u64>,
}

impl DiskNonceStore {
    pub fn new(db: Arc<DB>) -> Self {
        // Initialize the in-memory watermark from the DB so we're consistent
        // from the first call to persist() even if load() is never called.
        let initial = Self::read_from_db(&db).unwrap_or(0);
        Self {
            db,
            watermark: Mutex::new(initial),
        }
    }

    fn read_from_db(db: &DB) -> Option<u64> {
        let cf = db.cf_handle(CF_METADATA)?;
        let bytes = db.get_cf(&cf, KEY_NONCE).ok()??;
        bytes.try_into().ok().map(u64::from_le_bytes)
    }
}

impl NonceStore for DiskNonceStore {
    fn load(&self) -> Option<u64> {
        Self::read_from_db(&self.db)
    }

    fn persist(&self, value: u64) {
        let mut wm = match self.watermark.lock() {
            Ok(g) => g,
            Err(e) => {
                tracing::warn!("nonce persist: watermark lock poisoned: {e}");
                return;
            }
        };
        if value <= *wm {
            return; // monotonic: ignore stale / lower values
        }
        let cf = match self.db.cf_handle(CF_METADATA) {
            Some(cf) => cf,
            None => {
                tracing::warn!("nonce persist: metadata CF not found");
                return;
            }
        };
        if let Err(e) = self.db.put_cf(&cf, KEY_NONCE, value.to_le_bytes()) {
            tracing::error!("nonce persist: RocksDB write failed: {e}");
            return;
        }
        *wm = value;
    }

    fn reset(&self) {
        let mut wm = match self.watermark.lock() {
            Ok(g) => g,
            Err(e) => {
                tracing::warn!("nonce reset: watermark lock poisoned: {e}");
                return;
            }
        };
        let cf = match self.db.cf_handle(CF_METADATA) {
            Some(cf) => cf,
            None => {
                tracing::warn!("nonce reset: metadata CF not found");
                return;
            }
        };
        if let Err(e) = self.db.delete_cf(&cf, KEY_NONCE) {
            tracing::error!("nonce reset: RocksDB delete failed: {e}");
            return;
        }
        *wm = 0;
        tracing::info!("nonce reset: persisted highest sequence nonce cleared on deregister");
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[test]
    fn nonce_store_persist_and_load_round_trip() {
        let dir = TempDir::new().unwrap();
        // Persist a value, then drop the storage handle (closing RocksDB).
        {
            let storage = DiskStorage::new(dir.path()).unwrap();
            let store = storage.nonce_store();
            assert!(store.load().is_none(), "fresh DB has no persisted nonce");
            store.persist(42);
            assert_eq!(store.load(), Some(42));
        }
        // Reopen: value must survive the DB close/reopen cycle.
        {
            let storage = DiskStorage::new(dir.path()).unwrap();
            let store = storage.nonce_store();
            assert_eq!(
                store.load(),
                Some(42),
                "persisted value must survive DB reopen"
            );
        }
    }

    #[test]
    fn nonce_store_persist_is_monotonic() {
        let dir = TempDir::new().unwrap();
        let storage = DiskStorage::new(dir.path()).unwrap();
        let store = storage.nonce_store();
        store.persist(50);
        assert_eq!(store.load(), Some(50));
        // A lower value must not regress the stored highest sequence nonce.
        store.persist(10);
        assert_eq!(
            store.load(),
            Some(50),
            "lower value must not regress the persisted mark"
        );
        // A higher value must advance it.
        store.persist(51);
        assert_eq!(store.load(), Some(51));
    }

    #[test]
    fn nonce_store_reset_clears_persisted_value() {
        let dir = TempDir::new().unwrap();
        let storage = DiskStorage::new(dir.path()).unwrap();
        let store = storage.nonce_store();
        store.persist(100);
        assert_eq!(store.load(), Some(100));

        // Reset: value must be gone from the DB.
        store.reset();
        assert!(
            store.load().is_none(),
            "reset must clear the persisted mark"
        );

        // Persist at a low value must now succeed (watermark was zeroed by reset).
        store.persist(2);
        assert_eq!(store.load(), Some(2));
    }

    #[test]
    fn nonce_store_reset_clears_across_reopen() {
        let dir = TempDir::new().unwrap();
        {
            let storage = DiskStorage::new(dir.path()).unwrap();
            let store = storage.nonce_store();
            store.persist(50);
            store.reset();
            // Immediately after reset, load is None.
            assert!(store.load().is_none());
        }
        // After reopen, the reset must have been flushed to disk (key deleted).
        {
            let storage = DiskStorage::new(dir.path()).unwrap();
            let store = storage.nonce_store();
            assert!(
                store.load().is_none(),
                "reset must persist across DB reopen"
            );
        }
    }

    /// Overwrite a node's raw record with bytes that can't be decoded,
    /// standing in for a disk-level corruption (partial write, bit flip)
    /// rather than the node simply never having been written.
    fn corrupt_node(storage: &DiskStorage, hash: H256) {
        let cf = storage.db.cf_handle(CF_NODES).unwrap();
        storage
            .db
            .put_cf(&cf, hash.as_bytes(), b"not a valid bincode record")
            .unwrap();
    }

    /// Overwrite a bucket's raw record with bytes that can't be decoded,
    /// same standing-in as [`corrupt_node`] but for `CF_BUCKETS`.
    fn corrupt_bucket(storage: &DiskStorage, bucket_id: BucketId) {
        let cf = storage.db.cf_handle(CF_BUCKETS).unwrap();
        storage
            .db
            .put_cf(&cf, bucket_id.to_le_bytes(), b"not a valid bincode record")
            .unwrap();
    }

    #[test]
    fn get_mmr_proof_for_a_bucket_that_was_never_created_reports_absence() {
        // Stands for: the client asked about a bucket id that genuinely
        // does not exist.
        let dir = TempDir::new().unwrap();
        let storage = DiskStorage::new(dir.path()).unwrap();

        let err = storage.get_mmr_proof(42, 0).unwrap_err();
        assert!(matches!(err, Error::BucketNotFound(42)));
    }

    #[test]
    fn get_mmr_proof_for_a_corrupt_bucket_record_reports_a_backend_failure_not_absence() {
        // Stands for: an operator's disk has a corrupted record (partial
        // write, bit flip) for a bucket that really was created. This must
        // not be reported the same way as "bucket does not exist" - that
        // would tell a challenge responder its own committed data is gone.
        let dir = TempDir::new().unwrap();
        let storage = DiskStorage::new(dir.path()).unwrap();
        storage.init_bucket(7, 1024 * 1024).unwrap();

        corrupt_bucket(&storage, 7);

        let err = storage.get_mmr_proof(7, 0).unwrap_err();
        assert!(
            matches!(err, Error::Serialization(_)),
            "expected Serialization, got {err:?}"
        );
    }

    #[test]
    fn get_chunk_at_index_past_the_end_of_a_healthy_root_reports_absence() {
        // Stands for: the client asked for a chunk index that was never
        // part of this data root.
        let dir = TempDir::new().unwrap();
        let storage = DiskStorage::new(dir.path()).unwrap();
        storage.init_bucket(1, 1024 * 1024).unwrap();

        let chunk_hash = blake2_256(b"only-chunk");
        storage
            .store_node(1, chunk_hash, b"only-chunk".to_vec(), None)
            .unwrap();

        let err = storage.get_chunk_at_index(chunk_hash, 5).unwrap_err();
        assert!(matches!(err, Error::NodeNotFound(_)));
    }

    #[test]
    fn get_chunk_at_index_with_a_corrupt_node_mid_traversal_reports_a_backend_failure_not_a_short_list(
    ) {
        // Stands for: an operator's disk has a corrupted record for one
        // chunk of a multi-chunk file. Before this test existed, a failed
        // read mid-traversal silently shortened the chunk list, so asking
        // for a chunk that is genuinely still there could trip the
        // "index past the end" guard and be reported as absent - the exact
        // false "you will be slashed" signal this whole cycle exists to
        // rule out.
        let dir = TempDir::new().unwrap();
        let storage = DiskStorage::new(dir.path()).unwrap();
        storage.init_bucket(1, 1024 * 1024).unwrap();

        let c0 = blake2_256(b"chunk-0");
        let c1 = blake2_256(b"chunk-1");
        storage
            .store_node(1, c0, b"chunk-0".to_vec(), None)
            .unwrap();
        storage
            .store_node(1, c1, b"chunk-1".to_vec(), None)
            .unwrap();

        let root_data = b"two-chunk-root".to_vec();
        let root = blake2_256(&root_data);
        storage
            .store_node(1, root, root_data, Some(vec![c0, c1]))
            .unwrap();

        corrupt_node(&storage, c0);

        // Chunk index 1 (chunk-1) is healthy and was never actually removed,
        // but a traversal that silently skips the unreadable c0 would only
        // see one chunk and report index 1 as past the end.
        let err = storage.get_chunk_at_index(root, 1).unwrap_err();
        assert!(
            matches!(err, Error::Serialization(_)),
            "expected Serialization, got {err:?}"
        );
    }
}
