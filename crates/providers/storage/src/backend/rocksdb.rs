// SPDX-License-Identifier: Apache-2.0

//! RocksDB-backed [`StorageBackend`]: persists the [`types`] records to disk.
//!
//! Only the engine lives here - the record shapes it reads and writes are
//! defined in [`types`], which owns their on-disk encoding.
//!
//! [`types`]: super::types

use super::types::FORMAT_VERSION;
use super::{
    canonical_size, BucketInfo, BucketState, ChunkTreeNode, CommitOutcome, DeletionReceipt,
    EraseOutcome, PrunedRange, PrunedRangeInfo, StorageBackend, StorageStats,
};
use crate::error::Error;
use codec::{DecodeAll, Encode};
use rocksdb::{
    Direction, ErrorKind, IteratorMode, Options, Transaction, TransactionDB, TransactionDBOptions,
    TransactionOptions, WriteOptions,
};
use sp_core::H256;
use std::collections::HashMap;
use std::path::Path;
use std::sync::Arc;
use storage_primitives::{
    mmr::{self, MmrLeaf, MmrProof},
    BucketId, Commitment, DEFAULT_CHUNK_SIZE,
};

/// Column families for organizing data
const CF_NODES: &str = "nodes";
const CF_BUCKETS: &str = "buckets";
const CF_ROOT_TO_BUCKET: &str = "root_to_bucket";
/// Per-node reference counts: how many committed leaves (live or stashed,
/// across all buckets) reach the node. Keyed like CF_NODES.
const CF_REFCOUNTS: &str = "refcounts";
const CF_METADATA: &str = "metadata";
/// Every MMR leaf of every bucket, live and pruned, keyed by [`seq_key`].
/// Value: SCALE(`MmrLeaf`).
const CF_LEAVES: &str = "leaves";
/// Pruned ranges whose nodes are not all deleted, keyed by [`seq_key`] with
/// the range's first sequence number. Value: SCALE(`PrunedRange`).
const CF_PRUNED_RANGES: &str = "pruned_ranges";
/// Admin deletion receipts, keyed by [`seq_key`] with the receipt's
/// `new_start_seq`. Value: SCALE(`DeletionReceipt`).
const CF_DELETION_RECEIPTS: &str = "deletion_receipts";
/// Every column family this engine opens.
const COLUMN_FAMILIES: [&str; 8] = [
    CF_NODES,
    CF_BUCKETS,
    CF_ROOT_TO_BUCKET,
    CF_REFCOUNTS,
    CF_METADATA,
    CF_LEAVES,
    CF_PRUNED_RANGES,
    CF_DELETION_RECEIPTS,
];

mod refcounts;

use refcounts::{decode_refcount, encode_refcount};

/// RocksDB key for the persisted format version.
const KEY_FORMAT_VERSION: &[u8] = b"format_version";

/// How long a writer waits for another writer on the same bucket before
/// failing with [`Error::BucketBusy`]. A `commit` holds the bucket for the
/// MMR rebuild only, so a longer wait means a stuck writer, and the caller
/// can retry.
const BUCKET_LOCK_TIMEOUT_MS: i64 = 30_000;

/// Distinct nodes one erase transaction covers before it stops at a leaf
/// boundary. 65,536 nodes is about 32,768 chunks, 8 GiB of data at
/// `DEFAULT_CHUNK_SIZE`, and the transaction then locks and rewrites a few MB
/// of refcount records. One leaf can reach `MAX_TREE_WALK_VISITS` nodes and
/// forms a batch of its own, the same per-leaf bound `commit` uses.
const ERASE_BATCH_NODES: usize = 1 << 16;

/// Disk-based storage backend using RocksDB.
///
/// The read-modify-write paths (`init_bucket`, `store_node`, `commit`,
/// `delete_before`) each run in one pessimistic transaction that locks the
/// bucket row exclusively, so writers to one bucket serialize and writers to
/// different buckets do not. Every transaction runs with deadlock detection,
/// so a lock cycle fails one side immediately with [`Error::BucketBusy`]
/// instead of waiting out the lock timeout. Reads run outside transactions.
pub struct DiskStorage {
    db: Arc<TransactionDB>,
    max_tree_walk_visits: u64,
    erase_batch_nodes: usize,
}

/// What `erase_pruned_range` learns from walking a stashed range before it
/// opens the transaction.
#[derive(Default)]
struct EraseWalk {
    /// Number of times each node occurs in the walked trees.
    decrements: HashMap<H256, u64>,
    /// One error per leaf tree the walk could not finish.
    failures: Vec<Error>,
    /// Number of leaves the walk covered.
    leaves: u64,
}

/// What `commit` learns from walking its roots before it opens the
/// transaction.
struct CommitWalk {
    data_roots: Vec<H256>,
    data_sizes: Vec<u64>,
    /// Number of times each node occurs in the walked trees.
    increments: HashMap<H256, u64>,
}

/// Key of a per-bucket record at sequence number `seq`: both big-endian, so
/// one bucket's records are contiguous and ordered by `seq`.
fn seq_key(bucket_id: BucketId, seq: u64) -> [u8; 16] {
    let mut key = [0u8; 16];
    key[..8].copy_from_slice(&bucket_id.to_be_bytes());
    key[8..].copy_from_slice(&seq.to_be_bytes());
    key
}

/// The sequence number in a [`seq_key`] of `bucket_id`; `None` for a key of
/// another bucket.
fn seq_of(bucket_id: BucketId, key: &[u8]) -> Option<u64> {
    let key: [u8; 16] = key.try_into().ok()?;
    (key[..8] == bucket_id.to_be_bytes()).then(|| u64::from_be_bytes(key[8..].try_into().unwrap()))
}

/// The leaves `from..from + count` of `bucket_id` from an iterator positioned
/// at `from`. Stops at the first missing leaf, so the result can be shorter
/// than `count`.
fn read_leaves(
    entries: impl Iterator<Item = Result<(Box<[u8]>, Box<[u8]>), rocksdb::Error>>,
    bucket_id: BucketId,
    from: u64,
    count: u64,
) -> Result<Vec<MmrLeaf>, Error> {
    let mut leaves = Vec::new();
    for entry in entries {
        let (key, value) = entry?;
        let next = from.checked_add(leaves.len() as u64);
        if leaves.len() as u64 == count || next.is_none() || seq_of(bucket_id, &key) != next {
            break;
        }
        leaves.push(MmrLeaf::decode_all(&mut &value[..])?);
    }
    Ok(leaves)
}

/// The error for leaves `start_seq..start_seq + leaf_count` that `bucket`
/// should hold but the leaf store lacks.
fn leaves_not_held(bucket: &BucketState, start_seq: u64, leaf_count: u64) -> Error {
    Error::LeavesNotHeld {
        start_seq,
        leaf_count,
        local_start_seq: bucket.start_seq,
        local_end: bucket.end_seq(),
    }
}

/// The commitment the provider signs for the bucket in this state.
fn commitment_of(bucket: &BucketState) -> Commitment {
    Commitment {
        mmr_root: bucket.mmr_root,
        start_seq: bucket.start_seq,
        leaf_count: bucket.leaf_count,
    }
}

/// A lock wait that timed out is the caller's signal to retry; every other
/// engine failure stays an engine error.
fn bucket_write_error(bucket_id: BucketId, error: rocksdb::Error) -> Error {
    match error.kind() {
        ErrorKind::TimedOut | ErrorKind::Busy => Error::BucketBusy(bucket_id),
        _ => Error::RocksDb(error),
    }
}

impl DiskStorage {
    /// Create a new disk storage instance.
    pub fn new(path: impl AsRef<Path>) -> Result<Self, Error> {
        let mut txn_db_opts = TransactionDBOptions::default();
        txn_db_opts.set_txn_lock_timeout(BUCKET_LOCK_TIMEOUT_MS);
        txn_db_opts.set_default_lock_timeout(BUCKET_LOCK_TIMEOUT_MS);
        let db = TransactionDB::open_cf(
            &Self::db_options(),
            &txn_db_opts,
            path.as_ref(),
            COLUMN_FAMILIES,
        )?;

        Self::check_format_version(&db)?;

        Ok(Self {
            db: Arc::new(db),
            max_tree_walk_visits: super::MAX_TREE_WALK_VISITS,
            erase_batch_nodes: ERASE_BATCH_NODES,
        })
    }

    /// Read a CF_REFCOUNTS entry outside a transaction:
    /// `(count, charged_bucket, size)`.
    #[cfg(test)]
    fn refcount_entry(&self, hash: &H256) -> Result<Option<(u64, u64, u64)>, Error> {
        let cf = self
            .db
            .cf_handle(CF_REFCOUNTS)
            .ok_or(Error::ColumnFamilyMissing(CF_REFCOUNTS))?;
        self.db
            .get_cf(&cf, hash.as_bytes())?
            .map(|value| decode_refcount(&value))
            .transpose()
    }

    /// Lock a CF_REFCOUNTS entry for `txn` and decode it:
    /// `(count, charged_bucket, size)`.
    fn refcount_for_update(
        &self,
        txn: &Transaction<'_, TransactionDB>,
        bucket_id: BucketId,
        hash: &H256,
    ) -> Result<Option<(u64, u64, u64)>, Error> {
        let cf = self
            .db
            .cf_handle(CF_REFCOUNTS)
            .ok_or(Error::ColumnFamilyMissing(CF_REFCOUNTS))?;
        txn.get_for_update_cf(&cf, hash.as_bytes(), true)
            .map_err(|error| bucket_write_error(bucket_id, error))?
            .map(|value| decode_refcount(&value))
            .transpose()
    }

    #[cfg(test)]
    fn with_max_tree_walk_visits(mut self, max_visits: u64) -> Self {
        self.max_tree_walk_visits = max_visits;
        self
    }

    #[cfg(test)]
    fn with_erase_batch_nodes(mut self, nodes: usize) -> Self {
        self.erase_batch_nodes = nodes;
        self
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
    fn check_format_version(db: &TransactionDB) -> Result<(), Error> {
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
    fn is_empty(db: &TransactionDB) -> Result<bool, Error> {
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
        let txn = self.transaction();
        if self.bucket_for_update(&txn, bucket_id)?.is_some() {
            return Ok(());
        }
        self.put_bucket(&txn, bucket_id, &BucketState::new(max_bytes))?;
        txn.commit()?;
        Ok(())
    }

    /// Begin a pessimistic transaction with deadlock detection.
    fn transaction(&self) -> Transaction<'_, TransactionDB> {
        let mut txn_opts = TransactionOptions::default();
        txn_opts.set_deadlock_detect(true);
        self.db.transaction_opt(&WriteOptions::default(), &txn_opts)
    }

    /// Lock the bucket row exclusively for `txn` and decode it.
    fn bucket_for_update(
        &self,
        txn: &Transaction<'_, TransactionDB>,
        bucket_id: BucketId,
    ) -> Result<Option<BucketState>, Error> {
        let cf = self
            .db
            .cf_handle(CF_BUCKETS)
            .ok_or(Error::ColumnFamilyMissing(CF_BUCKETS))?;
        let Some(value) = txn
            .get_for_update_cf(&cf, bucket_id.to_le_bytes(), true)
            .map_err(|error| bucket_write_error(bucket_id, error))?
        else {
            return Ok(None);
        };
        Ok(Some(BucketState::decode_all(&mut &value[..])?))
    }

    /// Write the bucket row within `txn`.
    fn put_bucket(
        &self,
        txn: &Transaction<'_, TransactionDB>,
        bucket_id: BucketId,
        bucket: &BucketState,
    ) -> Result<(), Error> {
        let cf = self
            .db
            .cf_handle(CF_BUCKETS)
            .ok_or(Error::ColumnFamilyMissing(CF_BUCKETS))?;
        txn.put_cf(&cf, bucket_id.to_le_bytes(), bucket.encode())
            .map_err(|error| bucket_write_error(bucket_id, error))
    }

    /// The column family `name`.
    fn cf(&self, name: &'static str) -> Result<&rocksdb::ColumnFamily, Error> {
        self.db
            .cf_handle(name)
            .ok_or(Error::ColumnFamilyMissing(name))
    }

    /// Whether a pruned range of the bucket exists, as `txn` sees it.
    fn has_pruned_range(
        &self,
        txn: &Transaction<'_, TransactionDB>,
        bucket_id: BucketId,
    ) -> Result<bool, Error> {
        let first = txn
            .iterator_cf(
                self.cf(CF_PRUNED_RANGES)?,
                IteratorMode::From(&seq_key(bucket_id, 0), Direction::Forward),
            )
            .next()
            .transpose()?;
        Ok(first.is_some_and(|(key, _)| seq_of(bucket_id, &key).is_some()))
    }

    /// Whether a pruned range of the bucket ends at `new_start_seq`, as `txn` sees
    /// it.
    fn range_ends_at(
        &self,
        txn: &Transaction<'_, TransactionDB>,
        bucket_id: BucketId,
        new_start_seq: u64,
    ) -> Result<bool, Error> {
        for item in txn.iterator_cf(
            self.cf(CF_PRUNED_RANGES)?,
            IteratorMode::From(&seq_key(bucket_id, 0), Direction::Forward),
        ) {
            let (key, value) = item?;
            if seq_of(bucket_id, &key).is_none() {
                break;
            }
            if PrunedRange::decode_all(&mut &value[..])?.new_start_seq == new_start_seq {
                return Ok(true);
            }
        }
        Ok(false)
    }

    /// The retained leaves of `bucket`, oldest first.
    fn retained_leaves(
        &self,
        bucket_id: BucketId,
        bucket: &BucketState,
    ) -> Result<Vec<MmrLeaf>, Error> {
        let leaves = read_leaves(
            self.db.iterator_cf(
                self.cf(CF_LEAVES)?,
                IteratorMode::From(&seq_key(bucket_id, bucket.start_seq), Direction::Forward),
            ),
            bucket_id,
            bucket.start_seq,
            bucket.leaf_count,
        )?;
        if leaves.len() as u64 != bucket.leaf_count {
            return Err(leaves_not_held(bucket, bucket.start_seq, bucket.leaf_count));
        }
        Ok(leaves)
    }

    /// Get bucket state (internal, returns the fixed-size BucketState).
    ///
    /// `Ok(None)` means the bucket genuinely does not exist. `Err` means the
    /// backend itself failed (a RocksDB read error, or a stored record that
    /// could not be decoded) - the bucket may still be there.
    fn get_bucket(&self, bucket_id: BucketId) -> Result<Option<BucketState>, Error> {
        self.read_decoded(CF_BUCKETS, &bucket_id.to_le_bytes())
    }

    /// Read `key` from column family `cf` and decode it. `Ok(None)` when the
    /// key is absent; `Err` when the read or the decode fails.
    fn read_decoded<T: codec::Decode>(
        &self,
        cf: &'static str,
        key: &[u8],
    ) -> Result<Option<T>, Error> {
        let handle = self
            .db
            .cf_handle(cf)
            .ok_or(Error::ColumnFamilyMissing(cf))?;
        self.db
            .get_cf(&handle, key)?
            .map(|value| Ok(T::decode_all(&mut &value[..])?))
            .transpose()
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

    /// Ids of every locally-held bucket.
    pub fn bucket_ids(&self) -> Vec<BucketId> {
        self.iter_buckets(|bucket_id, _| bucket_id)
    }

    /// Totals across all buckets, summed from the per-bucket counters in
    /// CF_BUCKETS. Cost is one pass over the bucket rows, independent of how
    /// many nodes are stored.
    pub fn stats(&self) -> StorageStats {
        self.iter_buckets(|_, state| (state.node_count, state.used_bytes))
            .into_iter()
            .fold(StorageStats::default(), |totals, (nodes, bytes)| {
                StorageStats {
                    buckets: totals.buckets + 1,
                    nodes: totals.nodes.saturating_add(nodes),
                    bytes: totals.bytes.saturating_add(bytes),
                }
            })
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
                expected: expected_hash,
                actual: actual_hash,
            });
        }

        let cf_nodes = self
            .db
            .cf_handle(CF_NODES)
            .ok_or(Error::ColumnFamilyMissing(CF_NODES))?;

