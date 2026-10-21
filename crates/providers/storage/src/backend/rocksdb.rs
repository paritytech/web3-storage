// SPDX-License-Identifier: Apache-2.0

//! RocksDB-backed [`StorageBackend`]: persists the [`types`] records to disk.
//!
//! Only the engine lives here - the record shapes it reads and writes are
//! defined in [`types`], which owns their on-disk encoding.
//!
//! [`types`]: super::types

use super::types::FORMAT_VERSION;
use super::{BucketInfo, BucketState, BucketStats, BucketSummary, ChunkTreeNode, StorageBackend};
use crate::error::Error;
use codec::{DecodeAll, Encode};
use rocksdb::{Options, DB};
use sp_core::H256;
use std::path::Path;
use std::sync::Arc;
use storage_primitives::{
    mmr::{self, MmrLeaf, MmrProof},
    BucketId,
};

/// Column families for organizing data
const CF_NODES: &str = "nodes";
const CF_BUCKETS: &str = "buckets";
const CF_ROOT_TO_BUCKET: &str = "root_to_bucket";
const CF_METADATA: &str = "metadata";
/// Every column family this engine opens.
const COLUMN_FAMILIES: [&str; 4] = [CF_NODES, CF_BUCKETS, CF_ROOT_TO_BUCKET, CF_METADATA];

/// RocksDB key for the persisted format version.
const KEY_FORMAT_VERSION: &[u8] = b"format_version";

/// Disk-based storage backend using RocksDB.
pub struct DiskStorage {
    db: Arc<DB>,
}

impl DiskStorage {
    /// Create a new disk storage instance.
    pub fn new(path: impl AsRef<Path>) -> Result<Self, Error> {
        let db = DB::open_cf(&Self::db_options(), path.as_ref(), COLUMN_FAMILIES)?;

        Self::check_format_version(&db)?;

        Ok(Self { db: Arc::new(db) })
    }

    /// Options this engine opens its database with.
    fn db_options() -> Options {
        let mut opts = Options::default();
        opts.create_if_missing(true);
        opts.create_missing_column_families(true);
        opts
    }

    /// Verify the on-disk format version, seeding it on a genuinely fresh
    /// database.
    ///
    /// A database with no recorded version is compatible only if every
    /// column family is empty; otherwise it predates format versioning and
    /// this build's decoders cannot be trusted to read it. A recorded version
    /// that disagrees with [`FORMAT_VERSION`] always fails - silently reading
    /// its buckets as missing would look like data loss, not a format error.
    fn check_format_version(db: &DB) -> Result<(), Error> {
        let cf = db
            .cf_handle(CF_METADATA)
            .ok_or(Error::ColumnFamilyMissing(CF_METADATA))?;

        match db.get_cf(&cf, KEY_FORMAT_VERSION)? {
            Some(raw) => {
                let stored = u32::from_le_bytes(raw.as_slice().try_into().map_err(|_| {
                    Error::IncompatibleFormat("corrupt format_version value".to_string())
                })?);
                if stored != FORMAT_VERSION {
                    return Err(Error::IncompatibleFormat(format!(
                        "database format version {stored} is incompatible with this build \
                         (expects {FORMAT_VERSION}); a migration is required"
                    )));
                }
                Ok(())
            }
            None if Self::is_empty(db)? => {
                Ok(db.put_cf(&cf, KEY_FORMAT_VERSION, FORMAT_VERSION.to_le_bytes())?)
            }
            None => Err(Error::IncompatibleFormat(
                "database has data but no recorded format version - it predates format \
                 versioning and cannot be safely opened by this build"
                    .to_string(),
            )),
        }
    }

