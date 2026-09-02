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
/// Per-node reference counts: how many committed leaves (live or stashed,
/// across all buckets) reach the node. Keyed like CF_NODES.
const CF_REFCOUNTS: &str = "refcounts";
/// Small metadata values (e.g. the nonce counter highest sequence nonce).
const CF_METADATA: &str = "metadata";

/// RocksDB key for the persisted nonce counter highest sequence nonce.
const KEY_NONCE: &[u8] = b"nonce_counter";

mod refcounts;

use refcounts::{decode_refcount, encode_refcount};

/// A contiguous run of leaves removed by `delete_before`, retained until the
/// on-chain liability for them has provably passed (then physically erased).
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
struct PrunedRange {
    /// Global sequence number of `leaves[0]`.
    first_seq: u64,
    /// The removed leaves, contiguous from `first_seq`.
    leaves: Vec<MmrLeaf>,
    /// The start_seq this prune advanced the bucket to.
    new_start_seq: u64,
}

/// Bucket state managed by this provider (serialized to disk).
///
/// Changing fields or their order changes the bincode encoding: existing
/// data directories become unreadable and must be wiped (no migration
/// machinery — introduce it if a deployment ever needs one).
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
struct BucketState {
    mmr_root: H256,
    start_seq: u64,
    leaves: Vec<MmrLeaf>,
    used_bytes: u64,
    max_bytes: u64,
    /// Pruned-but-not-yet-erased leaf ranges (the pending-erasure queue).
    pruned: Vec<PrunedRange>,
    /// Admin-signed deletion receipts keyed by `new_start_seq` (one per
    /// prune point), kept even after their ranges are erased — permanent
    /// evidence for the on-chain `Deleted` defense.
    deletion_receipts: std::collections::BTreeMap<u64, super::DeletionReceipt>,
    /// Set when the bucket was deleted on-chain (or the agreement ended).
    /// The bucket row is removed once `leaves` and `pruned` are both empty.
    condemned: bool,
}

impl BucketState {
    fn new(max_bytes: u64) -> Self {
        Self {
            mmr_root: H256::zero(),
            start_seq: 0,
            leaves: Vec::new(),
            used_bytes: 0,
            max_bytes,
            pruned: Vec::new(),
            deletion_receipts: std::collections::BTreeMap::new(),
            condemned: false,
        }
    }

    fn leaf_count(&self) -> u64 {
        self.leaves.len() as u64
    }
}

/// Disk-based storage backend using RocksDB.
pub struct DiskStorage {
    db: Arc<DB>,
    /// Serializes the read-modify-write paths (`init_bucket`, `store_node`,
    /// `commit`, `delete_before`): they read `BucketState`, mutate it, and
    /// write several keys, so two concurrent writers would lose updates
    /// (e.g. a `used_bytes` increment). Reads never take this lock —
    /// RocksDB handles concurrent reads itself.
    ///
    /// Deliberately one global lock: write volume is far below contention
    /// territory; switch to per-bucket locks if that ever changes.
    write_lock: parking_lot::Mutex<()>,
}

impl DiskStorage {
    /// Create a new disk storage instance.
    pub fn new(path: impl AsRef<Path>) -> Result<Self, Error> {
        let mut opts = Options::default();
        opts.create_if_missing(true);
        opts.create_missing_column_families(true);

        // Define column families
        let cf_names = vec![
            CF_NODES,
            CF_BUCKETS,
            CF_ROOT_TO_BUCKET,
            CF_REFCOUNTS,
            CF_METADATA,
        ];

        let db = DB::open_cf(&opts, path, &cf_names)
            .map_err(|e| Error::Storage(format!("Failed to open RocksDB: {e}")))?;

        let storage = Self {
            db: Arc::new(db),
            write_lock: parking_lot::Mutex::new(()),
        };
        Ok(storage)
    }

    /// Read a CF_REFCOUNTS entry: `(count, charged_bucket, size)`.
    fn refcount_entry(&self, hash: &H256) -> Result<Option<(u64, u64, u64)>, Error> {
        let cf = self
            .db
            .cf_handle(CF_REFCOUNTS)
            .ok_or_else(|| Error::Storage("Refcounts CF not found".to_string()))?;
        self.db
            .get_cf(&cf, hash.as_bytes())
            .map_err(|e| Error::Storage(e.to_string()))?
            .map(|v| decode_refcount(&v))
            .transpose()
    }