        // If internal node, verify children exist
        if let ChunkTreeNode::Internal { left, right } = &node {
            let mut missing = Vec::new();
            for child in [*left, *right] {
                if !self.has_node(&child)? {
                    missing.push(child);
                }
            }
            if !missing.is_empty() {
                return Err(Error::ChildrenMissing(missing));
            }
        }

        let txn = self.transaction();
        let mut bucket = self
            .bucket_for_update(&txn, bucket_id)?
            .ok_or(Error::BucketNotFound(bucket_id))?;
        if bucket.condemned {
            return Err(Error::BucketCondemned(bucket_id));
        }

        let charge = node.size();

        let new_size = bucket.used_bytes.saturating_add(charge);
        if new_size > bucket.max_bytes {
            return Err(Error::QuotaExceeded {
                used: bucket.used_bytes,
                max: bucket.max_bytes,
            });
        }

        let key = expected_hash.as_bytes();
        let stored = txn
            .get_for_update_cf(&cf_nodes, key, true)
            .map_err(|error| bucket_write_error(bucket_id, error))?;
        if stored.is_none() {
            txn.put_cf(&cf_nodes, key, node.encode())
                .map_err(|error| bucket_write_error(bucket_id, error))?;
            bucket.used_bytes = bucket.used_bytes.saturating_add(charge);
            bucket.node_count = bucket.node_count.saturating_add(1);
            let cf_refcounts = self
                .db
                .cf_handle(CF_REFCOUNTS)
                .ok_or(Error::ColumnFamilyMissing(CF_REFCOUNTS))?;
            // Charge record: count stays 0 until a commit references the
            // node; erasure credits `bucket_id`'s used_bytes by `charge`.
            txn.put_cf(&cf_refcounts, key, encode_refcount(0, bucket_id, charge))
                .map_err(|error| bucket_write_error(bucket_id, error))?;
            self.put_bucket(&txn, bucket_id, &bucket)?;
            txn.commit()?;
        }