    /// Whether every column family this engine uses is empty.
    fn is_empty(db: &DB) -> Result<bool, Error> {
        for cf_name in COLUMN_FAMILIES {
            let cf = db
                .cf_handle(cf_name)
                .ok_or(Error::ColumnFamilyMissing(cf_name))?;
            if db
                .iterator_cf(&cf, rocksdb::IteratorMode::Start)
                .next()
                .transpose()?
                .is_some()
            {
                return Ok(false);
            }
        }
        Ok(true)
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
            mmr_root: format!("{:#x}", state.mmr_root),
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

    /// Store a node under the hash the caller claims for it.
    pub fn store_node(
        &self,
        bucket_id: BucketId,
        expected_hash: H256,
        node: ChunkTreeNode,
    ) -> Result<(), Error> {
        // Verify hash: derived from the children for an internal node, so a
        // node whose children don't match its claimed identity is rejected
        // here rather than trusted.
        let actual_hash = node.hash();
        if actual_hash != expected_hash {
            return Err(Error::InvalidHash {
                expected: format!("{:#x}", expected_hash),
                actual: format!("{:#x}", actual_hash),
            });
        }

        // If internal node, verify children exist
        if let ChunkTreeNode::Internal { left, right } = &node {
            let cf_nodes = self
                .db
                .cf_handle(CF_NODES)
                .ok_or(Error::ColumnFamilyMissing(CF_NODES))?;

            let missing: Vec<String> = [left, right]
                .into_iter()
                .filter(|h| {
                    self.db
                        .get_cf(&cf_nodes, h.as_bytes())
                        .ok()
                        .flatten()
                        .is_none()
                })
                .map(|h| format!("{:#x}", h))
                .collect();

            if !missing.is_empty() {
                return Err(Error::ChildrenMissing(missing));
            }
        }

        // Check quota.
        let mut bucket = self
            .get_bucket(bucket_id)
            .ok_or(Error::BucketNotFound(bucket_id))?;

        let charge = node.size();

        let new_size = bucket.used_bytes.saturating_add(charge);
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
            let value = node.encode();

            self.db.put_cf(&cf_nodes, key, &value)?;

            // Update quota
            bucket.used_bytes = bucket.used_bytes.saturating_add(charge);
            self.update_bucket(bucket_id, &bucket)?;
        }

        Ok(())
    }