    /// Acquire the write lock serializing read-modify-write paths (the
    /// guard protects no in-memory state — everything lives in RocksDB).
    fn lock_writes(&self) -> parking_lot::MutexGuard<'_, ()> {
        self.write_lock.lock()
    }

    /// Initialize a bucket with the given quota.
    pub fn init_bucket(&self, bucket_id: BucketId, max_bytes: u64) -> Result<(), Error> {
        let _guard = self.lock_writes();
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
    fn get_bucket(&self, bucket_id: BucketId) -> Option<BucketState> {
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
            used_bytes: state.used_bytes,
            max_bytes: state.max_bytes,
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
        let _guard = self.lock_writes();

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

            // Node and quota update land atomically: a crash between two
            // separate puts would leak an uncharged node.
            bucket.used_bytes = bucket.used_bytes.saturating_add(data_len);
            let cf_buckets = self
                .db
                .cf_handle(CF_BUCKETS)
                .ok_or_else(|| Error::Storage("Buckets CF not found".to_string()))?;
            let bucket_value =
                bincode::serialize(&bucket).map_err(|e| Error::Serialization(e.to_string()))?;

            let cf_refcounts = self
                .db
                .cf_handle(CF_REFCOUNTS)
                .ok_or_else(|| Error::Storage("Refcounts CF not found".to_string()))?;

            let mut batch = rocksdb::WriteBatch::default();
            batch.put_cf(&cf_nodes, key, &value);
            batch.put_cf(&cf_buckets, bucket_id.to_le_bytes(), &bucket_value);
            // Charge record: count stays 0 until a commit references the
            // node; erasure credits `bucket_id`'s used_bytes by `data_len`.
            batch.put_cf(&cf_refcounts, key, encode_refcount(0, bucket_id, data_len));
            self.db
                .write(batch)
                .map_err(|e| Error::Storage(e.to_string()))?;
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
        let _guard = self.lock_writes();

        // Verify all roots exist (missing roots keep their dedicated error)
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

        // Walk each root once: verify the whole tree is present (committing
        // to a partially-uploaded tree would create liability the provider
        // cannot prove), measure its content size, and gather the refcount
        // increments this commit adds.
        let mut increments: std::collections::HashMap<H256, u64> = std::collections::HashMap::new();
        let mut node_sizes: std::collections::HashMap<H256, u64> = std::collections::HashMap::new();
        let mut root_sizes = Vec::with_capacity(data_roots.len());
        for root in &data_roots {
            let mut data_size = 0u64;
            self.try_walk_tree(*root, &mut |hash, node| {
                *increments.entry(hash).or_default() += 1;
                node_sizes.entry(hash).or_insert(node.data.len() as u64);
                if node.children.is_none() {
                    data_size = data_size.saturating_add(node.data.len() as u64);
                }
            })?;
            root_sizes.push(data_size);
        }

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

            let data_size = root_sizes[i];
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

        // Bucket row and refcount increments land in one atomic batch.
        let cf_buckets = self
            .db
            .cf_handle(CF_BUCKETS)
            .ok_or_else(|| Error::Storage("Buckets CF not found".to_string()))?;
        let cf_refcounts = self
            .db
            .cf_handle(CF_REFCOUNTS)
            .ok_or_else(|| Error::Storage("Refcounts CF not found".to_string()))?;

        let mut batch = rocksdb::WriteBatch::default();
        for (hash, n) in increments {
            // A missing entry means the node predates refcounting (or its
            // charge record was created by another bucket's migration skip);
            // charge attribution defaults to the committing bucket.
            let fallback_size = node_sizes.get(&hash).copied().unwrap_or(0);
            let (count, charged_bucket, size) =
                self.refcount_entry(&hash)?
                    .unwrap_or((0, bucket_id, fallback_size));
            batch.put_cf(
                &cf_refcounts,
                hash.as_bytes(),
                encode_refcount(count.saturating_add(n), charged_bucket, size),
            );
        }
        let bucket_value =
            bincode::serialize(&bucket).map_err(|e| Error::Serialization(e.to_string()))?;
        batch.put_cf(&cf_buckets, bucket_id.to_le_bytes(), &bucket_value);
        self.db
            .write(batch)
            .map_err(|e| Error::Storage(e.to_string()))?;

        Ok((bucket.mmr_root, start_seq, leaf_indices))
    }

    /// Delete data before a given sequence number.
    ///
    /// The removed leaves move into the bucket's retention stash — the
    /// provider must stay able to prove commitments covering them until an
    /// admin deletion receipt is held and the canonical checkpoint passed
    /// the range, at which point the GC calls
    /// [`erase_pruned_range`](Self::erase_pruned_range). Quota (`used_bytes`)
    /// is deliberately NOT credited here: it tracks disk actually consumed.
    pub fn delete_before(
        &self,
        bucket_id: BucketId,
        new_start_seq: u64,
    ) -> Result<(H256, u64, u64), Error> {
        let _guard = self.lock_writes();

        let mut bucket = self
            .get_bucket(bucket_id)
            .ok_or(Error::BucketNotFound(bucket_id))?;

        // start_seq can only advance, and no further than one past the last
        // leaf; a rewind or overshoot is a caller error, never a silent no-op.
        let end_seq = bucket.start_seq.saturating_add(bucket.leaf_count());
        if new_start_seq < bucket.start_seq || new_start_seq > end_seq {
            return Err(Error::InvalidStartSeq {
                requested: new_start_seq,
                current: bucket.start_seq,
                end: end_seq,
            });
        }

        // Move leaves before new_start_seq into the retention stash
        let to_remove = (new_start_seq - bucket.start_seq) as usize;
        if to_remove > 0 {
            let removed: Vec<MmrLeaf> = bucket.leaves.drain(0..to_remove).collect();
            bucket.pruned.push(PrunedRange {
                first_seq: bucket.start_seq,
                leaves: removed,
                new_start_seq,
            });
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

    /// Set/refresh the bucket quota learned from the chain agreement.
    pub fn set_bucket_quota(&self, bucket_id: BucketId, max_bytes: u64) -> Result<(), Error> {
        let _guard = self.lock_writes();
        let mut bucket = self
            .get_bucket(bucket_id)
            .ok_or(Error::BucketNotFound(bucket_id))?;
        if bucket.max_bytes != max_bytes {
            bucket.max_bytes = max_bytes;
            self.update_bucket(bucket_id, &bucket)?;
        }
        Ok(())
    }

    /// Pruned ranges awaiting physical erasure, oldest first.
    pub fn pruned_ranges(&self, bucket_id: BucketId) -> Vec<super::PrunedRangeInfo> {
        let Some(bucket) = self.get_bucket(bucket_id) else {
            return Vec::new();
        };
        bucket
            .pruned
            .iter()
            .map(|r| super::PrunedRangeInfo {
                first_seq: r.first_seq,
                end_seq: r.first_seq.saturating_add(r.leaves.len() as u64),
                new_start_seq: r.new_start_seq,
                has_receipt: bucket.deletion_receipts.contains_key(&r.new_start_seq),
            })
            .collect()
    }

    /// Whether the bucket was condemned (deleted on-chain / agreement gone).
    pub fn is_condemned(&self, bucket_id: BucketId) -> bool {
        self.get_bucket(bucket_id).is_some_and(|b| b.condemned)
    }

    /// Store an admin-signed deletion receipt for a stashed range (matched
    /// by `new_start_seq`); replaces a previous receipt for the same range.
    pub fn attach_deletion_receipt(
        &self,
        bucket_id: BucketId,
        receipt: super::DeletionReceipt,
    ) -> Result<(), Error> {
        let _guard = self.lock_writes();
        let mut bucket = self
            .get_bucket(bucket_id)
            .ok_or(Error::BucketNotFound(bucket_id))?;
        let covers_stashed_range = bucket
            .pruned
            .iter()
            .any(|r| r.new_start_seq == receipt.new_start_seq);
        if !covers_stashed_range {
            return Err(Error::InvalidStartSeq {
                requested: receipt.new_start_seq,
                current: bucket.start_seq,
                end: bucket.start_seq.saturating_add(bucket.leaf_count()),
            });
        }
        bucket
            .deletion_receipts
            .insert(receipt.new_start_seq, receipt);
        self.update_bucket(bucket_id, &bucket)
    }

    /// The stored receipt with the smallest `new_start_seq` strictly greater
    /// than `seq`, if any.
    pub fn deletion_receipt_covering(
        &self,
        bucket_id: BucketId,
        seq: u64,
    ) -> Option<super::DeletionReceipt> {
        use std::ops::Bound;
        self.get_bucket(bucket_id)?
            .deletion_receipts
            .range((Bound::Excluded(seq), Bound::Unbounded))
            .next()
            .map(|(_, r)| r.clone())
    }

    /// Bucket teardown, first half: stash all remaining leaves and mark the
    /// bucket condemned; `erase_pruned_range` later erases the stash and
    /// removes the row. If nothing is stored, the row is dropped right away.
    pub fn condemn_bucket(&self, bucket_id: BucketId) -> Result<(), Error> {
        let _guard = self.lock_writes();
        let Some(mut bucket) = self.get_bucket(bucket_id) else {
            return Ok(()); // already torn down
        };
        if bucket.condemned {
            return Ok(());
        }

        if !bucket.leaves.is_empty() {
            let first_seq = bucket.start_seq;
            let leaves = std::mem::take(&mut bucket.leaves);
            let new_start_seq = first_seq.saturating_add(leaves.len() as u64);
            bucket.pruned.push(PrunedRange {
                first_seq,
                leaves,
                new_start_seq,
            });
            bucket.start_seq = new_start_seq;
            bucket.mmr_root = crate::mmr::Mmr::new().root();
        }
        bucket.condemned = true;

        if bucket.pruned.is_empty() {
            // Nothing committed was ever stored: no liability, drop the row.
            let cf = self
                .db
                .cf_handle(CF_BUCKETS)
                .ok_or_else(|| Error::Storage("Buckets CF not found".to_string()))?;
            self.db
                .delete_cf(&cf, bucket_id.to_le_bytes())
                .map_err(|e| Error::Storage(e.to_string()))?;
            return Ok(());
        }
        self.update_bucket(bucket_id, &bucket)
    }

    /// Physically erase one stashed range. See the trait docs for the
    /// contract; checking that liability has passed is the caller's job.
    pub fn erase_pruned_range(
        &self,
        bucket_id: BucketId,
        first_seq: u64,
    ) -> Result<super::EraseOutcome, Error> {
        let _guard = self.lock_writes();
        let mut bucket = self
            .get_bucket(bucket_id)
            .ok_or(Error::BucketNotFound(bucket_id))?;
        let Some(pos) = bucket.pruned.iter().position(|r| r.first_seq == first_seq) else {
            return Ok(super::EraseOutcome::default()); // already erased
        };
        let range = bucket.pruned.remove(pos);

        // Gather refcount decrements: one per encounter along each leaf tree,
        // symmetric with commit's increments.
        let mut decrements: std::collections::HashMap<H256, u64> = std::collections::HashMap::new();
        for leaf in &range.leaves {
            let walk = self.try_walk_tree(leaf.data_root, &mut |hash, _| {
                *decrements.entry(hash).or_default() += 1;
            });
            if let Err(e) = walk {
                // Nodes reached before the failure genuinely lose this
                // leaf's reference; unreachable ones keep their count and
                // leak — the safe direction.
                tracing::warn!(
                    bucket_id,
                    error = %e,
                    "erase: leaf tree incomplete, applying partial decrements"
                );
            }
        }

        let cf_nodes = self
            .db
            .cf_handle(CF_NODES)
            .ok_or_else(|| Error::Storage("Nodes CF not found".to_string()))?;
        let cf_refcounts = self
            .db
            .cf_handle(CF_REFCOUNTS)
            .ok_or_else(|| Error::Storage("Refcounts CF not found".to_string()))?;
        let cf_buckets = self
            .db
            .cf_handle(CF_BUCKETS)
            .ok_or_else(|| Error::Storage("Buckets CF not found".to_string()))?;

        let mut batch = rocksdb::WriteBatch::default();
        let mut nodes_deleted = 0u64;
        // charged bucket -> bytes to credit back to its used_bytes
        let mut credits: std::collections::HashMap<u64, u64> = std::collections::HashMap::new();
        for (hash, n) in decrements {
            let Some((count, charged_bucket, size)) = self.refcount_entry(&hash)? else {
                tracing::warn!(
                    bucket_id,
                    hash = %format!("0x{}", hex::encode(hash.as_bytes())),
                    "erase: refcount entry missing, node kept"
                );
                continue;
            };
            let new_count = count.saturating_sub(n);
            if new_count == 0 {
                batch.delete_cf(&cf_nodes, hash.as_bytes());
                batch.delete_cf(&cf_refcounts, hash.as_bytes());
                nodes_deleted += 1;
                *credits.entry(charged_bucket).or_default() += size;
            } else {
                batch.put_cf(
                    &cf_refcounts,
                    hash.as_bytes(),
                    encode_refcount(new_count, charged_bucket, size),
                );
            }
        }
        let bytes_freed: u64 = credits.values().sum();

        // Apply quota credits; the erasing bucket's own row may be credited.
        let mut rows: std::collections::HashMap<u64, BucketState> =
            std::collections::HashMap::new();
        rows.insert(bucket_id, bucket);
        for (credited, bytes) in credits {
            if let std::collections::hash_map::Entry::Vacant(entry) = rows.entry(credited) {
                match DiskStorage::get_bucket(self, credited) {
                    Some(state) => {
                        entry.insert(state);
                    }
                    None => {
                        tracing::warn!(
                            credited,
                            bytes,
                            "erase: charged bucket no longer exists, credit dropped"
                        );
                        continue;
                    }
                }
            }
            if let Some(state) = rows.get_mut(&credited) {
                state.used_bytes = state.used_bytes.saturating_sub(bytes);
            }
        }
        for (id, state) in rows {
            let fully_gone = id == bucket_id
                && state.condemned
                && state.leaves.is_empty()
                && state.pruned.is_empty();
            if fully_gone {
                batch.delete_cf(&cf_buckets, id.to_le_bytes());
            } else {
                let value =
                    bincode::serialize(&state).map_err(|e| Error::Serialization(e.to_string()))?;
                batch.put_cf(&cf_buckets, id.to_le_bytes(), value);
            }
        }

        self.db
            .write(batch)
            .map_err(|e| Error::Storage(e.to_string()))?;
        Ok(super::EraseOutcome {
            nodes_deleted,
            bytes_freed,
        })
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

    /// Rebuild the MMR proof for the exact commitment a challenge cites.
    ///
    /// The cited commitment covers leaves `[commitment_start_seq, …)` up to
    /// whatever leaf count reproduces `commitment_root`. Later commits grow
    /// the MMR (different root) and prunes shift the window, so the proof is
    /// generated by replaying the leaf history from `commitment_start_seq`
    /// until the root matches, then proving `leaf_index` inside that state.
    pub fn get_mmr_proof_for_commitment(
        &self,
        bucket_id: BucketId,
        commitment_root: H256,
        commitment_start_seq: u64,
        leaf_index: u64,
    ) -> Result<storage_primitives::MmrProof, Error> {
        let bucket = self
            .get_bucket(bucket_id)
            .ok_or(Error::BucketNotFound(bucket_id))?;

        // Full known leaf history in sequence order: stashed (pruned but not
        // yet erased) ranges first, then the live leaves. Ranges are pushed
        // in prune order, so this is already sorted; the sort defends the
        // invariant cheaply.
        let mut history: Vec<(u64, &MmrLeaf)> = Vec::new();
        for range in &bucket.pruned {
            for (i, leaf) in range.leaves.iter().enumerate() {
                history.push((range.first_seq.saturating_add(i as u64), leaf));
            }
        }
        for (i, leaf) in bucket.leaves.iter().enumerate() {
            history.push((bucket.start_seq.saturating_add(i as u64), leaf));
        }
        history.sort_by_key(|(seq, _)| *seq);

        let start_pos = history
            .iter()
            .position(|(seq, _)| *seq == commitment_start_seq)
            .ok_or_else(|| {
                Error::NodeNotFound(format!(
                    "no leaf at seq {commitment_start_seq} (pruned and erased, or never committed)"
                ))
            })?;
        let window = &history[start_pos..];

        // The commitment is some prefix of `window`; replay until the root
        // matches, stopping at any sequence gap (an erased middle range).
        // First match wins: a longer prefix hashes differently.
        // Linear scan with one root recompute per pushed leaf — fine at
        // current bucket sizes; cache (root -> leaf_count) if buckets grow
        // past ~10^4 leaves.
        let mut mmr = crate::mmr::Mmr::new();
        let mut matched_count = None;
        let mut expected_seq = commitment_start_seq;
        for (i, (seq, leaf)) in window.iter().enumerate() {
            if *seq != expected_seq {
                break;
            }
            expected_seq = expected_seq.saturating_add(1);
            mmr.push(blake2_256(&leaf.encode()));
            if mmr.root() == commitment_root {
                matched_count = Some(i + 1);
                break;
            }
        }
        let matched_count = matched_count.ok_or_else(|| {
            Error::NodeNotFound(format!(
                "commitment_root 0x{} not reproducible from local leaves",
                hex::encode(commitment_root.as_bytes())
            ))
        })?;

        if leaf_index as usize >= matched_count {
            return Err(Error::NodeNotFound(format!(
                "leaf_{leaf_index} outside commitment ({matched_count} leaves)"
            )));
        }

        let leaf = window[leaf_index as usize].1.clone();
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

    fn attach_deletion_receipt(
        &self,
        bucket_id: BucketId,
        receipt: super::DeletionReceipt,
    ) -> Result<(), Error> {
        self.attach_deletion_receipt(bucket_id, receipt)
    }

    fn deletion_receipt_covering(
        &self,
        bucket_id: BucketId,
        seq: u64,
    ) -> Option<super::DeletionReceipt> {
        self.deletion_receipt_covering(bucket_id, seq)
    }

    fn set_bucket_quota(&self, bucket_id: BucketId, max_bytes: u64) -> Result<(), Error> {
        self.set_bucket_quota(bucket_id, max_bytes)
    }

    fn pruned_ranges(&self, bucket_id: BucketId) -> Vec<super::PrunedRangeInfo> {
        self.pruned_ranges(bucket_id)
    }

    fn is_condemned(&self, bucket_id: BucketId) -> bool {
        self.is_condemned(bucket_id)
    }

    fn erase_pruned_range(
        &self,
        bucket_id: BucketId,
        first_seq: u64,
    ) -> Result<super::EraseOutcome, Error> {
        self.erase_pruned_range(bucket_id, first_seq)
    }

    fn condemn_bucket(&self, bucket_id: BucketId) -> Result<(), Error> {
        self.condemn_bucket(bucket_id)
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
        commitment_root: H256,
        commitment_start_seq: u64,
        leaf_index: u64,
    ) -> Result<storage_primitives::MmrProof, Error> {
        self.get_mmr_proof_for_commitment(
            bucket_id,
            commitment_root,
            commitment_start_seq,
            leaf_index,
        )
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

    /// Create a bucket and commit `n` single-node leaves, returning their roots.
    fn bucket_with_leaves(storage: &DiskStorage, bucket_id: BucketId, n: usize) -> Vec<H256> {
        storage.init_bucket(bucket_id, u64::MAX).unwrap();
        (0..n)
            .map(|i| {
                let data = vec![i as u8; 8];
                let hash = blake2_256(&data);
                storage.store_node(bucket_id, hash, data, None).unwrap();
                storage.commit(bucket_id, vec![hash]).unwrap();
                hash
            })
            .collect()
    }

    #[test]
    fn delete_before_rejects_rewind_and_overshoot() {
        let dir = TempDir::new().unwrap();
        let storage = DiskStorage::new(dir.path()).unwrap();
        bucket_with_leaves(&storage, 1, 3);

        storage.delete_before(1, 2).unwrap();

        // Rewind below the current start_seq must be rejected, not wrap.
        let err = storage.delete_before(1, 1).unwrap_err();
        assert!(
            matches!(
                err,
                Error::InvalidStartSeq {
                    requested: 1,
                    current: 2,
                    end: 3
                }
            ),
            "unexpected error: {err:?}"
        );

        // Advancing past the last leaf + 1 must be rejected, not silently Ok.
        let err = storage.delete_before(1, 4).unwrap_err();
        assert!(
            matches!(err, Error::InvalidStartSeq { requested: 4, .. }),
            "unexpected error: {err:?}"
        );
    }

    #[test]
    fn delete_before_noop_at_current_start_seq() {
        let dir = TempDir::new().unwrap();
        let storage = DiskStorage::new(dir.path()).unwrap();
        bucket_with_leaves(&storage, 1, 2);
        let before = storage.get_bucket(1).unwrap();

        let (root, start_seq, leaf_count) = storage.delete_before(1, before.start_seq).unwrap();

        assert_eq!(root, before.mmr_root);
        assert_eq!(start_seq, before.start_seq);
        assert_eq!(leaf_count, before.leaf_count());
    }

    #[test]
    fn delete_before_prunes_prefix_and_rebuilds_mmr() {
        let dir = TempDir::new().unwrap();
        let storage = DiskStorage::new(dir.path()).unwrap();
        bucket_with_leaves(&storage, 1, 3);
        let before = storage.get_bucket(1).unwrap();

        let (root, start_seq, leaf_count) = storage.delete_before(1, 2).unwrap();

        assert_eq!(start_seq, 2);
        assert_eq!(leaf_count, 1);
        assert_ne!(root, before.mmr_root);
        // The new root must equal an MMR built from the surviving leaf alone.
        let after = storage.get_bucket(1).unwrap();
        let mut mmr = crate::mmr::Mmr::new();
        for leaf in &after.leaves {
            mmr.push(blake2_256(&leaf.encode()));
        }
        assert_eq!(root, mmr.root());
        // Deleting up to the end (empty bucket) is a valid full prune.
        let (_, start_seq, leaf_count) = storage.delete_before(1, 3).unwrap();
        assert_eq!((start_seq, leaf_count), (3, 0));
    }

    /// Store a two-chunk file: two leaf nodes + one internal node.
    /// Returns (root, chunk hashes).
    fn store_two_chunk_tree(
        storage: &DiskStorage,
        bucket_id: BucketId,
        tag: u8,
    ) -> (H256, [H256; 2]) {
        let chunk_a = vec![tag, 1, 1, 1];
        let chunk_b = vec![tag, 2, 2, 2];
        let hash_a = blake2_256(&chunk_a);
        let hash_b = blake2_256(&chunk_b);
        storage
            .store_node(bucket_id, hash_a, chunk_a, None)
            .unwrap();
        storage
            .store_node(bucket_id, hash_b, chunk_b, None)
            .unwrap();
        let root_data: Vec<u8> = [hash_a.as_bytes(), hash_b.as_bytes()].concat();
        let root = blake2_256(&root_data);
        storage
            .store_node(bucket_id, root, root_data, Some(vec![hash_a, hash_b]))
            .unwrap();
        (root, [hash_a, hash_b])
    }

    #[test]
    fn commit_increments_refcounts() {
        let dir = TempDir::new().unwrap();
        let storage = DiskStorage::new(dir.path()).unwrap();
        storage.init_bucket(1, u64::MAX).unwrap();
        let (root, [hash_a, hash_b]) = store_two_chunk_tree(&storage, 1, 0);

        // Stored but uncommitted: charge record exists with count 0.
        assert_eq!(storage.refcount_entry(&root).unwrap(), Some((0, 1, 64)));
        assert_eq!(storage.refcount_entry(&hash_a).unwrap(), Some((0, 1, 4)));

        storage.commit(1, vec![root]).unwrap();
        assert_eq!(storage.refcount_entry(&root).unwrap(), Some((1, 1, 64)));
        assert_eq!(storage.refcount_entry(&hash_a).unwrap(), Some((1, 1, 4)));
        assert_eq!(storage.refcount_entry(&hash_b).unwrap(), Some((1, 1, 4)));

        // Committing the same root as a second leaf counts again.
        storage.commit(1, vec![root]).unwrap();
        assert_eq!(storage.refcount_entry(&root).unwrap(), Some((2, 1, 64)));
        assert_eq!(storage.refcount_entry(&hash_a).unwrap(), Some((2, 1, 4)));
    }

    #[test]
    fn shared_chunk_across_buckets_counts_twice_charges_once() {
        let dir = TempDir::new().unwrap();
        let storage = DiskStorage::new(dir.path()).unwrap();
        storage.init_bucket(1, u64::MAX).unwrap();
        storage.init_bucket(2, u64::MAX).unwrap();

        let data = vec![9u8; 16];
        let hash = blake2_256(&data);
        storage.store_node(1, hash, data.clone(), None).unwrap();
        // Second bucket stores the same content: global dedup, no re-charge.
        storage.store_node(2, hash, data, None).unwrap();
        assert_eq!(storage.get_bucket(2).unwrap().used_bytes, 0);

        storage.commit(1, vec![hash]).unwrap();
        storage.commit(2, vec![hash]).unwrap();
        // Two references, still charged to the first storer.
        assert_eq!(storage.refcount_entry(&hash).unwrap(), Some((2, 1, 16)));
    }

    #[test]
    fn commit_fails_on_missing_subtree_node() {
        let dir = TempDir::new().unwrap();
        let storage = DiskStorage::new(dir.path()).unwrap();
        storage.init_bucket(1, u64::MAX).unwrap();
        let (root, [hash_a, _]) = store_two_chunk_tree(&storage, 1, 0);

        // Simulate a vanished child (e.g. erased between upload and commit).
        let cf = storage.db.cf_handle(CF_NODES).unwrap();
        storage.db.delete_cf(&cf, hash_a.as_bytes()).unwrap();

        let err = storage.commit(1, vec![root]).unwrap_err();
        assert!(matches!(err, Error::NodeNotFound(_)), "got {err:?}");
        // Nothing was committed: no refcount increments applied.
        assert_eq!(storage.refcount_entry(&root).unwrap(), Some((0, 1, 64)));
    }

    #[test]
    fn delete_before_keeps_used_bytes_until_erasure() {
        let dir = TempDir::new().unwrap();
        let storage = DiskStorage::new(dir.path()).unwrap();
        storage.init_bucket(1, u64::MAX).unwrap();
        let (root, _) = store_two_chunk_tree(&storage, 1, 0);
        storage.commit(1, vec![root]).unwrap();
        let used = storage.get_bucket(1).unwrap().used_bytes;
        assert_eq!(used, 72); // 2 chunks of 4 + internal node of 64

        storage.delete_before(1, 1).unwrap();
        // Quota tracks disk actually consumed: the stash still holds the
        // bytes, so nothing is credited at prune time.
        assert_eq!(storage.get_bucket(1).unwrap().used_bytes, used);

        let outcome = storage.erase_pruned_range(1, 0).unwrap();
        assert_eq!(outcome.nodes_deleted, 3);
        assert_eq!(outcome.bytes_freed, 72);
        assert_eq!(storage.get_bucket(1).unwrap().used_bytes, 0);
        assert!(storage.get_node(&root).is_none());
    }

    #[test]
    fn erase_keeps_shared_nodes_and_credits_the_charged_bucket() {
        let dir = TempDir::new().unwrap();
        let storage = DiskStorage::new(dir.path()).unwrap();
        storage.init_bucket(1, u64::MAX).unwrap();
        storage.init_bucket(2, u64::MAX).unwrap();

        // Both buckets commit the same single-chunk leaf; bucket 1 stored it
        // first and carries the charge.
        let data = vec![7u8; 16];
        let hash = blake2_256(&data);
        storage.store_node(1, hash, data.clone(), None).unwrap();
        storage.store_node(2, hash, data, None).unwrap();
        storage.commit(1, vec![hash]).unwrap();
        storage.commit(2, vec![hash]).unwrap();
        assert_eq!(storage.get_bucket(1).unwrap().used_bytes, 16);
        assert_eq!(storage.get_bucket(2).unwrap().used_bytes, 0);

        // Bucket 1 deletes: the node survives (bucket 2 still references it)
        // and nothing is credited — the bytes are still on disk.
        storage.delete_before(1, 1).unwrap();
        let outcome = storage.erase_pruned_range(1, 0).unwrap();
        assert_eq!(outcome, super::super::EraseOutcome::default());
        assert!(storage.get_node(&hash).is_some());
        assert_eq!(storage.refcount_entry(&hash).unwrap(), Some((1, 1, 16)));
        assert_eq!(storage.get_bucket(1).unwrap().used_bytes, 16);

        // Bucket 2 deletes too: last reference dies, node erased, and the
        // credit goes to the bucket that was charged (bucket 1).
        storage.delete_before(2, 1).unwrap();
        let outcome = storage.erase_pruned_range(2, 0).unwrap();
        assert_eq!(outcome.nodes_deleted, 1);
        assert_eq!(outcome.bytes_freed, 16);
        assert!(storage.get_node(&hash).is_none());
        assert_eq!(storage.get_bucket(1).unwrap().used_bytes, 0);
    }

    #[test]
    fn erase_is_idempotent() {
        let dir = TempDir::new().unwrap();
        let storage = DiskStorage::new(dir.path()).unwrap();
        bucket_with_leaves(&storage, 1, 2);
        storage.delete_before(1, 1).unwrap();

        let first = storage.erase_pruned_range(1, 0).unwrap();
        assert!(first.nodes_deleted > 0);
        let second = storage.erase_pruned_range(1, 0).unwrap();
        assert_eq!(second, super::super::EraseOutcome::default());
    }

    #[test]
    fn condemn_then_erase_removes_bucket_row() {
        let dir = TempDir::new().unwrap();
        let storage = DiskStorage::new(dir.path()).unwrap();
        let roots = bucket_with_leaves(&storage, 1, 2);

        storage.condemn_bucket(1).unwrap();
        assert!(storage.is_condemned(1));
        let ranges = storage.pruned_ranges(1);
        assert_eq!(ranges.len(), 1);
        assert_eq!((ranges[0].first_seq, ranges[0].end_seq), (0, 2));
        // Condemning twice is a no-op.
        storage.condemn_bucket(1).unwrap();
        assert!(storage.is_condemned(1));

        let outcome = storage.erase_pruned_range(1, 0).unwrap();
        assert_eq!(outcome.nodes_deleted, 2);
        assert!(storage.get_bucket(1).is_none(), "bucket row must be gone");
        for root in roots {
            assert!(storage.get_node(&root).is_none());
        }

        // Condemning a bucket that never stored anything drops it directly.
        storage.init_bucket(2, u64::MAX).unwrap();
        storage.condemn_bucket(2).unwrap();
        assert!(storage.get_bucket(2).is_none());
    }

    #[test]
    fn set_bucket_quota_enforced_on_next_store() {
        let dir = TempDir::new().unwrap();
        let storage = DiskStorage::new(dir.path()).unwrap();
        storage.init_bucket(1, u64::MAX).unwrap();

        let data = vec![1u8; 8];
        let hash = blake2_256(&data);
        storage.store_node(1, hash, data, None).unwrap();

        storage.set_bucket_quota(1, 10).unwrap();
        let data2 = vec![2u8; 8];
        let hash2 = blake2_256(&data2);
        let err = storage.store_node(1, hash2, data2, None).unwrap_err();
        assert!(
            matches!(err, Error::QuotaExceeded { used: 8, max: 10 }),
            "got {err:?}"
        );
        // Unknown bucket errors instead of creating state.
        assert!(storage.set_bucket_quota(99, 10).is_err());
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
            let hash = blake2_256(&data);
            storage.store_node(1, hash, data, None).unwrap();
            let (root, _, _) = storage.commit(1, vec![hash]).unwrap();
            roots.push(root);
        }

        let proof = storage
            .get_mmr_proof_for_commitment(1, roots[1], 0, 1)
            .unwrap();
        assert!(storage_primitives::verify_mmr_proof(&proof, &roots[1]));
        // Current root (3 leaves) must NOT verify this two-leaf proof.
        assert_ne!(roots[1], roots[2]);

        // A leaf index outside the cited commitment is rejected even though
        // the bucket currently holds it.
        assert!(storage
            .get_mmr_proof_for_commitment(1, roots[1], 0, 2)
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
        let proof = storage
            .get_mmr_proof_for_commitment(1, post_prune_root, 1, 1)
            .unwrap();
        assert_eq!(proof.leaf.data_root, expected.data_root);
        assert!(storage_primitives::verify_mmr_proof(
            &proof,
            &post_prune_root
        ));
    }

    #[test]
    fn proof_from_stash_until_erased() {
        let dir = TempDir::new().unwrap();
        let storage = DiskStorage::new(dir.path()).unwrap();
        bucket_with_leaves(&storage, 1, 2);
        let old_root = storage.get_bucket(1).unwrap().mmr_root;

        storage.delete_before(1, 2).unwrap();

        // Pruned leaves live on in the stash: the provider must stay able to
        // prove the old commitment while it is still challengeable.
        let proof = storage
            .get_mmr_proof_for_commitment(1, old_root, 0, 0)
            .unwrap();
        assert!(storage_primitives::verify_mmr_proof(&proof, &old_root));

        // After physical erasure the leaves (and proofs) are gone for good.
        storage.erase_pruned_range(1, 0).unwrap();
        assert!(storage
            .get_mmr_proof_for_commitment(1, old_root, 0, 0)
            .is_err());
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
}