        Ok(())
    }

    /// Get a node by hash.
    ///
    /// `Ok(None)` means the node genuinely does not exist. `Err` means the
    /// backend itself failed - the node may still be there.
    pub fn get_node(&self, hash: &H256) -> Result<Option<ChunkTreeNode>, Error> {
        self.read_decoded(CF_NODES, hash.as_bytes())
    }

    /// Whether a node is stored under `hash`, without decoding it. `Err`
    /// means the read failed, not that the node is absent.
    fn has_node(&self, hash: &H256) -> Result<bool, Error> {
        let cf = self
            .db
            .cf_handle(CF_NODES)
            .ok_or(Error::ColumnFamilyMissing(CF_NODES))?;
        Ok(self.db.get_pinned_cf(&cf, hash.as_bytes())?.is_some())
    }

    /// Split `hashes` into those stored and those missing.
    pub fn check_exists(
        &self,
        _bucket_id: BucketId,
        hashes: &[H256],
    ) -> Result<(Vec<H256>, Vec<H256>), Error> {
        let mut exists = Vec::new();
        let mut missing = Vec::new();
        for &hash in hashes {
            if self.has_node(&hash)? {
                exists.push(hash);
            } else {
                missing.push(hash);
            }
        }
        Ok((exists, missing))
    }

    /// Commit data roots to the bucket's MMR.
    pub fn commit(
        &self,
        bucket_id: BucketId,
        data_roots: Vec<H256>,
    ) -> Result<CommitOutcome, Error> {
        let walk = self.walk_commit_roots(data_roots)?;
        self.apply_commit(bucket_id, walk)
    }

    /// Check the trees under `data_roots` outside any transaction.
    fn walk_commit_roots(&self, data_roots: Vec<H256>) -> Result<CommitWalk, Error> {
        // Verify all roots exist
        for root in &data_roots {
            if !self.has_node(root)? {
                return Err(Error::RootNotFound(*root));
            }
        }

        // Walk each root once: reject a missing node or a non-canonical tree
        // before the MMR changes, measure the data size, and gather the
        // refcount increments this commit adds.
        let mut increments: HashMap<H256, u64> = HashMap::new();
        let mut data_sizes = Vec::with_capacity(data_roots.len());
        for root in &data_roots {
            let mut leaves = Vec::new();
            self.walk_tree(*root, &mut |hash, node| {
                *increments.entry(hash).or_default() += 1;
                if let ChunkTreeNode::Chunk(data) = node {
                    leaves.push((hash, data.len()));
                }
            })?;
            data_sizes.push(canonical_size(*root, &leaves)?);
        }

        Ok(CommitWalk {
            data_roots,
            data_sizes,
            increments,
        })
    }

    /// Append the walked roots as leaves and add their refcounts in one
    /// transaction.
    fn apply_commit(&self, bucket_id: BucketId, walk: CommitWalk) -> Result<CommitOutcome, Error> {
        let CommitWalk {
            data_roots,
            data_sizes,
            increments,
        } = walk;
        let txn = self.transaction();
        let mut bucket = self
            .bucket_for_update(&txn, bucket_id)?
            .ok_or(Error::BucketNotFound(bucket_id))?;
        if bucket.condemned {
            return Err(Error::BucketCondemned(bucket_id));
        }

        let start_seq = bucket.start_seq;
        let mut leaf_indices = Vec::new();

        let cf_leaves = self.cf(CF_LEAVES)?;
        let mut total_size = 0;
        if bucket.leaf_count > 0 {
            let last_seq = bucket.end_seq() - 1;
            let value = txn
                .get_cf(cf_leaves, seq_key(bucket_id, last_seq))
                .map_err(|error| bucket_write_error(bucket_id, error))?
                .ok_or_else(|| leaves_not_held(&bucket, last_seq, 1))?;
            total_size = MmrLeaf::decode_all(&mut &value[..])?.total_size;
        }

        // Add new leaves
        for (data_root, data_size) in data_roots.iter().zip(data_sizes) {
            let leaf_index = bucket.leaf_count;
            leaf_indices.push(leaf_index);

            total_size = total_size.saturating_add(data_size);
            let leaf = MmrLeaf {
                leaf_version: storage_primitives::LEAF_VERSION,
                data_root: *data_root,
                data_size,
                total_size,
            };
            mmr::push_leaf(&mut bucket.peaks, leaf_index, &leaf);
            txn.put_cf(
                cf_leaves,
                seq_key(bucket_id, bucket.end_seq()),
                leaf.encode(),
            )
            .map_err(|error| bucket_write_error(bucket_id, error))?;
            bucket.leaf_count += 1;
        }

        bucket.mmr_root = mmr::bag_peaks(&bucket.peaks);

        let cf_refcounts = self.cf(CF_REFCOUNTS)?;
        for (hash, n) in increments {
            // The row exists while the node does, so a missing row means an
            // erase deleted the node after the walk. Enforcing user to re-upload
            // rather than use fallback calculation for better accuracy.
            //
            // E.g. Bucket B uploads the same data that bucket A committed and then
            // deleted, so both use one stored node. A's erase deletes the
            // node between B's walk and this transaction. Recreating the row
            // would commit a leaf whose data no longer exists.
            let (count, charged_bucket, size) = self
                .refcount_for_update(&txn, bucket_id, &hash)?
                .ok_or(Error::NodeNotFound(hash))?;
            txn.put_cf(
                cf_refcounts,
                hash.as_bytes(),
                encode_refcount(count.saturating_add(n), charged_bucket, size),
            )
            .map_err(|error| bucket_write_error(bucket_id, error))?;
        }
        self.put_bucket(&txn, bucket_id, &bucket)?;
        txn.commit()?;

        // leaf_count is derived from the same in-scope `bucket` as mmr_root, so the
        // returned (mmr_root, leaf_count) pair is self-consistent.
        Ok(CommitOutcome {
            mmr_root: bucket.mmr_root,
            start_seq,
            leaf_count: bucket.leaf_count,
            leaf_indices,
        })
    }

    /// Delete data before a given sequence number.
    ///
    /// The removed leaves stay in the leaf store and a pruned range record
    /// queues them for erasure — the provider must stay able to prove
    /// commitments covering them until an admin deletion receipt is held and
    /// the canonical checkpoint passed the range, at which point the GC calls
    /// [`erase_pruned_range`](Self::erase_pruned_range). Quota (`used_bytes`)
    /// is not credited here: it tracks disk actually consumed.
    pub fn delete_before(
        &self,
        bucket_id: BucketId,
        new_start_seq: u64,
    ) -> Result<Commitment, Error> {
        let txn = self.transaction();
        let mut bucket = self
            .bucket_for_update(&txn, bucket_id)?
            .ok_or(Error::BucketNotFound(bucket_id))?;

        // start_seq can only advance, and no further than one past the last
        // leaf; a rewind or overshoot is a caller error, never a silent no-op.
        if new_start_seq < bucket.start_seq || new_start_seq > bucket.end_seq() {
            return Err(Error::InvalidStartSeq {
                requested: new_start_seq,
                current: bucket.start_seq,
                end: bucket.end_seq(),
            });
        }

        // Queue the leaves before new_start_seq for erasure
        if new_start_seq > bucket.start_seq {
            let range = PrunedRange {
                first_seq: bucket.start_seq,
                new_start_seq,
                erased_leaves: 0,
            };
            txn.put_cf(
                self.cf(CF_PRUNED_RANGES)?,
                seq_key(bucket_id, bucket.start_seq),
                range.encode(),
            )
            .map_err(|error| bucket_write_error(bucket_id, error))?;
            bucket.leaf_count -= new_start_seq - bucket.start_seq;
            bucket.start_seq = new_start_seq;

            let leaves = read_leaves(
                txn.iterator_cf(
                    self.cf(CF_LEAVES)?,
                    IteratorMode::From(&seq_key(bucket_id, new_start_seq), Direction::Forward),
                ),
                bucket_id,
                new_start_seq,
                bucket.leaf_count,
            )?;
            if leaves.len() as u64 != bucket.leaf_count {
                return Err(leaves_not_held(&bucket, new_start_seq, bucket.leaf_count));
            }
            (bucket.mmr_root, bucket.peaks) = mmr::peaks(&leaves)?;

            self.put_bucket(&txn, bucket_id, &bucket)?;
            txn.commit()?;
        }

        Ok(commitment_of(&bucket))
    }

    /// Set/refresh the bucket quota learned from the chain agreement.
    pub fn set_bucket_quota(&self, bucket_id: BucketId, max_bytes: u64) -> Result<(), Error> {
        let txn = self.transaction();
        let mut bucket = self
            .bucket_for_update(&txn, bucket_id)?
            .ok_or(Error::BucketNotFound(bucket_id))?;
        if bucket.max_bytes != max_bytes {
            bucket.max_bytes = max_bytes;
            self.put_bucket(&txn, bucket_id, &bucket)?;
            txn.commit()?;
        }
        Ok(())
    }

    /// Pruned ranges awaiting physical erasure, oldest first.
    pub fn pruned_ranges(&self, bucket_id: BucketId) -> Result<Vec<PrunedRangeInfo>, Error> {
        if self.get_bucket(bucket_id)?.is_none() {
            return Ok(Vec::new());
        }
        let cf_receipts = self.cf(CF_DELETION_RECEIPTS)?;
        let mut infos = Vec::new();
        for item in self.db.iterator_cf(
            self.cf(CF_PRUNED_RANGES)?,
            IteratorMode::From(&seq_key(bucket_id, 0), Direction::Forward),
        ) {
            let (key, value) = item?;
            let Some(first_seq) = seq_of(bucket_id, &key) else {
                break;
            };
            let range = PrunedRange::decode_all(&mut &value[..])?;
            infos.push(PrunedRangeInfo {
                first_seq,
                end_seq: range.new_start_seq,
                new_start_seq: range.new_start_seq,
                has_receipt: self
                    .db
                    .get_cf(cf_receipts, seq_key(bucket_id, range.new_start_seq))?
                    .is_some(),
            });
        }
        Ok(infos)
    }

    /// Whether the bucket was condemned (deleted on-chain / agreement gone).
    pub fn is_condemned(&self, bucket_id: BucketId) -> Result<bool, Error> {
        Ok(self
            .get_bucket(bucket_id)?
            .is_some_and(|bucket| bucket.condemned))
    }

    /// Store an admin-signed deletion receipt for a pruned range (matched
    /// by `new_start_seq`); replaces a previous receipt for the same range.
    /// A receipt whose signature does not verify is rejected with
    /// [`Error::InvalidDeletionReceipt`].
    pub fn attach_deletion_receipt(
        &self,
        bucket_id: BucketId,
        receipt: DeletionReceipt,
    ) -> Result<(), Error> {
        if !receipt.verifies(bucket_id) {
            return Err(Error::InvalidDeletionReceipt(bucket_id));
        }
        let txn = self.transaction();
        let bucket = self
            .bucket_for_update(&txn, bucket_id)?
            .ok_or(Error::BucketNotFound(bucket_id))?;
        if bucket.condemned {
            return Err(Error::BucketCondemned(bucket_id));
        }
        let key = seq_key(bucket_id, receipt.new_start_seq);
        let cf_receipts = self.cf(CF_DELETION_RECEIPTS)?;
        let replaces_receipt = txn
            .get_cf(cf_receipts, key)
            .map_err(|error| bucket_write_error(bucket_id, error))?
            .is_some();
        if !replaces_receipt && !self.range_ends_at(&txn, bucket_id, receipt.new_start_seq)? {
            return Err(Error::InvalidStartSeq {
                requested: receipt.new_start_seq,
                current: bucket.start_seq,
                end: bucket.end_seq(),
            });
        }
        txn.put_cf(cf_receipts, key, receipt.encode())
            .map_err(|error| bucket_write_error(bucket_id, error))?;
        txn.commit()?;
        Ok(())
    }

    /// The stored receipt with the smallest `new_start_seq` strictly greater
    /// than `seq`, if any.
    pub fn deletion_receipt_covering(
        &self,
        bucket_id: BucketId,
        seq: u64,
    ) -> Result<Option<DeletionReceipt>, Error> {
        let Some(after) = seq.checked_add(1) else {
            return Ok(None);
        };
        let first = self
            .db
            .iterator_cf(
                self.cf(CF_DELETION_RECEIPTS)?,
                IteratorMode::From(&seq_key(bucket_id, after), Direction::Forward),
            )
            .next()
            .transpose()?;
        match first {
            Some((key, value)) if seq_of(bucket_id, &key).is_some() => {
                Ok(Some(DeletionReceipt::decode_all(&mut &value[..])?))
            }
            _ => Ok(None),
        }
    }

    /// Bucket teardown, first half: queue all remaining leaves as one pruned
    /// range and mark the bucket condemned; `erase_pruned_range` later erases
    /// the range and removes the bucket's records. If nothing is left to
    /// erase, they are removed right away. Calling it again on a condemned
    /// bucket resumes a removal that stopped part way.
    pub fn condemn_bucket(&self, bucket_id: BucketId) -> Result<(), Error> {
        let txn = self.transaction();
        let Some(mut bucket) = self.bucket_for_update(&txn, bucket_id)? else {
            return Ok(());
        };

        if !bucket.condemned {
            if bucket.leaf_count > 0 {
                let end_seq = bucket.end_seq();
                let range = PrunedRange {
                    first_seq: bucket.start_seq,
                    new_start_seq: end_seq,
                    erased_leaves: 0,
                };
                txn.put_cf(
                    self.cf(CF_PRUNED_RANGES)?,
                    seq_key(bucket_id, bucket.start_seq),
                    range.encode(),
                )
                .map_err(|error| bucket_write_error(bucket_id, error))?;
                bucket.start_seq = end_seq;
                bucket.leaf_count = 0;
                bucket.mmr_root = H256::zero();
                bucket.peaks.clear();
            }
            bucket.condemned = true;
            self.put_bucket(&txn, bucket_id, &bucket)?;
        }

        let pending = self.has_pruned_range(&txn, bucket_id)?;
        txn.commit()?;
        if !pending {
            self.remove_bucket(bucket_id)?;
        }
        Ok(())
    }

    /// Delete every record of a condemned bucket that has no retained leaf
    /// and no pruned range left, at most `erase_batch_nodes` records per
    /// transaction. The bucket row goes in the last transaction, so a failure
    /// leaves the row for `condemn_bucket` to resume from, and no record
    /// outlives its row.
    fn remove_bucket(&self, bucket_id: BucketId) -> Result<(), Error> {
        while self.remove_bucket_batch(bucket_id)? {}
        Ok(())
    }

    /// One transaction of `remove_bucket`. Returns whether more remain.
    fn remove_bucket_batch(&self, bucket_id: BucketId) -> Result<bool, Error> {
        let txn = self.transaction();
        let Some(bucket) = self.bucket_for_update(&txn, bucket_id)? else {
            return Ok(false);
        };
        if !bucket.condemned || bucket.leaf_count > 0 || self.has_pruned_range(&txn, bucket_id)? {
            return Ok(false);
        }

        let mut doomed = Vec::new();
        let mut more = false;
        'scan: for name in [CF_LEAVES, CF_DELETION_RECEIPTS] {
            for item in txn.iterator_cf(
                self.cf(name)?,
                IteratorMode::From(&seq_key(bucket_id, 0), Direction::Forward),
            ) {
                let (key, _) = item?;
                if seq_of(bucket_id, &key).is_none() {
                    break;
                }
                if doomed.len() == self.erase_batch_nodes {
                    more = true;
                    break 'scan;
                }
                doomed.push((name, key));
            }
        }
        for (name, key) in &doomed {
            txn.delete_cf(self.cf(name)?, key)
                .map_err(|error| bucket_write_error(bucket_id, error))?;
        }
        if !more {
            txn.delete_cf(self.cf(CF_BUCKETS)?, bucket_id.to_le_bytes())
                .map_err(|error| bucket_write_error(bucket_id, error))?;
        }
        txn.commit()?;
        Ok(more)
    }

    /// Physically erase one pruned range. See the trait docs for the
    /// contract; checking that liability has passed is the caller's job.
    pub fn erase_pruned_range(
        &self,
        bucket_id: BucketId,
        first_seq: u64,
    ) -> Result<EraseOutcome, Error> {
        let mut total = EraseOutcome::default();
        loop {
            let (outcome, more) = self.erase_batch(bucket_id, first_seq)?;
            total.nodes_deleted += outcome.nodes_deleted;
            total.bytes_freed += outcome.bytes_freed;
            if !more {
                return Ok(total);
            }
        }
    }

    /// Erase the next batch of a pruned range in one transaction. The flag is
    /// whether the range has leaves left.
    fn erase_batch(
        &self,
        bucket_id: BucketId,
        first_seq: u64,
    ) -> Result<(EraseOutcome, bool), Error> {
        self.get_bucket(bucket_id)?
            .ok_or(Error::BucketNotFound(bucket_id))?;
        let Some(range) =
            self.read_decoded::<PrunedRange>(CF_PRUNED_RANGES, &seq_key(bucket_id, first_seq))?
        else {
            return Ok((EraseOutcome::default(), false));
        };
        // Walk outside the transaction so that uploads and commits to the
        // bucket do not wait for the walk.
        let walk = self.walk_erase_range(bucket_id, &range)?;
        let more = range.erased_leaves + walk.leaves < range.leaf_count();
        self.apply_erase(bucket_id, &range, walk)
            .map(|outcome| (outcome, more))
    }

    /// Refcount decrements for erasing the next leaves of `range`: one per
    /// encounter along each leaf tree, symmetric with commit's increments.
    /// The walk covers whole leaves and stops once the batch reaches
    /// `erase_batch_nodes` distinct nodes.
    ///
    /// A committed leaf is a canonical tree of `data_size` bytes, so the walk
    /// reads only its internal nodes and never a chunk body.
    fn walk_erase_range(
        &self,
        bucket_id: BucketId,
        range: &PrunedRange,
    ) -> Result<EraseWalk, Error> {
        let mut walk = EraseWalk::default();
        for item in self.db.iterator_cf(
            self.cf(CF_LEAVES)?,
            IteratorMode::From(
                &seq_key(bucket_id, range.first_seq + range.erased_leaves),
                Direction::Forward,
            ),
        ) {
            let (key, value) = item?;
            let seq = range.first_seq + range.erased_leaves + walk.leaves;
            if seq >= range.new_start_seq || seq_of(bucket_id, &key) != Some(seq) {
                break;
            }
            let leaf = MmrLeaf::decode_all(&mut &value[..])?;
            walk.leaves += 1;
            let chunk_count = leaf.data_size.div_ceil(u64::from(DEFAULT_CHUNK_SIZE));
            let result = self.walk_canonical_tree(leaf.data_root, chunk_count, &mut |hash| {
                *walk.decrements.entry(hash).or_default() += 1;
            });
            if let Err(error) = result {
                walk.failures.push(error);
            }
            if walk.decrements.len() >= self.erase_batch_nodes {
                break;
            }
        }
        if walk.leaves == 0 {
            return Err(Error::LeavesNotHeld {
                start_seq: range.first_seq + range.erased_leaves,
                leaf_count: range.leaf_count() - range.erased_leaves,
                local_start_seq: range.first_seq,
                local_end: range.new_start_seq,
            });
        }
        Ok(walk)
    }

    /// Mark the leaves `walk` covers as erased and apply `walk.decrements` in
    /// one transaction.
    ///
    /// `walk` comes from `range` alone. While a leaf is not erased, each node
    /// it reaches keeps a count of at least that leaf's share, so no other
    /// erase deletes such a node and the walk outside the transaction stays
    /// valid. A range that changed under the lock was erased further by
    /// another call after the snapshot, so this call is a no-op.
    fn apply_erase(
        &self,
        bucket_id: BucketId,
        range: &PrunedRange,
        walk: EraseWalk,
    ) -> Result<EraseOutcome, Error> {
        let write_error = |error: rocksdb::Error| bucket_write_error(bucket_id, error);
        let txn = self.transaction();
        let bucket = self
            .bucket_for_update(&txn, bucket_id)?
            .ok_or(Error::BucketNotFound(bucket_id))?;
        let cf_ranges = self.cf(CF_PRUNED_RANGES)?;
        let range_key = seq_key(bucket_id, range.first_seq);
        let stored = txn
            .get_cf(cf_ranges, range_key)
            .map_err(write_error)?
            .map(|value| PrunedRange::decode_all(&mut &value[..]))
            .transpose()?;
        if stored.as_ref() != Some(range) {
            return Ok(EraseOutcome::default());
        }
        let advanced = PrunedRange {
            erased_leaves: range.erased_leaves + walk.leaves,
            ..range.clone()
        };
        if advanced.is_erased() {
            txn.delete_cf(cf_ranges, range_key).map_err(write_error)?;
        } else {
            txn.put_cf(cf_ranges, range_key, advanced.encode())
                .map_err(write_error)?;
        }
        for error in &walk.failures {
            // Nodes reached before the failure lose this leaf's reference;
            // unreachable ones keep their count and leak, which is the safe
            // direction.
            tracing::warn!(
                bucket_id,
                error = %error,
                "erase: leaf tree incomplete, applying partial decrements"
            );
        }

        let cf_nodes = self.cf(CF_NODES)?;
        let cf_refcounts = self.cf(CF_REFCOUNTS)?;

        let mut nodes_deleted = 0u64;
        // Per charged bucket: (nodes deleted, bytes freed).
        let mut credits_by_bucket: HashMap<BucketId, (u64, u64)> = HashMap::new();
        for (hash, n) in walk.decrements {
            let Some((count, charged_bucket, size)) =
                self.refcount_for_update(&txn, bucket_id, &hash)?
            else {
                tracing::warn!(
                    bucket_id,
                    hash = %format!("{:#x}", hash),
                    "erase: refcount entry missing, node kept"
                );
                continue;
            };
            let new_count = count.saturating_sub(n);
            if new_count == 0 {
                txn.delete_cf(cf_nodes, hash.as_bytes())
                    .map_err(write_error)?;
                txn.delete_cf(cf_refcounts, hash.as_bytes())
                    .map_err(write_error)?;
                nodes_deleted += 1;
                let credit = credits_by_bucket.entry(charged_bucket).or_default();
                credit.0 += 1;
                credit.1 += size;
            } else {
                txn.put_cf(
                    cf_refcounts,
                    hash.as_bytes(),
                    encode_refcount(new_count, charged_bucket, size),
                )
                .map_err(write_error)?;
            }
        }
        let bytes_freed: u64 = credits_by_bucket.values().map(|(_, bytes)| bytes).sum();

        // Apply quota credits; the erasing bucket's own row may be credited.
        let mut rows: HashMap<BucketId, BucketState> = HashMap::new();
        rows.insert(bucket_id, bucket);
        for (credited, (nodes, bytes)) in credits_by_bucket {
            if let std::collections::hash_map::Entry::Vacant(entry) = rows.entry(credited) {
                match self.bucket_for_update(&txn, credited)? {
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
                state.node_count = state.node_count.saturating_sub(nodes);
            }
        }
        let own = &rows[&bucket_id];
        let fully_gone =
            own.condemned && own.leaf_count == 0 && !self.has_pruned_range(&txn, bucket_id)?;
        for (id, state) in rows {
            self.put_bucket(&txn, id, &state)?;
        }

        txn.commit()?;
        if fully_gone {
            // The erase is committed; `condemn_bucket` resumes a failed removal.
            if let Err(error) = self.remove_bucket(bucket_id) {
                tracing::warn!(bucket_id, error = %error, "erase: bucket removal incomplete");
            }
        }
        Ok(EraseOutcome {
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
            .get_bucket(bucket_id)?
            .ok_or(Error::BucketNotFound(bucket_id))?;
        let leaves = self.retained_leaves(bucket_id, &bucket)?;
        Ok(leaf_proof(bucket_id, &leaves, leaf_index)?.1)
    }

    /// See [`StorageBackend::get_mmr_proof_for_commitment`]. The cited
    /// window is read from the leaf store, which holds the pruned leaves
    /// as well as the retained ones.
    pub fn get_mmr_proof_for_commitment(
        &self,
        bucket_id: BucketId,
        commitment: &storage_primitives::Commitment,
        leaf_index: u64,
    ) -> Result<storage_primitives::MmrProof, Error> {
        let bucket = self
            .get_bucket(bucket_id)?
            .ok_or(Error::BucketNotFound(bucket_id))?;

        if leaf_index >= commitment.leaf_count {
            return Err(Error::CommitmentNotReproducible(format!(
                "leaf_{leaf_index} outside commitment ({} leaves)",
                commitment.leaf_count
            )));
        }

        let window = read_leaves(
            self.db.iterator_cf(
                self.cf(CF_LEAVES)?,
                IteratorMode::From(
                    &seq_key(bucket_id, commitment.start_seq),
                    Direction::Forward,
                ),
            ),
            bucket_id,
            commitment.start_seq,
            commitment.leaf_count,
        )?;
        if window.len() as u64 != commitment.leaf_count {
            let oldest = self
                .db
                .iterator_cf(
                    self.cf(CF_LEAVES)?,
                    IteratorMode::From(&seq_key(bucket_id, 0), Direction::Forward),
                )
                .next()
                .transpose()?
                .and_then(|(key, _)| seq_of(bucket_id, &key));
            return Err(Error::LeavesNotHeld {
                start_seq: commitment.start_seq,
                leaf_count: commitment.leaf_count,
                local_start_seq: oldest.unwrap_or(bucket.start_seq),
                local_end: bucket.end_seq(),
            });
        }

        let (root, proof) = leaf_proof(bucket_id, &window, leaf_index)?;
        // The rebuilt window must reproduce the cited root; a mismatch means
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
            .get_bucket(bucket_id)?
            .ok_or(Error::BucketNotFound(bucket_id))?;

        Ok((bucket.mmr_root, bucket.peaks))
    }
}

impl StorageBackend for DiskStorage {
    fn max_tree_walk_visits(&self) -> u64 {
        self.max_tree_walk_visits
    }

    fn init_bucket(&self, bucket_id: BucketId, max_bytes: u64) -> Result<(), Error> {
        self.init_bucket(bucket_id, max_bytes)
    }

    fn get_bucket(&self, bucket_id: BucketId) -> Result<Option<BucketInfo>, Error> {
        let state = DiskStorage::get_bucket(self, bucket_id)?;
        Ok(state.map(|state| BucketInfo {
            mmr_root: state.mmr_root,
            start_seq: state.start_seq,
            leaf_count: state.leaf_count,
        }))
    }

    fn bucket_ids(&self) -> Vec<BucketId> {
        self.bucket_ids()
    }

    fn stats(&self) -> StorageStats {
        self.stats()
    }

    fn store_node(
        &self,
        bucket_id: BucketId,
        expected_hash: H256,
        node: ChunkTreeNode,
    ) -> Result<(), Error> {
        self.store_node(bucket_id, expected_hash, node)
    }

    fn get_node(&self, hash: &H256) -> Result<Option<ChunkTreeNode>, Error> {
        self.get_node(hash)
    }

    fn check_exists(
        &self,
        bucket_id: BucketId,
        hashes: &[H256],
    ) -> Result<(Vec<H256>, Vec<H256>), Error> {
        self.check_exists(bucket_id, hashes)
    }

    fn commit(&self, bucket_id: BucketId, data_roots: Vec<H256>) -> Result<CommitOutcome, Error> {
        self.commit(bucket_id, data_roots)
    }

    fn delete_before(&self, bucket_id: BucketId, new_start_seq: u64) -> Result<Commitment, Error> {
        self.delete_before(bucket_id, new_start_seq)
    }

    fn attach_deletion_receipt(
        &self,
        bucket_id: BucketId,
        receipt: DeletionReceipt,
    ) -> Result<(), Error> {
        self.attach_deletion_receipt(bucket_id, receipt)
    }

    fn deletion_receipt_covering(
        &self,
        bucket_id: BucketId,
        seq: u64,
    ) -> Result<Option<DeletionReceipt>, Error> {
        self.deletion_receipt_covering(bucket_id, seq)
    }

    fn set_bucket_quota(&self, bucket_id: BucketId, max_bytes: u64) -> Result<(), Error> {
        self.set_bucket_quota(bucket_id, max_bytes)
    }

    fn pruned_ranges(&self, bucket_id: BucketId) -> Result<Vec<PrunedRangeInfo>, Error> {
        self.pruned_ranges(bucket_id)
    }

    fn is_condemned(&self, bucket_id: BucketId) -> Result<bool, Error> {
        self.is_condemned(bucket_id)
    }

    fn erase_pruned_range(
        &self,
        bucket_id: BucketId,
        first_seq: u64,
    ) -> Result<EraseOutcome, Error> {
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
fn leaf_proof(
    bucket_id: BucketId,
    leaves: &[MmrLeaf],
    leaf_index: u64,
) -> Result<(H256, MmrProof), Error> {
    mmr::proof(leaves, leaf_index)?.ok_or(Error::LeafNotFound {
        bucket_id,
        leaf_index,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::backend::{build_merkle_tree, MAX_TREE_DEPTH};
    use rocksdb::DB;
    use sp_core::Pair as _;
    use std::sync::atomic::{AtomicU64, Ordering};
    use storage_primitives::{hash_leaf, verify_merkle_proof};
    use tempfile::TempDir;

    /// Open the database at `path` directly, bypassing `DiskStorage::new` and
    /// its format-version check.
    fn open_raw(path: &Path) -> DB {
        DB::open_cf(&DiskStorage::db_options(), path, COLUMN_FAMILIES).unwrap()
    }

    /// Read-only backend over a `DiskStorage` that counts `get_node` calls.
    /// The read tests use it to bound how many nodes a read visits.
    struct CountingReads<'a> {
        inner: &'a DiskStorage,
        get_node_calls: AtomicU64,
    }

    impl<'a> CountingReads<'a> {
        fn new(inner: &'a DiskStorage) -> Self {
            Self {
                inner,
                get_node_calls: Default::default(),
            }
        }
    }

    impl StorageBackend for CountingReads<'_> {
        fn max_tree_walk_visits(&self) -> u64 {
            self.inner.max_tree_walk_visits
        }
        fn get_node(&self, hash: &H256) -> Result<Option<ChunkTreeNode>, Error> {
            self.get_node_calls.fetch_add(1, Ordering::Relaxed);
            self.inner.get_node(hash)
        }
        fn init_bucket(&self, _: BucketId, _: u64) -> Result<(), Error> {
            unreachable!("read-only")
        }
        fn get_bucket(&self, _: BucketId) -> Result<Option<BucketInfo>, Error> {
            unreachable!("read-only")
        }
        fn bucket_ids(&self) -> Vec<BucketId> {
            unreachable!("read-only")
        }
        fn stats(&self) -> StorageStats {
            unreachable!("read-only")
        }
        fn store_node(&self, _: BucketId, _: H256, _: ChunkTreeNode) -> Result<(), Error> {
            unreachable!("read-only")
        }
        fn check_exists(&self, _: BucketId, _: &[H256]) -> Result<(Vec<H256>, Vec<H256>), Error> {
            unreachable!("read-only")
        }
        fn commit(&self, _: BucketId, _: Vec<H256>) -> Result<CommitOutcome, Error> {
            unreachable!("read-only")
        }
        fn delete_before(&self, _: BucketId, _: u64) -> Result<Commitment, Error> {
            unreachable!("read-only")
        }
        fn attach_deletion_receipt(&self, _: BucketId, _: DeletionReceipt) -> Result<(), Error> {
            unreachable!("read-only")
        }
        fn deletion_receipt_covering(
            &self,
            _: BucketId,
            _: u64,
        ) -> Result<Option<DeletionReceipt>, Error> {
            unreachable!("read-only")
        }
        fn set_bucket_quota(&self, _: BucketId, _: u64) -> Result<(), Error> {
            unreachable!("read-only")
        }
        fn pruned_ranges(&self, _: BucketId) -> Result<Vec<PrunedRangeInfo>, Error> {
            unreachable!("read-only")
        }
        fn is_condemned(&self, _: BucketId) -> Result<bool, Error> {
            unreachable!("read-only")
        }
        fn erase_pruned_range(&self, _: BucketId, _: u64) -> Result<EraseOutcome, Error> {
            unreachable!("read-only")
        }
        fn condemn_bucket(&self, _: BucketId) -> Result<(), Error> {
            unreachable!("read-only")
        }
        fn get_mmr_proof(&self, _: BucketId, _: u64) -> Result<MmrProof, Error> {
            unreachable!("read-only")
        }
        fn get_mmr_proof_for_commitment(
            &self,
            _: BucketId,
            _: &Commitment,
            _: u64,
        ) -> Result<MmrProof, Error> {
            unreachable!("read-only")
        }
        fn get_mmr_peaks(&self, _: BucketId) -> Result<(H256, Vec<H256>), Error> {
            unreachable!("read-only")
        }
    }

    /// The retained leaves of the bucket, oldest first.
    fn live_leaves(storage: &DiskStorage, bucket_id: BucketId) -> Vec<MmrLeaf> {
        let bucket = storage.get_bucket(bucket_id).unwrap().unwrap();
        storage.retained_leaves(bucket_id, &bucket).unwrap()
    }

    /// The pruned range record that starts at `first_seq`.
    fn pruned_range(
        storage: &DiskStorage,
        bucket_id: BucketId,
        first_seq: u64,
    ) -> Option<PrunedRange> {
        storage
            .read_decoded(CF_PRUNED_RANGES, &seq_key(bucket_id, first_seq))
            .unwrap()
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
            [
                "nodes",
                "buckets",
                "root_to_bucket",
                "refcounts",
                "metadata",
                "leaves",
                "pruned_ranges",
                "deletion_receipts"
            ],
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
            // BucketState::new(1_000): zero root, start_seq = 0, no leaves, no peaks,
            // no bytes or nodes, max_bytes = 1_000, condemned = false
            "0000000000000000000000000000000000000000000000000000000000000000\
             0000000000000000\
             0000000000000000\
             00\
             0000000000000000\
             0000000000000000\
             e803000000000000\
             00"
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

        // CF_LEAVES, CF_PRUNED_RANGES, CF_DELETION_RECEIPTS: key = bucket_id
        // then seq, both u64 big-endian.
        storage.commit(bucket_id, vec![chunk_hash]).unwrap();
        let leaf_key = [1u8, 2, 3, 4, 5, 6, 7, 8, 0, 0, 0, 0, 0, 0, 0, 0];
        let cf = storage.db.cf_handle(CF_LEAVES).unwrap();
        let raw = storage.db.get_cf(&cf, leaf_key).unwrap().unwrap();
        assert_eq!(
            array_bytes::bytes2hex("", &raw),
            // SCALE(MmrLeaf): leaf_version, data_root, data_size = 5, total_size = 5
            format!(
                "01{}05000000000000000500000000000000",
                array_bytes::bytes2hex("", chunk_hash)
            )
        );

        let mmr_root = storage.delete_before(bucket_id, 1).unwrap().mmr_root;
        let cf = storage.db.cf_handle(CF_PRUNED_RANGES).unwrap();
        let raw = storage.db.get_cf(&cf, leaf_key).unwrap().unwrap();
        // SCALE(PrunedRange): first_seq = 0, new_start_seq = 1, erased_leaves = 0
        assert_eq!(
            array_bytes::bytes2hex("", &raw),
            "000000000000000001000000000000000000000000000000"
        );

        let receipt = signed_receipt(bucket_id, mmr_root, 1, &admin_pair());
        storage
            .attach_deletion_receipt(bucket_id, receipt.clone())
            .unwrap();
        let cf = storage.db.cf_handle(CF_DELETION_RECEIPTS).unwrap();
        let raw = storage
            .db
            .get_cf(&cf, [1u8, 2, 3, 4, 5, 6, 7, 8, 0, 0, 0, 0, 0, 0, 0, 1])
            .unwrap()
            .unwrap();
        assert_eq!(raw, receipt.encode());
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
        let before = storage.get_bucket(BUCKET).unwrap().unwrap();
        let err = storage.commit(BUCKET, vec![root]).unwrap_err();
        assert!(matches!(err, Error::NonCanonicalTree(_)), "got {err:?}");
        let after = storage.get_bucket(BUCKET).unwrap().unwrap();
        assert_eq!(after, before);
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
        let leaves = live_leaves(&storage, BUCKET);
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
        let leaves = live_leaves(&storage, BUCKET);
        assert_eq!(leaves[0].data_size, 2 * DEFAULT_CHUNK_SIZE as u64 + 10);
    }

    #[test]
    fn commit_rejects_a_single_empty_chunk() {
        let (storage, _dir) = storage_with_bucket(COMMIT_QUOTA);
        let root = store_chunk_of_len(&storage, 0, 0);
        assert_rejected_and_bucket_unchanged(&storage, root);
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
        let used = storage.get_bucket(BUCKET).unwrap().unwrap().used_bytes;
        assert_eq!(storage.get_bucket(BUCKET).unwrap().unwrap().node_count, 2);

        store_parent(&storage, left, right);

        let bucket = storage.get_bucket(BUCKET).unwrap().unwrap();
        assert_eq!(bucket.used_bytes, used + 64);
        assert_eq!(bucket.node_count, 3);
    }

    #[test]
    fn node_count_counts_each_distinct_node_stored() {
        let (storage, _dir) = storage_with_bucket(COMMIT_QUOTA);
        assert_eq!(storage.get_bucket(BUCKET).unwrap().unwrap().node_count, 0);

        for byte in 0..5 {
            store_chunk_of_len(&storage, byte, 10);
        }

        assert_eq!(storage.get_bucket(BUCKET).unwrap().unwrap().node_count, 5);
    }

    #[test]
    fn node_count_ignores_a_node_stored_again_in_the_same_bucket() {
        let (storage, _dir) = storage_with_bucket(COMMIT_QUOTA);
        store_chunk_of_len(&storage, 1, 10);
        store_chunk_of_len(&storage, 1, 10);

        assert_eq!(storage.get_bucket(BUCKET).unwrap().unwrap().node_count, 1);
    }

    #[test]
    fn node_count_ignores_a_node_another_bucket_stored_first() {
        let (storage, _dir) = storage_with_bucket(COMMIT_QUOTA);
        storage.init_bucket(2, COMMIT_QUOTA).unwrap();
        let data = vec![7u8; 10];
        let hash = hash_leaf(&data);
        for bucket_id in [BUCKET, 2] {
            storage
                .store_node(bucket_id, hash, ChunkTreeNode::Chunk(data.clone()))
                .unwrap();
        }

        assert_eq!(storage.get_bucket(BUCKET).unwrap().unwrap().node_count, 1);
        assert_eq!(storage.get_bucket(2).unwrap().unwrap().node_count, 0);
    }

    #[test]
    fn node_count_ignores_rejected_stores() {
        let (storage, _dir) = storage_with_bucket(10);
        let stored = store_chunk_of_len(&storage, 1, 10);
        let before = storage.get_bucket(BUCKET).unwrap().unwrap();
        assert_eq!(before.node_count, 1);

        let data = vec![2u8; 10];
        let err = storage
            .store_node(
                BUCKET,
                H256::repeat_byte(0xaa),
                ChunkTreeNode::Chunk(data.clone()),
            )
            .unwrap_err();
        assert!(matches!(err, Error::InvalidHash { .. }), "got {err:?}");

        let missing = H256::repeat_byte(0x42);
        let err = storage
            .store_node(
                BUCKET,
                storage_primitives::hash_children(stored, missing),
                ChunkTreeNode::Internal {
                    left: stored,
                    right: missing,
                },
            )
            .unwrap_err();
        assert!(matches!(err, Error::ChildrenMissing(_)), "got {err:?}");

        let err = storage
            .store_node(BUCKET, hash_leaf(&data), ChunkTreeNode::Chunk(data))
            .unwrap_err();
        assert!(matches!(err, Error::QuotaExceeded { .. }), "got {err:?}");

        assert_eq!(storage.get_bucket(BUCKET).unwrap().unwrap(), before);
    }

    #[test]
    fn stats_sum_the_bucket_counters_and_match_the_stored_nodes() {
        let (storage, _dir) = storage_with_bucket(COMMIT_QUOTA);
        storage.init_bucket(2, COMMIT_QUOTA).unwrap();
        let [a, b] = [1, 2].map(|byte| store_chunk_of_len(&storage, byte, 10));
        store_parent(&storage, a, b);
        // Bucket 2 repeats a node bucket 1 stored, and adds one of its own.
        let shared = vec![1u8; 10];
        let own = vec![9u8; 20];
        for data in [shared, own] {
            storage
                .store_node(2, hash_leaf(&data), ChunkTreeNode::Chunk(data))
                .unwrap();
        }

        let stats = storage.stats();

        let stored_keys = {
            let cf = storage.db.cf_handle(CF_NODES).unwrap();
            storage
                .db
                .iterator_cf(&cf, rocksdb::IteratorMode::Start)
                .count() as u64
        };
        assert_eq!(
            stats,
            StorageStats {
                buckets: 2,
                nodes: 4,
                bytes: 10 + 10 + 64 + 20,
            }
        );
        assert_eq!(stats.nodes, stored_keys);
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
            storage.get_node(&chunk_hash).unwrap(),
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
            storage.get_node(&internal_hash).unwrap(),
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
            "1ea55ee1bc91a98868211d99dbe4ddf1e42b21655f5d76360cd09f0bfc53535a"
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
    fn bucket_with_leaves(storage: &DiskStorage, bucket_id: BucketId, n: u8) -> Vec<H256> {
        storage.init_bucket(bucket_id, u64::MAX).unwrap();
        (0..n)
            .map(|i| {
                let data = vec![i; 8];
                let hash = hash_leaf(&data);
                storage
                    .store_node(bucket_id, hash, ChunkTreeNode::Chunk(data))
                    .unwrap();
                storage.commit(bucket_id, vec![hash]).unwrap();
                hash
            })
            .collect()
    }

    /// Overwrite the raw record at `key` in column family `cf` with bytes
    /// that can't be decoded, standing in for a disk-level corruption
    /// (partial write, bit flip) rather than the record simply never having
    /// been written.
    fn corrupt(storage: &DiskStorage, cf: &str, key: &[u8]) {
        let cf = storage.db.cf_handle(cf).unwrap();
        storage
            .db
            .put_cf(&cf, key, b"not a valid bincode record")
            .unwrap();
    }

    /// Remove a node's raw record entirely, standing in for the node
    /// genuinely never having been written (or having been pruned) rather
    /// than corrupted - `store_node`'s children-exist check would otherwise
    /// make it impossible to reference a missing child through the public
    /// API alone.
    fn delete_node(storage: &DiskStorage, hash: H256) {
        let cf = storage.db.cf_handle(CF_NODES).unwrap();
        storage.db.delete_cf(&cf, hash.as_bytes()).unwrap();
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
        let (storage, _dir) = storage_with_bucket(COMMIT_QUOTA);

        corrupt(&storage, CF_BUCKETS, &BUCKET.to_le_bytes());

        let err = storage.get_mmr_proof(BUCKET, 0).unwrap_err();
        assert!(
            matches!(err, Error::Serialization(_)),
            "expected Serialization, got {err:?}"
        );
    }

    /// A bucket holding a root over two stored chunks. Returns the root and
    /// the first chunk's hash.
    fn two_chunk_tree() -> (DiskStorage, TempDir, H256, H256) {
        let (storage, dir) = storage_with_bucket(COMMIT_QUOTA);
        let c0 = store_chunk_of_len(&storage, 0, 7);
        let c1 = store_chunk_of_len(&storage, 1, 7);
        let root = store_parent(&storage, c0, c1);
        (storage, dir, root, c0)
    }

    #[test]
    fn get_chunk_at_index_past_the_end_of_a_healthy_root_reports_absence() {
        // Stands for: the client asked for a chunk index that was never
        // part of this data root.
        let (storage, _dir) = storage_with_bucket(COMMIT_QUOTA);
        let chunk_hash = store_chunk_of_len(&storage, 1, 10);

        let err = storage.get_chunk_at_index(chunk_hash, 5).unwrap_err();
        assert!(matches!(err, Error::ChunkIndexOutOfRange { .. }));
    }

    #[test]
    fn get_chunk_at_index_with_a_corrupt_node_mid_traversal_reports_a_backend_failure_not_a_short_list(
    ) {
        // Stands for: an operator's disk has a corrupted record for one
        // chunk of a multi-chunk file. A failed read mid-traversal must not
        // shorten the chunk list: asking for a chunk that is still there
        // would trip the "index past the end" guard and be reported as
        // absent, a false "you will be slashed" signal.
        let (storage, _dir, root, c0) = two_chunk_tree();

        corrupt(&storage, CF_NODES, c0.as_bytes());

        // Chunk index 1 (chunk-1) is healthy and was never actually removed,
        // but a traversal that silently skips the unreadable c0 would only
        // see one chunk and report index 1 as past the end.
        let err = storage.get_chunk_at_index(root, 1).unwrap_err();
        assert!(
            matches!(err, Error::Serialization(_)),
            "expected Serialization, got {err:?}"
        );
    }

    #[test]
    fn get_chunk_at_index_with_a_missing_node_mid_traversal_reports_node_not_found() {
        // Stands for: a node the tree references was never written (or was
        // pruned) rather than corrupted. Traversal must surface the gap, not
        // treat the node as an empty subtree.
        let (storage, _dir, root, c0) = two_chunk_tree();

        // c0 is now gone, e.g. pruned, even though the root still references it.
        delete_node(&storage, c0);

        let err = storage.get_chunk_at_index(root, 1).unwrap_err();
        assert!(
            matches!(err, Error::NodeNotFound(_)),
            "expected NodeNotFound, got {err:?}"
        );
    }

    #[test]
    fn proof_for_a_missing_leaf_is_leaf_not_found() {
        let dir = TempDir::new().unwrap();
        let storage = DiskStorage::new(dir.path()).unwrap();
        bucket_with_leaves(&storage, 1, 0);
        bucket_with_leaves(&storage, 2, 4);

        assert!(matches!(
            storage.get_mmr_proof(1, 0),
            Err(Error::LeafNotFound { .. })
        ));
        assert!(matches!(
            storage.get_mmr_proof(2, 4),
            Err(Error::LeafNotFound { .. })
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
        let expected = live_leaves(&storage, 1)[2].clone();

        let Commitment {
            mmr_root: post_prune_root,
            start_seq,
            ..
        } = storage.delete_before(1, 1).unwrap();
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
    fn older_commitment_stays_provable_after_erase() {
        let dir = TempDir::new().unwrap();
        let storage = DiskStorage::new(dir.path()).unwrap();
        let roots = bucket_with_leaves(&storage, 1, 2);
        let cited = Commitment {
            mmr_root: storage.get_bucket(1).unwrap().unwrap().mmr_root,
            start_seq: 0,
            leaf_count: 2,
        };

        storage.delete_before(1, 1).unwrap();

        // Pruned leaves remain in the stash: the provider must stay able to
        // prove the old commitment while it is still challengeable.
        let proof = storage.get_mmr_proof_for_commitment(1, &cited, 0).unwrap();
        assert!(storage_primitives::verify_mmr_proof(proof, 0, &cited));

        // Erasure deletes leaf 0's nodes but keeps the leaf: the cited
        // commitment also covers leaf 1, which the provider still stores, and
        // a proof for leaf 1 needs leaf 0 to rebuild the cited root.
        storage.erase_pruned_range(1, 0).unwrap();
        assert!(storage.get_node(&roots[0]).unwrap().is_none());
        let proof = storage.get_mmr_proof_for_commitment(1, &cited, 1).unwrap();
        assert!(storage_primitives::verify_mmr_proof(proof, 1, &cited));
    }

    /// Store a two-chunk file: one full chunk, one short last chunk, and the
    /// internal node over them. Returns (root, chunk hashes).
    fn store_two_chunk_tree(
        storage: &DiskStorage,
        bucket_id: BucketId,
        tag: u8,
    ) -> (H256, [H256; 2]) {
        let chunk_a = vec![tag; DEFAULT_CHUNK_SIZE as usize];
        let chunk_b = vec![tag, 2, 2, 2];
        let hash_a = hash_leaf(&chunk_a);
        let hash_b = hash_leaf(&chunk_b);
        storage
            .store_node(bucket_id, hash_a, ChunkTreeNode::Chunk(chunk_a))
            .unwrap();
        storage
            .store_node(bucket_id, hash_b, ChunkTreeNode::Chunk(chunk_b))
            .unwrap();
        let root = build_merkle_tree(storage, bucket_id, &[hash_a, hash_b]).unwrap();
        (root, [hash_a, hash_b])
    }

    const FULL_CHUNK: u64 = DEFAULT_CHUNK_SIZE as u64;

    #[test]
    fn commit_increments_refcounts() {
        let (storage, _dir) = storage_with_bucket(u64::MAX);
        let (root, [hash_a, hash_b]) = store_two_chunk_tree(&storage, BUCKET, 0);

        // Stored but uncommitted: charge record exists with count 0.
        assert_eq!(storage.refcount_entry(&root).unwrap(), Some((0, 1, 64)));
        assert_eq!(
            storage.refcount_entry(&hash_a).unwrap(),
            Some((0, 1, FULL_CHUNK))
        );

        storage.commit(BUCKET, vec![root]).unwrap();
        assert_eq!(storage.refcount_entry(&root).unwrap(), Some((1, 1, 64)));
        assert_eq!(
            storage.refcount_entry(&hash_a).unwrap(),
            Some((1, 1, FULL_CHUNK))
        );
        assert_eq!(storage.refcount_entry(&hash_b).unwrap(), Some((1, 1, 4)));

        // Committing the same root as a second leaf counts again.
        storage.commit(BUCKET, vec![root]).unwrap();
        assert_eq!(storage.refcount_entry(&root).unwrap(), Some((2, 1, 64)));
        assert_eq!(
            storage.refcount_entry(&hash_a).unwrap(),
            Some((2, 1, FULL_CHUNK))
        );
    }

    #[test]
    fn shared_chunk_across_buckets_counts_twice_charges_once() {
        let dir = TempDir::new().unwrap();
        let storage = DiskStorage::new(dir.path()).unwrap();
        storage.init_bucket(1, u64::MAX).unwrap();
        storage.init_bucket(2, u64::MAX).unwrap();

        let data = vec![9u8; 16];
        let hash = hash_leaf(&data);
        storage
            .store_node(1, hash, ChunkTreeNode::Chunk(data.clone()))
            .unwrap();
        // Second bucket stores the same content: global dedup, no re-charge.
        storage
            .store_node(2, hash, ChunkTreeNode::Chunk(data))
            .unwrap();
        assert_eq!(storage.get_bucket(2).unwrap().unwrap().used_bytes, 0);

        storage.commit(1, vec![hash]).unwrap();
        storage.commit(2, vec![hash]).unwrap();
        // Two references, still charged to the first storer.
        assert_eq!(storage.refcount_entry(&hash).unwrap(), Some((2, 1, 16)));
    }

    #[test]
    fn commit_fails_on_missing_subtree_node() {
        let (storage, _dir) = storage_with_bucket(u64::MAX);
        let (root, [hash_a, _]) = store_two_chunk_tree(&storage, BUCKET, 0);

        // Simulate a vanished child (e.g. erased between upload and commit).
        let cf = storage.db.cf_handle(CF_NODES).unwrap();
        storage.db.delete_cf(&cf, hash_a.as_bytes()).unwrap();

        let err = storage.commit(BUCKET, vec![root]).unwrap_err();
        assert!(matches!(err, Error::NodeNotFound(_)), "got {err:?}");
        // Nothing was committed: no refcount increments applied.
        assert_eq!(storage.refcount_entry(&root).unwrap(), Some((0, 1, 64)));
    }

    #[test]
    fn delete_before_keeps_used_bytes_until_erasure() {
        let (storage, _dir) = storage_with_bucket(u64::MAX);
        let (root, _) = store_two_chunk_tree(&storage, BUCKET, 0);
        storage.commit(BUCKET, vec![root]).unwrap();
        let used = storage.get_bucket(BUCKET).unwrap().unwrap().used_bytes;
        assert_eq!(used, FULL_CHUNK + 4 + 64);

        storage.delete_before(BUCKET, 1).unwrap();
        // Quota tracks disk actually consumed: the stash still holds the
        // bytes, so nothing is credited at prune time.
        assert_eq!(
            storage.get_bucket(BUCKET).unwrap().unwrap().used_bytes,
            used
        );

        let outcome = storage.erase_pruned_range(BUCKET, 0).unwrap();
        assert_eq!(outcome.nodes_deleted, 3);
        assert_eq!(outcome.bytes_freed, used);
        assert_eq!(storage.get_bucket(BUCKET).unwrap().unwrap().used_bytes, 0);
        assert!(storage.get_node(&root).unwrap().is_none());
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
        let hash = hash_leaf(&data);
        storage
            .store_node(1, hash, ChunkTreeNode::Chunk(data.clone()))
            .unwrap();
        storage
            .store_node(2, hash, ChunkTreeNode::Chunk(data))
            .unwrap();
        storage.commit(1, vec![hash]).unwrap();
        storage.commit(2, vec![hash]).unwrap();
        assert_eq!(storage.get_bucket(1).unwrap().unwrap().used_bytes, 16);
        assert_eq!(storage.get_bucket(2).unwrap().unwrap().used_bytes, 0);

        // Bucket 1 deletes: the node remains (bucket 2 still references it)
        // and nothing is credited, because the bytes are still on disk.
        storage.delete_before(1, 1).unwrap();
        let outcome = storage.erase_pruned_range(1, 0).unwrap();
        assert_eq!(outcome, EraseOutcome::default());
        assert!(storage.get_node(&hash).unwrap().is_some());
        assert_eq!(storage.refcount_entry(&hash).unwrap(), Some((1, 1, 16)));
        assert_eq!(storage.get_bucket(1).unwrap().unwrap().used_bytes, 16);

        // Bucket 2 deletes too: the last reference goes, the node is erased,
        // and the credit goes to the bucket that was charged (bucket 1).
        storage.delete_before(2, 1).unwrap();
        let outcome = storage.erase_pruned_range(2, 0).unwrap();
        assert_eq!(outcome.nodes_deleted, 1);
        assert_eq!(outcome.bytes_freed, 16);
        assert!(storage.get_node(&hash).unwrap().is_none());
        assert_eq!(storage.get_bucket(1).unwrap().unwrap().used_bytes, 0);
    }

    /// Bucket 1 commits X, deletes it (X stays stashed with count 1) and
    /// uploads X again. Returns X.
    fn stash_and_reupload(storage: &DiskStorage) -> H256 {
        let data = vec![3u8; 16];
        let hash = hash_leaf(&data);
        storage
            .store_node(1, hash, ChunkTreeNode::Chunk(data.clone()))
            .unwrap();
        storage.commit(1, vec![hash]).unwrap();
        storage.delete_before(1, 1).unwrap();
        storage
            .store_node(1, hash, ChunkTreeNode::Chunk(data))
            .unwrap();
        hash
    }

    #[test]
    fn commit_fails_when_an_erase_deletes_a_walked_node() {
        let dir = TempDir::new().unwrap();
        let storage = DiskStorage::new(dir.path()).unwrap();
        storage.init_bucket(1, u64::MAX).unwrap();
        let hash = stash_and_reupload(&storage);
        let before = storage.get_bucket(1).unwrap().unwrap();

        // The erase runs between commit's walk and its transaction.
        let walk = storage.walk_commit_roots(vec![hash]).unwrap();
        let erased = storage.erase_pruned_range(1, 0).unwrap();
        assert_eq!(erased.nodes_deleted, 1);
        let result = storage.apply_commit(1, walk);

        assert!(matches!(result, Err(Error::NodeNotFound(h)) if h == hash));
        let after = storage.get_bucket(1).unwrap().unwrap();
        assert_eq!(after.leaf_count, 0);
        assert_eq!(after.mmr_root, before.mmr_root);
        assert_eq!(storage.refcount_entry(&hash).unwrap(), None);
    }

    #[test]
    fn erase_keeps_a_node_committed_again_before_it() {
        let dir = TempDir::new().unwrap();
        let storage = DiskStorage::new(dir.path()).unwrap();
        storage.init_bucket(1, u64::MAX).unwrap();
        let hash = stash_and_reupload(&storage);

        storage.commit(1, vec![hash]).unwrap();
        let erased = storage.erase_pruned_range(1, 0).unwrap();

        assert_eq!(erased, EraseOutcome::default());
        assert!(storage.get_node(&hash).unwrap().is_some());
        assert_eq!(storage.refcount_entry(&hash).unwrap(), Some((1, 1, 16)));
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
        assert_eq!(second, EraseOutcome::default());
    }

    #[test]
    fn erase_counts_every_leaf_of_the_range_as_erased() {
        let dir = TempDir::new().unwrap();
        let storage = DiskStorage::new(dir.path()).unwrap();
        bucket_with_leaves(&storage, 1, 3);
        storage.delete_before(1, 2).unwrap();
        assert_eq!(
            pruned_range(&storage, 1, 0),
            Some(PrunedRange {
                first_seq: 0,
                new_start_seq: 2,
                erased_leaves: 0
            })
        );

        storage.erase_pruned_range(1, 0).unwrap();

        assert_eq!(pruned_range(&storage, 1, 0), None);
    }

    /// The walk stops once the visit count passes the budget, after visiting
    /// exactly the budget.
    #[test]
    fn canonical_walk_stops_at_the_visit_budget() {
        let (storage, _dir) = storage_with_bucket(u64::MAX);
        let root = diamond_tree(&storage, BUCKET, 4);
        let storage = storage.with_max_tree_walk_visits(10);

        let mut visited = Vec::new();
        let result = storage.walk_canonical_tree(root, 16, &mut |hash| visited.push(hash));

        assert!(matches!(result, Err(Error::TreeTooLarge { max_nodes: 10 })));
        assert_eq!(visited.len(), 10);
        assert_eq!(visited[0], root);
    }

    /// A chunk where the shape requires an internal node fails the walk
    /// before the chunk is visited.
    #[test]
    fn canonical_walk_rejects_a_chunk_where_the_shape_needs_an_internal_node() {
        let (storage, _dir) = storage_with_bucket(u64::MAX);
        let chunk = store_chunk(&storage, 1);

        let mut visited = Vec::new();
        let result = storage.walk_canonical_tree(chunk, 2, &mut |hash| visited.push(hash));

        assert!(matches!(result, Err(Error::NonCanonicalTree(_))));
        assert!(visited.is_empty());
    }

    /// 16 chunks under 15 internal-node visits: the canonical walk reads
    /// the internal nodes only and still visits every chunk.
    #[test]
    fn canonical_walk_reads_no_chunk() {
        let (storage, _dir) = storage_with_bucket(u64::MAX);
        let root = diamond_tree(&storage, BUCKET, 4);
        let chunk = hash_leaf(&vec![0u8; DEFAULT_CHUNK_SIZE as usize]);

        let reads = CountingReads::new(&storage);
        let mut visited = Vec::new();
        reads
            .walk_canonical_tree(root, 16, &mut |hash| visited.push(hash))
            .unwrap();

        assert_eq!(reads.get_node_calls.load(Ordering::Relaxed), 15);
        assert_eq!(visited.len(), 31);
        assert_eq!(visited[0], root);
        assert_eq!(visited.iter().filter(|hash| **hash == chunk).count(), 16);
    }

    /// The erase walk counts one decrement per path, like commit's
    /// increments.
    #[test]
    fn erase_walk_counts_every_path() {
        let (storage, _dir) = storage_with_bucket(u64::MAX);
        let root = diamond_tree(&storage, BUCKET, 4);
        storage.commit(BUCKET, vec![root]).unwrap();
        storage.delete_before(BUCKET, 1).unwrap();
        let range = pruned_range(&storage, BUCKET, 0).unwrap();
        let chunk = hash_leaf(&vec![0u8; DEFAULT_CHUNK_SIZE as usize]);

        let walk = storage.walk_erase_range(1, &range).unwrap();

        assert!(walk.failures.is_empty());
        assert_eq!(walk.decrements.get(&root), Some(&1));
        assert_eq!(walk.decrements.get(&chunk), Some(&16));
        assert_eq!(walk.decrements.values().sum::<u64>(), 31);
    }

    /// Commit a tree of two full chunks and one 10-byte chunk to `BUCKET`
    /// and stash it. Returns the root and the three chunks.
    fn stash_three_chunk_tree(storage: &DiskStorage) -> (H256, [H256; 3]) {
        let [c0, c1] = [1, 2].map(|b| store_chunk(storage, b));
        let c2 = store_chunk_of_len(storage, 3, 10);
        let root = build_merkle_tree(storage, BUCKET, &[c0, c1, c2]).unwrap();
        storage.commit(BUCKET, vec![root]).unwrap();
        storage.delete_before(BUCKET, 1).unwrap();
        (root, [c0, c1, c2])
    }

    /// A tree whose chunk count is not a power of two and whose last chunk is
    /// short: the erase removes every node commit counted.
    #[test]
    fn erase_removes_an_uneven_tree() {
        let (storage, _dir) = storage_with_bucket(u64::MAX);
        let (root, chunks) = stash_three_chunk_tree(&storage);

        let outcome = storage.erase_pruned_range(BUCKET, 0).unwrap();

        assert_eq!(outcome.nodes_deleted, 5);
        assert_eq!(outcome.bytes_freed, 2 * FULL_CHUNK + 10 + 2 * 64);
        for hash in chunks.into_iter().chain([root]) {
            assert!(storage.get_node(&hash).unwrap().is_none());
            assert_eq!(storage.refcount_entry(&hash).unwrap(), None);
        }
        let bucket = storage.get_bucket(BUCKET).unwrap().unwrap();
        assert_eq!((bucket.used_bytes, bucket.node_count), (0, 0));
    }

    /// Another erase of the range runs between this call's snapshot and its
    /// walk, so the walk stops at a deleted node. The transaction finds the
    /// range gone and changes nothing.
    #[test]
    fn erase_applied_after_another_erase_is_a_no_op() {
        let (storage, _dir) = storage_with_bucket(u64::MAX);
        stash_three_chunk_tree(&storage);
        let range = pruned_range(&storage, BUCKET, 0).unwrap();

        let first = storage.erase_pruned_range(BUCKET, 0).unwrap();
        assert_eq!(first.nodes_deleted, 5);
        let walk = storage.walk_erase_range(1, &range).unwrap();
        assert!(walk.decrements.is_empty(), "walk stops at the deleted root");
        assert!(matches!(walk.failures[..], [Error::NodeNotFound(_)]));
        let before = storage.get_bucket(BUCKET).unwrap().unwrap();
        let second = storage.apply_erase(BUCKET, &range, walk).unwrap();

        assert_eq!(second, EraseOutcome::default());
        assert_eq!(storage.get_bucket(BUCKET).unwrap().unwrap(), before);
    }

    /// A commit between the walk and the transaction adds a reference to a
    /// walked node: the erase removes only its own reference and keeps the
    /// node.
    #[test]
    fn erase_keeps_a_node_committed_again_after_the_walk() {
        let dir = TempDir::new().unwrap();
        let storage = DiskStorage::new(dir.path()).unwrap();
        let roots = bucket_with_leaves(&storage, 1, 2);
        storage.delete_before(1, 1).unwrap();
        let range = pruned_range(&storage, 1, 0).unwrap();
        let walk = storage.walk_erase_range(1, &range).unwrap();

        storage.commit(1, vec![roots[0]]).unwrap();
        let outcome = storage.apply_erase(1, &range, walk).unwrap();

        assert_eq!(outcome, EraseOutcome::default());
        assert!(storage.get_node(&roots[0]).unwrap().is_some());
        assert_eq!(storage.refcount_entry(&roots[0]).unwrap(), Some((1, 1, 8)));
    }

    /// Writes to the bucket between the walk and the transaction do not
    /// change what the erase removes.
    #[test]
    fn erase_applies_after_bucket_writes_since_the_walk() {
        let dir = TempDir::new().unwrap();
        let storage = DiskStorage::new(dir.path()).unwrap();
        let roots = bucket_with_leaves(&storage, 1, 3);
        storage.delete_before(1, 1).unwrap();
        let range = pruned_range(&storage, 1, 0).unwrap();
        let walk = storage.walk_erase_range(1, &range).unwrap();

        storage.delete_before(1, 2).unwrap();
        let data = vec![9u8; 8];
        let hash = hash_leaf(&data);
        storage
            .store_node(1, hash, ChunkTreeNode::Chunk(data))
            .unwrap();
        storage.commit(1, vec![hash]).unwrap();
        let outcome = storage.apply_erase(1, &range, walk).unwrap();

        assert_eq!(outcome.nodes_deleted, 1);
        assert!(storage.get_node(&roots[0]).unwrap().is_none());
        assert!(storage.get_node(&roots[1]).unwrap().is_some());
        let ranges = storage.pruned_ranges(1).unwrap();
        assert_eq!(ranges.len(), 1);
        assert_eq!((ranges[0].first_seq, ranges[0].end_seq), (1, 2));
        assert_eq!(storage.get_bucket(1).unwrap().unwrap().leaf_count, 2);
    }

    /// Commit `n` single-chunk leaves, one per byte value from 0, and stash
    /// the first `pruned` of them.
    fn stash_leaves(storage: &DiskStorage, n: u8, pruned: u64) -> Vec<H256> {
        let roots = bucket_with_leaves(storage, 1, n);
        storage.delete_before(1, pruned).unwrap();
        roots
    }

    /// The bucket row has a fixed size, so writes to a bucket with a long
    /// history cost the same as to a new one.
    #[test]
    fn bucket_row_size_does_not_grow_with_history() {
        let (storage, _dir) = storage_with_bucket(u64::MAX);
        let empty_row_len = storage.get_bucket(BUCKET).unwrap().unwrap().encode().len();
        let mut total_leaves = 0u64;
        for round in 0..10u64 {
            for i in 0..100u64 {
                let data = (round * 100 + i).to_le_bytes().to_vec();
                let hash = hash_leaf(&data);
                storage
                    .store_node(BUCKET, hash, ChunkTreeNode::Chunk(data))
                    .unwrap();
                storage.commit(BUCKET, vec![hash]).unwrap();
                total_leaves += 1;
            }
            let mmr_root = storage
                .delete_before(BUCKET, total_leaves)
                .unwrap()
                .mmr_root;
            storage
                .attach_deletion_receipt(
                    BUCKET,
                    signed_receipt(BUCKET, mmr_root, total_leaves, &admin_pair()),
                )
                .unwrap();
            storage
                .erase_pruned_range(BUCKET, total_leaves - 100)
                .unwrap();
        }

        let row = storage.get_bucket(BUCKET).unwrap().unwrap();
        assert_eq!(row.encode().len(), empty_row_len);

        // With 1,000 retained leaves the row only adds the MMR peaks, at most
        // 64 hashes.
        for i in 1_000..2_000u64 {
            let data = i.to_le_bytes().to_vec();
            let hash = hash_leaf(&data);
            storage
                .store_node(BUCKET, hash, ChunkTreeNode::Chunk(data))
                .unwrap();
            storage.commit(BUCKET, vec![hash]).unwrap();
        }
        let row = storage.get_bucket(BUCKET).unwrap().unwrap();
        assert_eq!(row.peaks.len(), 1_000u32.count_ones() as usize);
        assert!(row.encode().len() <= empty_row_len + 64 * 32);
    }

    /// `store_node` runs once per node, so it reads and writes only the
    /// fixed-size row.
    #[test]
    fn store_node_does_not_read_any_leaf() {
        let dir = TempDir::new().unwrap();
        let storage = DiskStorage::new(dir.path()).unwrap();
        bucket_with_leaves(&storage, 1, 2);
        corrupt(&storage, CF_LEAVES, &seq_key(1, 0));
        corrupt(&storage, CF_LEAVES, &seq_key(1, 1));

        let data = vec![9u8; 8];
        storage
            .store_node(1, hash_leaf(&data), ChunkTreeNode::Chunk(data))
            .unwrap();

        assert_eq!(storage.get_bucket(1).unwrap().unwrap().node_count, 3);
    }

    fn raw(storage: &DiskStorage, cf: &str, key: &[u8]) -> Option<Vec<u8>> {
        storage
            .db
            .get_cf(&storage.db.cf_handle(cf).unwrap(), key)
            .unwrap()
    }

    #[test]
    fn commit_writes_one_leaf_record_and_reads_no_old_leaf() {
        let dir = TempDir::new().unwrap();
        let storage = DiskStorage::new(dir.path()).unwrap();
        bucket_with_leaves(&storage, 1, 3);
        for seq in 0..3 {
            assert!(
                raw(&storage, CF_LEAVES, &seq_key(1, seq)).is_some(),
                "leaf {seq}"
            );
        }

        corrupt(&storage, CF_LEAVES, &seq_key(1, 0));
        let data = vec![9u8; 8];
        let hash = hash_leaf(&data);
        storage
            .store_node(1, hash, ChunkTreeNode::Chunk(data))
            .unwrap();
        storage.commit(1, vec![hash]).unwrap();

        assert!(raw(&storage, CF_LEAVES, &seq_key(1, 3)).is_some());
        assert_eq!(raw(&storage, CF_LEAVES, &seq_key(1, 4)), None);
    }

    #[test]
    fn prune_writes_one_range_record_and_keeps_every_leaf() {
        let dir = TempDir::new().unwrap();
        let storage = DiskStorage::new(dir.path()).unwrap();
        stash_leaves(&storage, 3, 2);

        // SCALE(PrunedRange): first_seq = 0, new_start_seq = 2, erased_leaves = 0
        assert_eq!(
            raw(&storage, CF_PRUNED_RANGES, &seq_key(1, 0)).map(|v| array_bytes::bytes2hex("", v)),
            Some("000000000000000002000000000000000000000000000000".to_string())
        );
        for seq in 0..3 {
            assert!(
                raw(&storage, CF_LEAVES, &seq_key(1, seq)).is_some(),
                "leaf {seq}"
            );
        }
    }

    #[test]
    fn a_receipt_is_stored_under_its_new_start_seq() {
        let dir = TempDir::new().unwrap();
        let storage = DiskStorage::new(dir.path()).unwrap();
        stash_leaves(&storage, 3, 2);

        let receipt = receipt_for(2);
        storage.attach_deletion_receipt(1, receipt.clone()).unwrap();

        assert_eq!(
            raw(&storage, CF_DELETION_RECEIPTS, &seq_key(1, 2)),
            Some(receipt.encode())
        );
    }

    fn admin_pair() -> sp_core::sr25519::Pair {
        sp_core::sr25519::Pair::from_seed(&[7u8; 32])
    }

    /// A receipt for `bucket_id` that `signer` signed over the deletion
    /// commitment, the way the pallet's `Deleted` defense verifies it.
    fn signed_receipt(
        bucket_id: BucketId,
        mmr_root: H256,
        new_start_seq: u64,
        signer: &sp_core::sr25519::Pair,
    ) -> DeletionReceipt {
        let payload = storage_primitives::CommitmentPayload::new(
            bucket_id,
            Commitment {
                mmr_root,
                start_seq: new_start_seq,
                leaf_count: 0,
            },
        );
        DeletionReceipt {
            mmr_root,
            new_start_seq,
            admin: signer.public().0.into(),
            signature: signer.sign(&payload.encode()).into(),
        }
    }

    /// A receipt for bucket 1 signed by the test admin.
    fn receipt_for(new_start_seq: u64) -> DeletionReceipt {
        signed_receipt(1, H256::repeat_byte(1), new_start_seq, &admin_pair())
    }

    fn forged_receipt(new_start_seq: u64) -> DeletionReceipt {
        DeletionReceipt {
            signature: sp_runtime::MultiSignature::Sr25519(sp_core::sr25519::Signature::from(
                [0u8; 64],
            )),
            ..receipt_for(new_start_seq)
        }
    }

    #[test]
    fn a_forged_receipt_is_rejected_and_not_stored() {
        let dir = TempDir::new().unwrap();
        let storage = DiskStorage::new(dir.path()).unwrap();
        stash_leaves(&storage, 3, 2);

        let err = storage
            .attach_deletion_receipt(1, forged_receipt(2))
            .unwrap_err();

        assert!(
            matches!(err, Error::InvalidDeletionReceipt(1)),
            "got {err:?}"
        );
        assert_eq!(raw(&storage, CF_DELETION_RECEIPTS, &seq_key(1, 2)), None);
        assert!(!storage.pruned_ranges(1).unwrap()[0].has_receipt);
    }

    #[test]
    fn a_receipt_signed_by_another_key_is_rejected() {
        let dir = TempDir::new().unwrap();
        let storage = DiskStorage::new(dir.path()).unwrap();
        stash_leaves(&storage, 3, 2);
        let other = sp_core::sr25519::Pair::from_seed(&[8u8; 32]);
        let receipt = DeletionReceipt {
            admin: admin_pair().public().0.into(),
            ..signed_receipt(1, H256::repeat_byte(1), 2, &other)
        };

        let err = storage.attach_deletion_receipt(1, receipt).unwrap_err();

        assert!(
            matches!(err, Error::InvalidDeletionReceipt(1)),
            "got {err:?}"
        );
    }

    #[test]
    fn a_receipt_signed_for_another_bucket_or_range_is_rejected() {
        let dir = TempDir::new().unwrap();
        let storage = DiskStorage::new(dir.path()).unwrap();
        stash_leaves(&storage, 3, 2);
        let admin = admin_pair();
        let root = H256::repeat_byte(1);

        // Signed for bucket 2.
        let other_bucket = signed_receipt(2, root, 2, &admin);
        // Signed for new_start_seq 1, claims 2.
        let other_range = DeletionReceipt {
            new_start_seq: 2,
            ..signed_receipt(1, root, 1, &admin)
        };
        // Signed over another root.
        let other_root = DeletionReceipt {
            mmr_root: H256::repeat_byte(9),
            ..signed_receipt(1, root, 2, &admin)
        };

        for receipt in [other_bucket, other_range, other_root] {
            let err = storage.attach_deletion_receipt(1, receipt).unwrap_err();
            assert!(
                matches!(err, Error::InvalidDeletionReceipt(1)),
                "got {err:?}"
            );
        }
    }

    #[test]
    fn a_forged_replacement_keeps_the_stored_receipt() {
        let dir = TempDir::new().unwrap();
        let storage = DiskStorage::new(dir.path()).unwrap();
        stash_leaves(&storage, 3, 2);
        let receipt = receipt_for(2);
        storage.attach_deletion_receipt(1, receipt.clone()).unwrap();
        storage.erase_pruned_range(1, 0).unwrap();

        let err = storage
            .attach_deletion_receipt(1, forged_receipt(2))
            .unwrap_err();

        assert!(
            matches!(err, Error::InvalidDeletionReceipt(1)),
            "got {err:?}"
        );
        assert_eq!(
            storage.deletion_receipt_covering(1, 1).unwrap(),
            Some(receipt)
        );
    }

    #[test]
    fn a_receipt_needs_a_pruned_range_ending_at_its_new_start_seq() {
        let dir = TempDir::new().unwrap();
        let storage = DiskStorage::new(dir.path()).unwrap();
        stash_leaves(&storage, 3, 2);

        let err = storage
            .attach_deletion_receipt(1, receipt_for(1))
            .unwrap_err();

        assert!(
            matches!(err, Error::InvalidStartSeq { requested: 1, .. }),
            "got {err:?}"
        );
    }

    #[test]
    fn a_receipt_is_replaced_after_its_range_is_erased() {
        let dir = TempDir::new().unwrap();
        let storage = DiskStorage::new(dir.path()).unwrap();
        stash_leaves(&storage, 3, 2);
        storage.attach_deletion_receipt(1, receipt_for(2)).unwrap();
        storage.erase_pruned_range(1, 0).unwrap();

        let replacement = signed_receipt(1, H256::repeat_byte(2), 2, &admin_pair());
        storage
            .attach_deletion_receipt(1, replacement.clone())
            .unwrap();

        assert_eq!(
            storage.deletion_receipt_covering(1, 1).unwrap(),
            Some(replacement)
        );
    }

    #[test]
    fn a_condemned_bucket_rejects_a_receipt() {
        let dir = TempDir::new().unwrap();
        let storage = DiskStorage::new(dir.path()).unwrap();
        stash_leaves(&storage, 3, 2);
        storage.condemn_bucket(1).unwrap();

        let err = storage
            .attach_deletion_receipt(1, receipt_for(2))
            .unwrap_err();

        assert!(matches!(err, Error::BucketCondemned(1)), "got {err:?}");
    }

    #[test]
    fn total_size_restarts_when_every_leaf_is_pruned() {
        let dir = TempDir::new().unwrap();
        let storage = DiskStorage::new(dir.path()).unwrap();
        bucket_with_leaves(&storage, 1, 2);
        storage.delete_before(1, 2).unwrap();
        let data = vec![9u8; 8];
        let hash = hash_leaf(&data);
        storage
            .store_node(1, hash, ChunkTreeNode::Chunk(data))
            .unwrap();
        storage.commit(1, vec![hash]).unwrap();

        let value = raw(&storage, CF_LEAVES, &seq_key(1, 2)).unwrap();
        assert_eq!(MmrLeaf::decode_all(&mut &value[..]).unwrap().total_size, 8);
    }

    #[test]
    fn row_peaks_match_the_mmr_over_the_live_leaves() {
        let dir = TempDir::new().unwrap();
        let storage = DiskStorage::new(dir.path()).unwrap();
        bucket_with_leaves(&storage, 1, 7);
        storage.delete_before(1, 3).unwrap();
        let live = || {
            let row = storage.get_bucket(1).unwrap().unwrap();
            (row.start_seq..row.start_seq + row.leaf_count)
                .map(|seq| {
                    let value = raw(&storage, CF_LEAVES, &seq_key(1, seq)).unwrap();
                    MmrLeaf::decode_all(&mut &value[..]).unwrap()
                })
                .collect::<Vec<_>>()
        };
        assert_eq!(
            storage.get_mmr_peaks(1).unwrap(),
            mmr::peaks(&live()).unwrap()
        );

        for byte in 100..105u8 {
            let data = vec![byte; 8];
            let hash = hash_leaf(&data);
            storage
                .store_node(1, hash, ChunkTreeNode::Chunk(data))
                .unwrap();
            storage.commit(1, vec![hash]).unwrap();
            assert_eq!(
                storage.get_mmr_peaks(1).unwrap(),
                mmr::peaks(&live()).unwrap()
            );
        }
        storage.delete_before(1, 9).unwrap();
        assert_eq!(
            storage.get_mmr_peaks(1).unwrap(),
            mmr::peaks(&live()).unwrap()
        );
    }

    #[test]
    fn a_failed_batch_keeps_the_batches_already_erased() {
        let dir = TempDir::new().unwrap();
        let storage = DiskStorage::new(dir.path())
            .unwrap()
            .with_erase_batch_nodes(1);
        let roots = stash_leaves(&storage, 3, 3);
        corrupt(&storage, CF_REFCOUNTS, roots[2].as_bytes());

        assert!(storage.erase_pruned_range(1, 0).is_err());

        assert!(storage.get_node(&roots[0]).unwrap().is_none());
        assert!(storage.get_node(&roots[1]).unwrap().is_none());
        assert!(storage.get_node(&roots[2]).unwrap().is_some());
        let bucket = storage.get_bucket(1).unwrap().unwrap();
        assert_eq!((bucket.used_bytes, bucket.node_count), (8, 1));
        assert_eq!(pruned_range(&storage, 1, 0).unwrap().erased_leaves, 2);
        assert_eq!(storage.pruned_ranges(1).unwrap().len(), 1);

        let cf = storage.db.cf_handle(CF_REFCOUNTS).unwrap();
        storage
            .db
            .put_cf(&cf, roots[2].as_bytes(), encode_refcount(1, 1, 8))
            .unwrap();
        let outcome = storage.erase_pruned_range(1, 0).unwrap();

        assert_eq!(outcome.nodes_deleted, 1);
        assert!(storage.get_node(&roots[2]).unwrap().is_none());
        assert!(storage.pruned_ranges(1).unwrap().is_empty());
    }

    #[test]
    fn batched_erase_ends_in_the_same_state_as_one_batch() {
        // Leaves 0 and 2 share one node, which bucket 2 also commits.
        fn run(budget: usize) -> Vec<Option<(u64, u64, u64)>> {
            let dir = TempDir::new().unwrap();
            let storage = DiskStorage::new(dir.path())
                .unwrap()
                .with_erase_batch_nodes(budget);
            let roots = bucket_with_leaves(&storage, 1, 2);
            storage.commit(1, vec![roots[0]]).unwrap();
            storage.init_bucket(2, u64::MAX).unwrap();
            storage.commit(2, vec![roots[0]]).unwrap();
            storage.delete_before(1, 3).unwrap();

            storage.erase_pruned_range(1, 0).unwrap();

            let mut state: Vec<_> = roots
                .iter()
                .map(|root| storage.refcount_entry(root).unwrap())
                .collect();
            for id in [1, 2] {
                let bucket = storage.get_bucket(id).unwrap().unwrap();
                state.push(Some((id, bucket.used_bytes, bucket.node_count)));
            }
            state
        }

        let one_batch = run(usize::MAX);
        assert_eq!(run(1), one_batch);
        // The shared node stays for bucket 2's leaf, the other node goes.
        assert_eq!(one_batch[..2], [Some((1, 1, 8)), None]);
    }

    #[test]
    fn a_commit_between_batches_succeeds_and_the_erase_completes() {
        let dir = TempDir::new().unwrap();
        let storage = DiskStorage::new(dir.path())
            .unwrap()
            .with_erase_batch_nodes(1);
        let roots = stash_leaves(&storage, 3, 2);

        let (first, more) = storage.erase_batch(1, 0).unwrap();
        assert_eq!((first.nodes_deleted, more), (1, true));
        assert!(storage.get_node(&roots[0]).unwrap().is_none());
        assert!(storage.get_node(&roots[1]).unwrap().is_some());

        let data = vec![9u8; 8];
        let hash = hash_leaf(&data);
        storage
            .store_node(1, hash, ChunkTreeNode::Chunk(data))
            .unwrap();
        storage.commit(1, vec![hash]).unwrap();
        let rest = storage.erase_pruned_range(1, 0).unwrap();

        assert_eq!(rest.nodes_deleted, 1);
        assert!(storage.get_node(&roots[1]).unwrap().is_none());
        assert!(storage.get_node(&roots[2]).unwrap().is_some());
        assert!(storage.get_node(&hash).unwrap().is_some());
    }

    #[test]
    fn condemned_bucket_row_goes_with_the_last_batch() {
        let dir = TempDir::new().unwrap();
        let storage = DiskStorage::new(dir.path())
            .unwrap()
            .with_erase_batch_nodes(1);
        bucket_with_leaves(&storage, 1, 2);
        storage.condemn_bucket(1).unwrap();

        let (_, more) = storage.erase_batch(1, 0).unwrap();
        assert!(more);
        assert!(storage.get_bucket(1).unwrap().is_some());

        let (_, more) = storage.erase_batch(1, 0).unwrap();
        assert!(!more);
        assert!(storage.get_bucket(1).unwrap().is_none());
    }

    /// Number of leaf, pruned range and receipt records of the bucket.
    fn records_of(storage: &DiskStorage, bucket_id: BucketId) -> usize {
        [CF_LEAVES, CF_PRUNED_RANGES, CF_DELETION_RECEIPTS]
            .into_iter()
            .map(|name| {
                storage
                    .db
                    .iterator_cf(
                        storage.db.cf_handle(name).unwrap(),
                        IteratorMode::From(&seq_key(bucket_id, 0), Direction::Forward),
                    )
                    .map(Result::unwrap)
                    .take_while(|(key, _)| seq_of(bucket_id, key).is_some())
                    .count()
            })
            .sum()
    }

    #[test]
    fn removing_a_bucket_removes_every_record_of_it() {
        let dir = TempDir::new().unwrap();
        let storage = DiskStorage::new(dir.path())
            .unwrap()
            .with_erase_batch_nodes(1);

        // The last erase batch of a condemned bucket.
        bucket_with_leaves(&storage, 1, 2);
        storage.condemn_bucket(1).unwrap();
        storage.erase_batch(1, 0).unwrap();
        assert!(records_of(&storage, 1) > 0);
        storage.erase_batch(1, 0).unwrap();
        assert!(storage.get_bucket(1).unwrap().is_none());
        assert_eq!(records_of(&storage, 1), 0);

        // Condemning a bucket whose only range was erased before.
        bucket_with_leaves(&storage, 2, 1);
        storage.delete_before(2, 1).unwrap();
        storage.erase_pruned_range(2, 0).unwrap();
        assert!(records_of(&storage, 2) > 0);
        storage.condemn_bucket(2).unwrap();
        assert!(storage.get_bucket(2).unwrap().is_none());
        assert_eq!(records_of(&storage, 2), 0);
    }

    #[test]
    fn leaf_count_matches_the_leaf_records() {
        fn assert_in_sync(storage: &DiskStorage, expected: u64) {
            let row = storage.get_bucket(1).unwrap().unwrap();
            assert_eq!(row.leaf_count, expected);
            assert_eq!(live_leaves(storage, 1).len() as u64, expected);
        }

        let dir = TempDir::new().unwrap();
        let storage = DiskStorage::new(dir.path()).unwrap();
        bucket_with_leaves(&storage, 1, 4);
        assert_in_sync(&storage, 4);
        storage.delete_before(1, 1).unwrap();
        assert_in_sync(&storage, 3);
        storage.delete_before(1, 4).unwrap();
        assert_in_sync(&storage, 0);
        let data = vec![9u8; 8];
        let hash = hash_leaf(&data);
        storage
            .store_node(1, hash, ChunkTreeNode::Chunk(data))
            .unwrap();
        storage.commit(1, vec![hash]).unwrap();
        assert_in_sync(&storage, 1);
        storage.condemn_bucket(1).unwrap();
        assert_in_sync(&storage, 0);
    }

    /// A condemned bucket with five leaf records left and nothing to erase:
    /// the state a removal that stopped part way leaves behind.
    fn interrupted_removal(storage: &DiskStorage) {
        bucket_with_leaves(storage, 1, 5);
        storage.delete_before(1, 5).unwrap();
        storage.erase_pruned_range(1, 0).unwrap();
        let mut row = storage.get_bucket(1).unwrap().unwrap();
        row.condemned = true;
        let cf = storage.db.cf_handle(CF_BUCKETS).unwrap();
        storage
            .db
            .put_cf(&cf, 1u64.to_le_bytes(), row.encode())
            .unwrap();
        assert_eq!(records_of(storage, 1), 5);
    }

    #[test]
    fn bucket_removal_deletes_at_most_one_batch_per_transaction() {
        let dir = TempDir::new().unwrap();
        let storage = DiskStorage::new(dir.path())
            .unwrap()
            .with_erase_batch_nodes(2);
        interrupted_removal(&storage);

        assert!(storage.remove_bucket_batch(1).unwrap());
        assert_eq!(records_of(&storage, 1), 3);
        assert!(storage.get_bucket(1).unwrap().is_some());
        assert!(storage.remove_bucket_batch(1).unwrap());
        assert_eq!(records_of(&storage, 1), 1);
        assert!(storage.get_bucket(1).unwrap().is_some());
        assert!(!storage.remove_bucket_batch(1).unwrap());
        assert_eq!(records_of(&storage, 1), 0);
        assert!(storage.get_bucket(1).unwrap().is_none());
    }

    #[test]
    fn condemning_again_resumes_an_interrupted_removal() {
        let dir = TempDir::new().unwrap();
        let storage = DiskStorage::new(dir.path())
            .unwrap()
            .with_erase_batch_nodes(2);
        interrupted_removal(&storage);

        storage.condemn_bucket(1).unwrap();

        assert!(storage.get_bucket(1).unwrap().is_none());
        assert_eq!(records_of(&storage, 1), 0);
    }

    #[test]
    fn a_batch_applied_after_the_range_advanced_is_a_no_op() {
        let dir = TempDir::new().unwrap();
        let storage = DiskStorage::new(dir.path())
            .unwrap()
            .with_erase_batch_nodes(1);
        let roots = stash_leaves(&storage, 2, 2);
        let range = pruned_range(&storage, 1, 0).unwrap();
        let stale = storage.walk_erase_range(1, &range).unwrap();

        storage.erase_batch(1, 0).unwrap();
        let before = storage.get_bucket(1).unwrap().unwrap();
        let outcome = storage.apply_erase(1, &range, stale).unwrap();

        assert_eq!(outcome, EraseOutcome::default());
        assert_eq!(storage.get_bucket(1).unwrap().unwrap(), before);
        assert!(storage.get_node(&roots[1]).unwrap().is_some());
    }

    #[test]
    fn batches_split_between_leaves_that_share_an_internal_subtree() {
        // Two 3-chunk trees share the subtree over their first two chunks.
        // Each tree has 5 nodes, over the 2-node budget, so each leaf is its
        // own batch.
        let (storage, _dir) = storage_with_bucket(u64::MAX);
        let storage = storage.with_erase_batch_nodes(2);
        let [c0, c1] = [1, 2].map(|b| store_chunk(&storage, b));
        let [c2, c3] = [3, 4].map(|b| store_chunk_of_len(&storage, b, 10));
        let roots =
            [c2, c3].map(|last| build_merkle_tree(&storage, BUCKET, &[c0, c1, last]).unwrap());
        storage.commit(BUCKET, roots.to_vec()).unwrap();
        storage.delete_before(BUCKET, 2).unwrap();

        // The first leaf frees its root and its own chunk; the shared nodes
        // keep the second leaf's reference.
        let (first, more) = storage.erase_batch(BUCKET, 0).unwrap();
        assert_eq!((first.nodes_deleted, more), (2, true));
        assert!(storage.get_node(&roots[0]).unwrap().is_none());
        assert!(storage.get_node(&c2).unwrap().is_none());
        for shared in [c0, c1] {
            assert_eq!(storage.refcount_entry(&shared).unwrap().unwrap().0, 1);
        }

        let rest = storage.erase_pruned_range(BUCKET, 0).unwrap();

        assert_eq!(rest.nodes_deleted, 5);
        let bucket = storage.get_bucket(BUCKET).unwrap().unwrap();
        assert_eq!((bucket.used_bytes, bucket.node_count), (0, 0));
        assert!(storage.pruned_ranges(BUCKET).unwrap().is_empty());
    }

    #[test]
    fn concurrent_erases_of_one_range_delete_each_node_once() {
        let dir = TempDir::new().unwrap();
        let storage = DiskStorage::new(dir.path())
            .unwrap()
            .with_erase_batch_nodes(1);
        let roots = stash_leaves(&storage, 6, 6);

        let deleted: u64 = std::thread::scope(|scope| {
            let erasers: Vec<_> = (0..2)
                .map(|_| scope.spawn(|| storage.erase_pruned_range(1, 0).unwrap().nodes_deleted))
                .collect();
            erasers.into_iter().map(|e| e.join().unwrap()).sum()
        });

        assert_eq!(deleted, 6);
        assert!(storage.pruned_ranges(1).unwrap().is_empty());
        for root in roots {
            assert!(storage.get_node(&root).unwrap().is_none());
        }
    }

    #[test]
    fn condemn_then_erase_removes_bucket_row() {
        let dir = TempDir::new().unwrap();
        let storage = DiskStorage::new(dir.path()).unwrap();
        let roots = bucket_with_leaves(&storage, 1, 2);

        storage.condemn_bucket(1).unwrap();
        assert!(storage.is_condemned(1).unwrap());
        let ranges = storage.pruned_ranges(1).unwrap();
        assert_eq!(ranges.len(), 1);
        assert_eq!((ranges[0].first_seq, ranges[0].end_seq), (0, 2));
        // Condemning twice is a no-op.
        storage.condemn_bucket(1).unwrap();
        assert!(storage.is_condemned(1).unwrap());

        let outcome = storage.erase_pruned_range(1, 0).unwrap();
        assert_eq!(outcome.nodes_deleted, 2);
        assert!(
            storage.get_bucket(1).unwrap().is_none(),
            "bucket row must be gone"
        );
        for root in roots {
            assert!(storage.get_node(&root).unwrap().is_none());
        }

        // Condemning a bucket that never stored anything drops it directly.
        storage.init_bucket(2, u64::MAX).unwrap();
        storage.condemn_bucket(2).unwrap();
        assert!(storage.get_bucket(2).unwrap().is_none());
    }

    #[test]
    fn condemn_after_an_earlier_erase_removes_bucket_row() {
        // An erased range keeps its leaves, so the row must go once every
        // range is erased, not once the range list is empty; otherwise a
        // bucket pruned before its teardown would stay on disk forever.
        let dir = TempDir::new().unwrap();
        let storage = DiskStorage::new(dir.path()).unwrap();
        bucket_with_leaves(&storage, 1, 2);
        storage.delete_before(1, 1).unwrap();
        storage.erase_pruned_range(1, 0).unwrap();
        assert!(storage.get_bucket(1).unwrap().is_some());

        storage.condemn_bucket(1).unwrap();
        storage.erase_pruned_range(1, 1).unwrap();
        assert!(
            storage.get_bucket(1).unwrap().is_none(),
            "bucket row must be gone"
        );

        // A condemned bucket with only erased ranges has nothing to erase.
        bucket_with_leaves(&storage, 2, 1);
        storage.delete_before(2, 1).unwrap();
        storage.erase_pruned_range(2, 0).unwrap();
        storage.condemn_bucket(2).unwrap();
        assert!(storage.get_bucket(2).unwrap().is_none());
    }

    #[test]
    fn store_node_on_condemned_bucket_is_rejected() {
        let dir = TempDir::new().unwrap();
        let storage = DiskStorage::new(dir.path()).unwrap();
        let roots = bucket_with_leaves(&storage, 1, 1);
        storage.condemn_bucket(1).unwrap();
        let before = storage.get_bucket(1).unwrap().unwrap();

        // A new node.
        let data = vec![9u8; 8];
        let hash = hash_leaf(&data);
        let err = storage
            .store_node(1, hash, ChunkTreeNode::Chunk(data))
            .unwrap_err();
        assert!(matches!(err, Error::BucketCondemned(1)), "got {err:?}");
        assert!(storage.get_node(&hash).unwrap().is_none());
        assert_eq!(storage.refcount_entry(&hash).unwrap(), None);

        // A node that is already stored.
        let err = storage
            .store_node(1, roots[0], ChunkTreeNode::Chunk(vec![0u8; 8]))
            .unwrap_err();
        assert!(matches!(err, Error::BucketCondemned(1)), "got {err:?}");

        let after = storage.get_bucket(1).unwrap().unwrap();
        assert_eq!(after.used_bytes, before.used_bytes);
        assert_eq!(after.node_count, before.node_count);
    }

    #[test]
    fn commit_on_condemned_bucket_is_rejected() {
        let dir = TempDir::new().unwrap();
        let storage = DiskStorage::new(dir.path()).unwrap();
        bucket_with_leaves(&storage, 1, 1);

        // Stored before condemnation, committed after it.
        let data = vec![9u8; 8];
        let hash = hash_leaf(&data);
        storage
            .store_node(1, hash, ChunkTreeNode::Chunk(data))
            .unwrap();
        storage.condemn_bucket(1).unwrap();

        let err = storage.commit(1, vec![hash]).unwrap_err();
        assert!(matches!(err, Error::BucketCondemned(1)), "got {err:?}");
        let bucket = storage.get_bucket(1).unwrap().unwrap();
        assert_eq!(bucket.leaf_count, 0);
        assert_eq!(storage.refcount_entry(&hash).unwrap(), Some((0, 1, 8)));

        // The rejected writes leave nothing behind: erasing drops the row.
        storage.erase_pruned_range(1, 0).unwrap();
        assert!(storage.get_bucket(1).unwrap().is_none());
    }

    #[test]
    fn set_bucket_quota_enforced_on_next_store() {
        let (storage, _dir) = storage_with_bucket(u64::MAX);
        store_chunk_of_len(&storage, 1, 8);

        storage.set_bucket_quota(BUCKET, 10).unwrap();
        let data = vec![2u8; 8];
        let hash = hash_leaf(&data);
        let err = storage
            .store_node(BUCKET, hash, ChunkTreeNode::Chunk(data))
            .unwrap_err();
        assert!(
            matches!(err, Error::QuotaExceeded { used: 8, max: 10 }),
            "got {err:?}"
        );
        // Unknown bucket errors instead of creating state.
        assert!(storage.set_bucket_quota(99, 10).is_err());
    }

    /// A quota synced from the chain agreement must remain after a node
    /// restart, never reset to unlimited.
    #[test]
    fn bucket_quota_survives_reopen() {
        let dir = TempDir::new().unwrap();
        {
            let storage = DiskStorage::new(dir.path()).unwrap();
            storage.init_bucket(1, u64::MAX).unwrap();
            storage.set_bucket_quota(1, 100).unwrap();
        }

        let storage = DiskStorage::new(dir.path()).unwrap();
        let data = vec![7u8; 128];
        let hash = hash_leaf(&data);
        assert!(matches!(
            storage.store_node(1, hash, ChunkTreeNode::Chunk(data)),
            Err(Error::QuotaExceeded { max: 100, .. })
        ));
    }

    #[test]
    fn proof_for_commitment_past_the_local_end_is_leaves_not_held() {
        let dir = TempDir::new().unwrap();
        let storage = DiskStorage::new(dir.path()).unwrap();
        bucket_with_leaves(&storage, 1, 1);

        // Cites two leaves; this store only holds one (e.g. a replica that
        // has not synced the second commit yet).
        let err = storage
            .get_mmr_proof_for_commitment(
                1,
                &storage_primitives::Commitment {
                    mmr_root: H256::zero(),
                    start_seq: 0,
                    leaf_count: 2,
                },
                0,
            )
            .unwrap_err();
        assert!(
            matches!(err, Error::LeavesNotHeld { local_end: 1, .. }),
            "got {err:?}"
        );
    }

    #[test]
    fn new_wraps_rocksdb_open_failure() {
        // A regular file where RocksDB expects a directory:
        // `TransactionDB::open_cf` must fail, and that failure must surface as
        // `Error::RocksDb`.
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

    /// One full-size chunk at the bottom; each level above is an internal
    /// node whose two children are the level below. `depth` levels store
    /// `depth + 1` nodes but span `2^depth` chunks, and the result is the
    /// canonical tree over `2^depth` copies of the chunk.
    fn diamond_tree(storage: &DiskStorage, bucket_id: BucketId, depth: u32) -> H256 {
        let chunk = vec![0u8; DEFAULT_CHUNK_SIZE as usize];
        let mut hash = hash_leaf(&chunk);
        storage
            .store_node(bucket_id, hash, ChunkTreeNode::Chunk(chunk))
            .unwrap();
        for _ in 0..depth {
            let parent = storage_primitives::hash_children(hash, hash);
            storage
                .store_node(
                    bucket_id,
                    parent,
                    ChunkTreeNode::Internal {
                        left: hash,
                        right: hash,
                    },
                )
                .unwrap();
            hash = parent;
        }
        hash
    }

    /// `data_size` is the logical size: reads and challenge chunk indices
    /// address the logical chunk list, so a chunk referenced twice counts
    /// twice. Deduplication shows up in `used_bytes` only.
    #[test]
    fn commit_records_logical_size_of_deduplicated_tree() {
        let (storage, _dir) = storage_with_bucket(u64::MAX);
        let root = diamond_tree(&storage, BUCKET, 6);

        let outcome = storage.commit(BUCKET, vec![root]).unwrap();

        let bucket = storage.get_bucket(BUCKET).unwrap().unwrap();
        assert_eq!(outcome.leaf_count, 1);
        assert_eq!(
            live_leaves(&storage, BUCKET)[0].data_size,
            64 * DEFAULT_CHUNK_SIZE as u64
        );
        assert_eq!(bucket.used_bytes, DEFAULT_CHUNK_SIZE as u64 + 6 * 64);
        assert_eq!(bucket.node_count, 7);
        assert_eq!(storage.collect_leaves(root).unwrap().len(), 64);
    }

    /// Seven stored nodes span 2^6 chunks, so every traversal of the tree
    /// costs 2^7 - 1 node reads. The budget rejects such a root before
    /// `commit` signs it into the MMR, and rejects reads of it the same way.
    #[test]
    fn traversal_rejects_tree_over_node_budget() {
        let dir = TempDir::new().unwrap();
        let storage = DiskStorage::new(dir.path())
            .unwrap()
            .with_max_tree_walk_visits(100);
        storage.init_bucket(BUCKET, u64::MAX).unwrap();
        let root = diamond_tree(&storage, BUCKET, 6);

        assert!(matches!(
            storage.commit(BUCKET, vec![root]),
            Err(Error::TreeTooLarge { max_nodes: 100 })
        ));
        assert!(matches!(
            storage.collect_leaves(root),
            Err(Error::TreeTooLarge { .. })
        ));
        assert!(live_leaves(&storage, BUCKET).is_empty());
    }

    /// 22 stored nodes span 2^21 chunks. A read follows one path through
    /// them instead of visiting every chunk.
    #[test]
    fn read_of_a_deduplicated_tree_follows_one_path() {
        let (storage, _dir) = storage_with_bucket(u64::MAX);
        let root = diamond_tree(&storage, BUCKET, 21);
        let count = 1u64 << 21;
        let chunk = hash_leaf(&vec![0u8; DEFAULT_CHUNK_SIZE as usize]);

        let reads = CountingReads::new(&storage);
        let range = reads.get_chunks_in_range(root, 0..1).unwrap();
        assert!(reads.get_node_calls.swap(0, Ordering::Relaxed) < 1000);
        assert_eq!(range.len(), 1);
        assert!(verify_merkle_proof(chunk, 0, count, &range[0].2, &root));

        let (_, proof) = reads.get_chunk_at_index(root, count - 1).unwrap();
        assert!(reads.get_node_calls.swap(0, Ordering::Relaxed) < 1000);
        assert!(verify_merkle_proof(chunk, count - 1, count, &proof, &root));
    }

    /// A chain of 70 nodes is deeper than any chunk tree. A read rejects it
    /// after about one pass down the chain.
    #[test]
    fn read_of_a_chain_deeper_than_the_limit_is_rejected() {
        let (storage, _dir) = storage_with_bucket(u64::MAX);
        let leaf = store_chunk_of_len(&storage, 1, 32);
        for left_deep in [true, false] {
            let mut root = leaf;
            for _ in 0..70 {
                root = if left_deep {
                    store_parent(&storage, root, leaf)
                } else {
                    store_parent(&storage, leaf, root)
                };
            }

            let reads = CountingReads::new(&storage);
            assert!(matches!(
                reads.get_chunks_in_range(root, 0..1),
                Err(Error::NonCanonicalTree(_))
            ));
            assert!(
                reads.get_node_calls.load(Ordering::Relaxed) <= 2 * (MAX_TREE_DEPTH as u64 + 1)
            );
        }
    }

    /// A read fails when its path reaches a missing node, and never returns
    /// fewer chunks than asked for. A chunk whose path avoids the missing node
    /// still reads.
    #[test]
    fn read_fails_when_its_path_reaches_a_missing_node() {
        let (storage, _dir) = storage_with_bucket(u64::MAX);
        let leaves: Vec<H256> = [1, 2, 3].map(|b| store_chunk(&storage, b)).to_vec();
        let root = build_merkle_tree(&storage, BUCKET, &leaves).unwrap();
        let delete = |hash: H256| {
            let cf = storage.db.cf_handle(CF_NODES).unwrap();
            storage.db.delete_cf(&cf, hash.as_bytes()).unwrap();
        };

        delete(leaves[1]);
        assert!(matches!(
            storage.get_chunks_in_range(root, 0..3),
            Err(Error::NodeNotFound(_))
        ));
        assert!(matches!(
            storage.get_chunk_at_index(root, 1),
            Err(Error::NodeNotFound(_))
        ));
        assert!(storage.get_chunk_at_index(root, 2).is_ok());

        delete(leaves[2]);
        assert!(matches!(
            storage.get_chunk_at_index(root, 0),
            Err(Error::NodeNotFound(_))
        ));
        assert!(matches!(
            storage.get_chunks_in_range(root, 0..1),
            Err(Error::NodeNotFound(_))
        ));
    }

    /// A tree with a missing node is incomplete data: serving the chunks that
    /// remain would return a truncated file as if it were whole.
    #[test]
    fn traversal_fails_on_missing_node() {
        let dir = TempDir::new().unwrap();
        let storage = DiskStorage::new(dir.path()).unwrap();
        let missing = H256::repeat_byte(7);

        assert!(matches!(
            storage.collect_leaves(missing),
            Err(Error::NodeNotFound(_))
        ));
        assert!(matches!(
            storage.get_chunks_in_range(missing, 0..1),
            Err(Error::NodeNotFound(_))
        ));
    }

    /// Two transactions that lock two bucket rows in opposite order must not
    /// wait out the 30 s lock timeout: deadlock detection fails one of them
    /// at once with `Busy`, which maps to `BucketBusy` and a retry.
    #[test]
    fn lock_cycle_fails_fast_with_busy() {
        let dir = TempDir::new().unwrap();
        let storage = DiskStorage::new(dir.path()).unwrap();
        for bucket_id in [1u64, 2] {
            storage.init_bucket(bucket_id, u64::MAX).unwrap();
        }
        let both_locked = std::sync::Barrier::new(2);
        let started = std::time::Instant::now();

        let lock_in_order = |first: u64, second: u64| {
            let cf = storage.db.cf_handle(CF_BUCKETS).unwrap();
            let txn = storage.transaction();
            txn.get_for_update_cf(&cf, first.to_le_bytes(), true)
                .unwrap();
            both_locked.wait();
            txn.get_for_update_cf(&cf, second.to_le_bytes(), true)
                .map(|_| ())
        };
        let (forward, backward) = std::thread::scope(|scope| {
            let forward = scope.spawn(|| lock_in_order(1, 2));
            let backward = scope.spawn(|| lock_in_order(2, 1));
            (forward.join().unwrap(), backward.join().unwrap())
        });

        let mut failures: Vec<rocksdb::Error> = [forward, backward]
            .into_iter()
            .filter_map(Result::err)
            .collect();
        assert_eq!(failures.len(), 1);
        assert_eq!(failures[0].kind(), ErrorKind::Busy);
        assert!(matches!(
            bucket_write_error(1, failures.pop().unwrap()),
            Error::BucketBusy(1)
        ));
        assert!(started.elapsed() < std::time::Duration::from_secs(5));
    }

    /// Concurrent writers to one bucket must not lose `used_bytes` updates,
    /// and writers to different buckets must not corrupt each other.
    #[test]
    fn concurrent_writers_keep_bucket_accounting_exact() {
        const WRITERS: u64 = 8;
        const NODES_PER_WRITER: u64 = 25;
        const NODE_LEN: u64 = 8;

        let dir = TempDir::new().unwrap();
        let storage = DiskStorage::new(dir.path()).unwrap();
        for bucket_id in [1, 2] {
            storage.init_bucket(bucket_id, u64::MAX).unwrap();
        }

        std::thread::scope(|scope| {
            for writer in 0..WRITERS {
                let storage = &storage;
                scope.spawn(move || {
                    let bucket_id = 1 + writer % 2;
                    for index in 0..NODES_PER_WRITER {
                        let data = (writer * NODES_PER_WRITER + index).to_le_bytes().to_vec();
                        let hash = hash_leaf(&data);
                        storage
                            .store_node(bucket_id, hash, ChunkTreeNode::Chunk(data))
                            .unwrap();
                        storage.commit(bucket_id, vec![hash]).unwrap();
                    }
                });
            }
        });

        for bucket_id in [1, 2] {
            let bucket = storage.get_bucket(bucket_id).unwrap().unwrap();
            assert_eq!(bucket.used_bytes, WRITERS / 2 * NODES_PER_WRITER * NODE_LEN);
            assert_eq!(bucket.node_count, WRITERS / 2 * NODES_PER_WRITER);
            assert_eq!(bucket.leaf_count, WRITERS / 2 * NODES_PER_WRITER);
        }
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
        let before = storage.get_bucket(1).unwrap().unwrap();

        let after = storage.delete_before(1, before.start_seq).unwrap();

        assert_eq!(after.mmr_root, before.mmr_root);
        assert_eq!(after.start_seq, before.start_seq);
        assert_eq!(after.leaf_count, before.leaf_count);
    }

    #[test]
    fn delete_before_prunes_prefix_and_rebuilds_mmr() {
        let dir = TempDir::new().unwrap();
        let storage = DiskStorage::new(dir.path()).unwrap();
        bucket_with_leaves(&storage, 1, 3);
        let before = storage.get_bucket(1).unwrap().unwrap();

        let pruned = storage.delete_before(1, 2).unwrap();

        assert_eq!(pruned.start_seq, 2);
        assert_eq!(pruned.leaf_count, 1);
        assert_ne!(pruned.mmr_root, before.mmr_root);
        // The new root must equal an MMR built from the remaining leaf alone.
        assert_eq!(
            pruned.mmr_root,
            mmr::root(&live_leaves(&storage, 1)).unwrap()
        );
        // Deleting up to the end (empty bucket) is a valid full prune.
        let emptied = storage.delete_before(1, 3).unwrap();
        assert_eq!((emptied.start_seq, emptied.leaf_count), (3, 0));
    }
}