    /// Get a node by hash.
    pub fn get_node(&self, hash: &H256) -> Option<ChunkTreeNode> {
        let cf = self.db.cf_handle(CF_NODES)?;
        let key = hash.as_bytes();
        let value = self.db.get_cf(&cf, key).ok()??;
        match ChunkTreeNode::decode_all(&mut &value[..]) {
            Ok(node) => Some(node),
            Err(e) => {
                tracing::warn!(hash = %format!("{:#x}", hash), error = %e, "Failed to deserialize node");
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
    ) -> Result<super::CommitOutcome, Error> {
        // Verify all roots exist
        let cf_nodes = self
            .db
            .cf_handle(CF_NODES)
            .ok_or(Error::ColumnFamilyMissing(CF_NODES))?;

        for root in &data_roots {
            let key = root.as_bytes();
            if self.db.get_cf(&cf_nodes, key)?.is_none() {
                return Err(Error::RootNotFound(format!("{:#x}", root)));
            }
        }

        // Reject non-canonical roots before the MMR changes.
        let data_sizes = data_roots
            .iter()
            .map(|root| self.canonical_data_size(*root))
            .collect::<Result<Vec<_>, _>>()?;

        // Get bucket and update MMR
        let mut bucket = self
            .get_bucket(bucket_id)
            .ok_or(Error::BucketNotFound(bucket_id))?;

        let start_seq = bucket.start_seq;
        let mut leaf_indices = Vec::new();

        // Add new leaves
        let start_index = bucket.leaves.len() as u64;
        for (i, (data_root, data_size)) in data_roots.iter().zip(data_sizes).enumerate() {
            leaf_indices.push(start_index + i as u64);

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
            bucket.leaves.push(leaf);
        }

        bucket.mmr_root = mmr::root(&bucket.leaves)?;

        // Update bucket
        self.update_bucket(bucket_id, &bucket)?;

        // leaf_count is derived from the same in-scope `bucket` as mmr_root, so the
        // returned (mmr_root, leaf_count) pair is self-consistent.
        Ok(super::CommitOutcome {
            mmr_root: bucket.mmr_root,
            start_seq,
            leaf_count: bucket.leaf_count(),
            leaf_indices,
        })
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

            bucket.mmr_root = mmr::root(&bucket.leaves)?;

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
        Ok(leaf_proof(&bucket.leaves, leaf_index)?.1)
    }

    /// See [`StorageBackend::get_mmr_proof_for_commitment`].
    pub fn get_mmr_proof_for_commitment(
        &self,
        bucket_id: BucketId,
        commitment: &storage_primitives::Commitment,
        leaf_index: u64,
    ) -> Result<storage_primitives::MmrProof, Error> {
        let bucket = self
            .get_bucket(bucket_id)
            .ok_or(Error::BucketNotFound(bucket_id))?;

        if leaf_index >= commitment.leaf_count {
            return Err(Error::CommitmentNotReproducible(format!(
                "leaf_{leaf_index} outside commitment ({} leaves)",
                commitment.leaf_count
            )));
        }

        // Rebase the commitment window onto the local leaf vector, which
        // covers [bucket.start_seq, bucket.start_seq + leaves.len()).
        if commitment.start_seq < bucket.start_seq {
            return Err(Error::CommitmentNotReproducible(format!(
                "leaves from seq {} pruned (local start_seq {})",
                commitment.start_seq, bucket.start_seq
            )));
        }
        let offset = (commitment.start_seq - bucket.start_seq) as usize;
        let end = offset.saturating_add(commitment.leaf_count as usize);
        if end > bucket.leaves.len() {
            return Err(Error::CommitmentNotReproducible(format!(
                "commitment range {}+{} exceeds local leaves (local end {})",
                commitment.start_seq,
                commitment.leaf_count,
                bucket.start_seq.saturating_add(bucket.leaf_count())
            )));
        }

        let (root, proof) = leaf_proof(&bucket.leaves[offset..end], leaf_index)?;
        // The rebuilt prefix must reproduce the cited root; a mismatch means
        // the local history diverged from what was signed, and any proof from
        // it would fail on-chain verification anyway.
        if root != commitment.mmr_root {
            return Err(Error::CommitmentNotReproducible(format!(
                "commitment_root {:#x} not reproducible from local leaves",
                commitment.mmr_root
            )));
        }
        Ok(proof)
    }

    /// Get MMR peaks.
    pub fn get_mmr_peaks(&self, bucket_id: BucketId) -> Result<(H256, Vec<H256>), Error> {
        let bucket = self
            .get_bucket(bucket_id)
            .ok_or(Error::BucketNotFound(bucket_id))?;

        Ok(mmr::peaks(&bucket.leaves)?)
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
        node: ChunkTreeNode,
    ) -> Result<(), Error> {
        self.store_node(bucket_id, expected_hash, node)
    }

    fn get_node(&self, hash: &H256) -> Option<ChunkTreeNode> {
        self.get_node(hash)
    }

    fn check_exists(&self, bucket_id: BucketId, hashes: &[H256]) -> (Vec<H256>, Vec<H256>) {
        self.check_exists(bucket_id, hashes)
    }

    fn commit(
        &self,
        bucket_id: BucketId,
        data_roots: Vec<H256>,
    ) -> Result<super::CommitOutcome, Error> {
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

    fn get_mmr_proof_for_commitment(
        &self,
        bucket_id: BucketId,
        commitment: &storage_primitives::Commitment,
        leaf_index: u64,
    ) -> Result<storage_primitives::MmrProof, Error> {
        self.get_mmr_proof_for_commitment(bucket_id, commitment, leaf_index)
    }

    fn get_mmr_peaks(&self, bucket_id: BucketId) -> Result<(H256, Vec<H256>), Error> {
        self.get_mmr_peaks(bucket_id)
    }
}

/// Root of the MMR over `leaves` and the proof for the leaf at `leaf_index`.
fn leaf_proof(leaves: &[MmrLeaf], leaf_index: u64) -> Result<(H256, MmrProof), Error> {
    mmr::proof(leaves, leaf_index)?.ok_or_else(|| Error::NodeNotFound(format!("leaf_{leaf_index}")))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::backend::build_merkle_tree;
    use storage_primitives::{hash_leaf, DEFAULT_CHUNK_SIZE};
    use tempfile::TempDir;

    /// Open the database at `path` directly, bypassing `DiskStorage::new` and
    /// its format-version check.
    fn open_raw(path: &Path) -> DB {
        DB::open_cf(&DiskStorage::db_options(), path, COLUMN_FAMILIES).unwrap()
    }

    /// A database with bucket 1 of `quota` bytes.
    fn storage_with_bucket(quota: u64) -> (DiskStorage, TempDir) {
        let dir = TempDir::new().unwrap();
        let storage = DiskStorage::new(dir.path()).unwrap();
        storage.init_bucket(1, quota).unwrap();
        (storage, dir)
    }

    /// Pins the raw keys and values this engine writes. A failure here means
    /// existing provider databases can no longer be read - bump
    /// [`FORMAT_VERSION`].
    ///
    /// The record encodings themselves are pinned in [`super::types`].
    #[test]
    fn on_disk_bytes() {
        // Raw keys and values as written through the public API.
        assert_eq!(
            COLUMN_FAMILIES,
            ["nodes", "buckets", "root_to_bucket", "metadata"],
            "column-family names locate every record on disk",
        );

        let dir = TempDir::new().unwrap();
        let storage = DiskStorage::new(dir.path()).unwrap();

        // CF_BUCKETS: key = bucket_id as u64 little-endian, value = SCALE(BucketState).
        let bucket_id: BucketId = 0x0102030405060708;
        storage.init_bucket(bucket_id, 1_000).unwrap();
        let cf = storage.db.cf_handle(CF_BUCKETS).unwrap();
        let key = [8u8, 7, 6, 5, 4, 3, 2, 1];
        let raw = storage
            .db
            .get_cf(&cf, key)
            .unwrap()
            .expect("bucket must be stored under the little-endian bucket_id key");
        assert_eq!(
            array_bytes::bytes2hex("", &raw),
            // BucketState::new(1_000): zero root, no leaves, max_bytes = 1_000
            "0000000000000000000000000000000000000000000000000000000000000000\
             0000000000000000\
             00\
             0000000000000000\
             e803000000000000"
        );

        // CF_NODES: key = hash_leaf(data), value = SCALE(ChunkTreeNode).
        let data = vec![1u8, 2, 3, 4, 5];
        let chunk_hash = hash_leaf(&data);
        storage
            .store_node(bucket_id, chunk_hash, ChunkTreeNode::Chunk(data))
            .unwrap();
        let cf = storage.db.cf_handle(CF_NODES).unwrap();
        let raw = storage
            .db
            .get_cf(&cf, chunk_hash.as_bytes())
            .unwrap()
            .unwrap();
        // variant 0 (Chunk), compact length 5, then the bytes
        assert_eq!(array_bytes::bytes2hex("", &raw), "00140102030405");

        // CF_NODES: an internal node's value is 65 B: the variant tag plus
        // the two child hashes.
        let other_hash = hash_leaf(&[9u8]);
        storage
            .store_node(bucket_id, other_hash, ChunkTreeNode::Chunk(vec![9]))
            .unwrap();
        let internal_hash = storage_primitives::hash_children(chunk_hash, other_hash);
        storage
            .store_node(
                bucket_id,
                internal_hash,
                ChunkTreeNode::Internal {
                    left: chunk_hash,
                    right: other_hash,
                },
            )
            .unwrap();
        let raw = storage
            .db
            .get_cf(&cf, internal_hash.as_bytes())
            .unwrap()
            .unwrap();
        assert_eq!(
            array_bytes::bytes2hex("", &raw),
            format!(
                "01{}{}",
                array_bytes::bytes2hex("", chunk_hash),
                array_bytes::bytes2hex("", other_hash)
            )
        );
    }

    const BUCKET: BucketId = 1;

    fn store_chunk_of_len(storage: &DiskStorage, byte: u8, len: usize) -> H256 {
        let data = vec![byte; len];
        let hash = hash_leaf(&data);
        storage
            .store_node(BUCKET, hash, ChunkTreeNode::Chunk(data))
            .unwrap();
        hash
    }

    fn store_chunk(storage: &DiskStorage, byte: u8) -> H256 {
        store_chunk_of_len(storage, byte, DEFAULT_CHUNK_SIZE as usize)
    }

    fn store_parent(storage: &DiskStorage, left: H256, right: H256) -> H256 {
        let hash = storage_primitives::hash_children(left, right);
        storage
            .store_node(BUCKET, hash, ChunkTreeNode::Internal { left, right })
            .unwrap();
        hash
    }

    fn assert_rejected_and_bucket_unchanged(storage: &DiskStorage, root: H256) {
        let before = storage.get_bucket(BUCKET).unwrap();
        let err = storage.commit(BUCKET, vec![root]).unwrap_err();
        assert!(matches!(err, Error::NonCanonicalTree(_)), "got {err:?}");
        let after = storage.get_bucket(BUCKET).unwrap();
        assert_eq!(after.leaves, before.leaves);
        assert_eq!(after.mmr_root, before.mmr_root);
    }

    /// Room for a few chunks of `DEFAULT_CHUNK_SIZE`.
    const COMMIT_QUOTA: u64 = 1_000_000;

    #[test]
    fn commit_rejects_right_leaning_tree() {
        let (storage, _dir) = storage_with_bucket(COMMIT_QUOTA);
        let [c0, c1, c2] = [1, 2, 3].map(|b| store_chunk(&storage, b));
        let right = store_parent(&storage, c1, c2);
        let root = store_parent(&storage, c0, right);
        assert_rejected_and_bucket_unchanged(&storage, root);
    }

    #[test]
    fn commit_accepts_tree_with_the_odd_chunk_moved_up_and_single_leaf() {
        let (storage, _dir) = storage_with_bucket(COMMIT_QUOTA);
        let [c0, c1, c2] = [1, 2, 3].map(|b| store_chunk(&storage, b));
        let root = build_merkle_tree(&storage, BUCKET, &[c0, c1, c2]).unwrap();
        assert_eq!(
            root,
            storage_primitives::hash_children(storage_primitives::hash_children(c0, c1), c2)
        );
        storage.commit(BUCKET, vec![root, c0]).unwrap();
        let leaves = storage.get_bucket(BUCKET).unwrap().leaves;
        assert_eq!(leaves.len(), 2);
        assert_eq!(leaves[0].data_size, 3 * DEFAULT_CHUNK_SIZE as u64);
    }

    #[test]
    fn commit_accepts_a_short_last_chunk() {
        let (storage, _dir) = storage_with_bucket(COMMIT_QUOTA);
        let [c0, c1] = [1, 2].map(|b| store_chunk(&storage, b));
        let c2 = store_chunk_of_len(&storage, 3, 10);
        let root = build_merkle_tree(&storage, BUCKET, &[c0, c1, c2]).unwrap();
        storage.commit(BUCKET, vec![root]).unwrap();
        let leaves = storage.get_bucket(BUCKET).unwrap().leaves;
        assert_eq!(leaves[0].data_size, 2 * DEFAULT_CHUNK_SIZE as u64 + 10);
    }

    #[test]
    fn commit_accepts_a_single_empty_chunk() {
        let (storage, _dir) = storage_with_bucket(COMMIT_QUOTA);
        let root = store_chunk_of_len(&storage, 0, 0);
        storage.commit(BUCKET, vec![root]).unwrap();
        assert_eq!(storage.get_bucket(BUCKET).unwrap().leaves[0].data_size, 0);
    }

    #[test]
    fn commit_rejects_a_short_chunk_that_is_not_last() {
        let (storage, _dir) = storage_with_bucket(COMMIT_QUOTA);
        let c0 = store_chunk_of_len(&storage, 1, 10);
        let c1 = store_chunk(&storage, 2);
        let root = store_parent(&storage, c0, c1);
        assert_rejected_and_bucket_unchanged(&storage, root);
    }

    #[test]
    fn commit_rejects_an_empty_last_chunk_of_several() {
        let (storage, _dir) = storage_with_bucket(COMMIT_QUOTA);
        let c0 = store_chunk(&storage, 1);
        let c1 = store_chunk_of_len(&storage, 2, 0);
        let root = store_parent(&storage, c0, c1);
        assert_rejected_and_bucket_unchanged(&storage, root);
    }

    #[test]
    fn store_node_rejects_internal_node_whose_children_disagree_with_expected_hash() {
        let (storage, _dir) = storage_with_bucket(1_000);
        let bucket_id: BucketId = 1;

        let left = H256::zero();
        let right = H256::zero();
        let claimed_hash = H256::repeat_byte(0xaa);
        assert_ne!(claimed_hash, storage_primitives::hash_children(left, right));

        let err = storage
            .store_node(
                bucket_id,
                claimed_hash,
                ChunkTreeNode::Internal { left, right },
            )
            .unwrap_err();
        assert!(matches!(err, Error::InvalidHash { .. }));
    }

    #[test]
    fn store_node_rejects_missing_child() {
        let (storage, _dir) = storage_with_bucket(1_000);
        let bucket_id: BucketId = 1;

        let missing_child = H256::repeat_byte(0x42);
        let hash = storage_primitives::hash_children(missing_child, H256::zero());
        let err = storage
            .store_node(
                bucket_id,
                hash,
                ChunkTreeNode::Internal {
                    left: missing_child,
                    right: H256::zero(),
                },
            )
            .unwrap_err();
        assert!(matches!(err, Error::ChildrenMissing(_)));
    }

    #[test]
    fn internal_node_charges_64_bytes() {
        let (storage, _dir) = storage_with_bucket(COMMIT_QUOTA);
        let [left, right] = [1, 2].map(|b| store_chunk_of_len(&storage, b, 10));
        let used = storage.get_bucket(BUCKET).unwrap().used_bytes;

        store_parent(&storage, left, right);

        let bucket = storage.get_bucket(BUCKET).unwrap();
        assert_eq!(bucket.used_bytes, used + 64);
    }

    #[test]
    fn store_node_rejects_a_zero_child() {
        let (storage, _dir) = storage_with_bucket(COMMIT_QUOTA);
        let c0 = store_chunk(&storage, 1);
        let hash = storage_primitives::hash_children(c0, H256::zero());
        let err = storage
            .store_node(
                BUCKET,
                hash,
                ChunkTreeNode::Internal {
                    left: c0,
                    right: H256::zero(),
                },
            )
            .unwrap_err();
        assert!(matches!(err, Error::ChildrenMissing(_)), "got {err:?}");
    }

    #[test]
    fn get_node_round_trips_both_variants() {
        let (storage, _dir) = storage_with_bucket(1_000);
        let bucket_id: BucketId = 1;

        let chunk_data = vec![7u8, 8, 9];
        let chunk_hash = hash_leaf(&chunk_data);
        storage
            .store_node(
                bucket_id,
                chunk_hash,
                ChunkTreeNode::Chunk(chunk_data.clone()),
            )
            .unwrap();
        assert_eq!(
            storage.get_node(&chunk_hash),
            Some(ChunkTreeNode::Chunk(chunk_data))
        );

        let left_data = vec![1u8];
        let left = hash_leaf(&left_data);
        storage
            .store_node(bucket_id, left, ChunkTreeNode::Chunk(left_data))
            .unwrap();
        let right_data = vec![2u8];
        let right = hash_leaf(&right_data);
        storage
            .store_node(bucket_id, right, ChunkTreeNode::Chunk(right_data))
            .unwrap();

        let internal_hash = storage_primitives::hash_children(left, right);
        storage
            .store_node(
                bucket_id,
                internal_hash,
                ChunkTreeNode::Internal { left, right },
            )
            .unwrap();
        assert_eq!(
            storage.get_node(&internal_hash),
            Some(ChunkTreeNode::Internal { left, right })
        );
    }

    #[test]
    fn a_fresh_database_records_the_current_format_version() {
        let dir = TempDir::new().unwrap();
        let storage = DiskStorage::new(dir.path()).unwrap();

        let cf = storage.db.cf_handle(CF_METADATA).unwrap();
        let raw = storage
            .db
            .get_cf(&cf, KEY_FORMAT_VERSION)
            .unwrap()
            .expect("a fresh database must record a format version");
        assert_eq!(
            u32::from_le_bytes(raw.as_slice().try_into().unwrap()),
            FORMAT_VERSION
        );
    }

    #[test]
    fn reopening_a_database_this_build_created_succeeds() {
        let dir = TempDir::new().unwrap();
        DiskStorage::new(dir.path()).unwrap();
        DiskStorage::new(dir.path()).expect("reopening this build's own database must succeed");
    }

    #[test]
    fn opening_a_database_with_a_different_format_version_fails() {
        let dir = TempDir::new().unwrap();
        DiskStorage::new(dir.path()).unwrap();

        // Corrupt the recorded version directly, bypassing DiskStorage::new.
        let db = open_raw(dir.path());
        let cf = db.cf_handle(CF_METADATA).unwrap();
        db.put_cf(&cf, KEY_FORMAT_VERSION, 9999u32.to_le_bytes())
            .unwrap();
        drop(db);

        assert!(matches!(
            DiskStorage::new(dir.path()),
            Err(Error::IncompatibleFormat(_))
        ));
    }

    /// Roots built by the provider equal the values the primitives and the
    /// TypeScript package assert for the same chunks.
    #[test]
    fn provider_roots_match_the_shared_hash_vectors() {
        let dir = TempDir::new().unwrap();
        let storage = DiskStorage::new(dir.path()).unwrap();
        storage.init_bucket(1, 1_000_000).unwrap();

        let mut leaves = Vec::new();
        for chunk in [b"a", b"b", b"c"] {
            let hash = hash_leaf(chunk);
            storage
                .store_node(1, hash, ChunkTreeNode::Chunk(chunk.to_vec()))
                .unwrap();
            leaves.push(hash);
        }

        let data_root = crate::backend::build_merkle_tree(&storage, 1, &leaves).unwrap();
        assert_eq!(
            data_root,
            "17321db51c1ef3ec1f77e271aa300b4e5c6091708bcba37e46025774a26142ee"
                .parse::<H256>()
                .unwrap()
        );

        // `commit` accepts only canonical chunk sizes, so the MMR vector
        // commits the single chunk `abc`.
        let abc = hash_leaf(b"abc");
        storage
            .store_node(1, abc, ChunkTreeNode::Chunk(b"abc".to_vec()))
            .unwrap();
        let outcome = storage.commit(1, vec![abc]).unwrap();
        assert_eq!(outcome.leaf_indices, vec![0]);
        assert_eq!(
            outcome.mmr_root,
            "5892aeee490d7378e4dcabfaf79053c3073ece6437f74c001c82a55de8ac977c"
                .parse::<H256>()
                .unwrap()
        );
    }

    #[test]
    fn opening_a_non_empty_pre_versioning_database_fails() {
        let dir = TempDir::new().unwrap();

        // Simulate a database written before format versioning existed: real
        // data, but no format_version key - bypassing DiskStorage::new so no
        // version ever gets recorded.
        let db = open_raw(dir.path());
        let cf = db.cf_handle(CF_BUCKETS).unwrap();
        db.put_cf(&cf, 1u64.to_le_bytes(), BucketState::new(1_000).encode())
            .unwrap();
        drop(db);

        assert!(matches!(
            DiskStorage::new(dir.path()),
            Err(Error::IncompatibleFormat(_))
        ));
    }

    /// Init a bucket and commit `n` distinct single-chunk leaves, one commit
    /// per leaf (sequences `0..n`).
    fn bucket_with_leaves(storage: &DiskStorage, bucket_id: BucketId, n: u8) {
        storage.init_bucket(bucket_id, u64::MAX).unwrap();
        for i in 0..n {
            let data = vec![i; 8];
            let hash = hash_leaf(&data);
            storage
                .store_node(bucket_id, hash, ChunkTreeNode::Chunk(data))
                .unwrap();
            storage.commit(bucket_id, vec![hash]).unwrap();
        }
    }

    #[test]
    fn proof_for_a_missing_leaf_is_node_not_found() {
        let dir = TempDir::new().unwrap();
        let storage = DiskStorage::new(dir.path()).unwrap();
        bucket_with_leaves(&storage, 1, 0);
        bucket_with_leaves(&storage, 2, 4);

        assert!(matches!(
            storage.get_mmr_proof(1, 0),
            Err(Error::NodeNotFound(_))
        ));
        assert!(matches!(
            storage.get_mmr_proof(2, 4),
            Err(Error::NodeNotFound(_))
        ));
        assert!(storage.get_mmr_proof(2, 3).is_ok());
    }

    #[test]
    fn proof_for_older_commitment_matches_cited_root() {
        let dir = TempDir::new().unwrap();
        let storage = DiskStorage::new(dir.path()).unwrap();
        storage.init_bucket(1, u64::MAX).unwrap();

        // Three commits; the challenge cites the root after the second.
        let mut roots = Vec::new();
        for i in 0..3u8 {
            let data = vec![i; 8];
            let hash = hash_leaf(&data);
            storage
                .store_node(1, hash, ChunkTreeNode::Chunk(data))
                .unwrap();
            let root = storage.commit(1, vec![hash]).unwrap().mmr_root;
            roots.push(root);
        }

        let cited = storage_primitives::Commitment {
            mmr_root: roots[1],
            start_seq: 0,
            leaf_count: 2,
        };
        let proof = storage.get_mmr_proof_for_commitment(1, &cited, 1).unwrap();
        assert!(storage_primitives::verify_mmr_proof(
            proof.clone(),
            1,
            &cited
        ));
        // The same proof must not satisfy a challenge citing the current
        // 3-leaf commitment: it was built for the two-leaf MMR.
        let current = storage_primitives::Commitment {
            mmr_root: roots[2],
            start_seq: 0,
            leaf_count: 3,
        };
        assert!(!storage_primitives::verify_mmr_proof(proof, 1, &current));

        // A leaf index outside the cited commitment is rejected even though
        // the bucket currently holds it.
        assert!(storage
            .get_mmr_proof_for_commitment(
                1,
                &storage_primitives::Commitment {
                    mmr_root: roots[1],
                    start_seq: 0,
                    leaf_count: 2,
                },
                2,
            )
            .is_err());

        // A cited (root, leaf_count) pair the local history cannot reproduce
        // is rejected: three leaves do not hash to the two-leaf root.
        assert!(storage
            .get_mmr_proof_for_commitment(
                1,
                &storage_primitives::Commitment {
                    mmr_root: roots[1],
                    start_seq: 0,
                    leaf_count: 3,
                },
                1,
            )
            .is_err());

        // A cited range extending past the locally held leaves is rejected
        // before any MMR work.
        assert!(storage
            .get_mmr_proof_for_commitment(
                1,
                &storage_primitives::Commitment {
                    mmr_root: roots[1],
                    start_seq: 0,
                    leaf_count: 99,
                },
                1,
            )
            .is_err());
    }

    #[test]
    fn proof_after_prune_rebases_leaf_index() {
        let dir = TempDir::new().unwrap();
        let storage = DiskStorage::new(dir.path()).unwrap();
        bucket_with_leaves(&storage, 1, 3);
        let expected = storage.get_bucket(1).unwrap().leaves[2].clone();

        let (post_prune_root, start_seq, _) = storage.delete_before(1, 1).unwrap();
        assert_eq!(start_seq, 1);

        // Challenge cites the post-prune commitment (start_seq 1); its
        // leaf_index 1 is global seq 2 — the third leaf ever committed.
        let cited = storage_primitives::Commitment {
            mmr_root: post_prune_root,
            start_seq: 1,
            leaf_count: 2,
        };
        let proof = storage.get_mmr_proof_for_commitment(1, &cited, 1).unwrap();
        assert_eq!(proof.leaf.data_root, expected.data_root);
        assert!(storage_primitives::verify_mmr_proof(proof, 1, &cited));
    }

    #[test]
    fn proof_for_pruned_commitment_errors() {
        let dir = TempDir::new().unwrap();
        let storage = DiskStorage::new(dir.path()).unwrap();
        bucket_with_leaves(&storage, 1, 2);
        let old_root = storage.get_bucket(1).unwrap().mmr_root;

        storage.delete_before(1, 2).unwrap();

        // The cited commitment starts below the local start_seq: its leaves
        // are gone (until a retention stash exists), so this must error.
        assert!(storage
            .get_mmr_proof_for_commitment(
                1,
                &storage_primitives::Commitment {
                    mmr_root: old_root,
                    start_seq: 0,
                    leaf_count: 2,
                },
                0,
            )
            .is_err());
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
