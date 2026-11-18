// SPDX-License-Identifier: Apache-2.0

use crate::GcError;
use storage_primitives::BucketId;

/// Chain truth about one bucket, as the GC needs it.
#[derive(Clone, Debug, Default)]
pub struct CanonicalBucketState {
    /// Finalized block number the bucket row was read at.
    pub block_number: u64,
    /// Whether the bucket row is present on chain.
    pub exists: bool,
    /// `NextBucketId` at the same block: every bucket id below it was created.
    pub next_bucket_id: u64,
    /// `Bucket::frozen_start_seq` - deletion floor for frozen buckets.
    pub frozen_start_seq: Option<u64>,
    /// `snapshot.commitment.start_seq`; `None` = no snapshot yet.
    pub canonical_start_seq: Option<u64>,
}

/// Chain reads the GC depends on - bucket row, our agreement, pending
/// challenges: everything an erasure decision needs. `Err` means "could
/// not read", and the caller must skip or refuse rather than assume.
#[async_trait::async_trait]
pub trait GcChainClient: Send + Sync {
    /// Bucket row truth (existence, frozen floor, canonical start_seq).
    async fn fetch_canonical_bucket(
        &self,
        bucket_id: BucketId,
    ) -> Result<CanonicalBucketState, GcError>;

    /// This provider's agreement `max_bytes` on the bucket;
    /// `Ok(None)` = no agreement row.
    async fn fetch_agreement_max_bytes(&self, bucket_id: BucketId) -> Result<Option<u64>, GcError>;

    /// Whether any pending challenge targets this provider on the bucket.
    async fn has_pending_challenges(&self, bucket_id: BucketId) -> Result<bool, GcError>;
}

/// One pruned leaf range that is not yet physically erased.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PrunedRange {
    /// Global sequence number of the first stashed leaf.
    pub first_seq: u64,
    /// One past the last stashed leaf.
    pub end_seq: u64,
    /// The start_seq the prune advanced the bucket to.
    pub new_start_seq: u64,
    /// Whether an admin-signed deletion receipt covering this range is held.
    pub has_receipt: bool,
}

/// Result of physically erasing one pruned range.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct EraseOutcome {
    /// Nodes whose refcount reached zero and were deleted.
    pub nodes_deleted: u64,
    /// Bytes credited back to bucket quotas.
    pub bytes_freed: u64,
}

/// Local storage calls the GC makes.
pub trait GcStorage: Send + Sync {
    /// Ids of every locally held bucket.
    fn bucket_ids(&self) -> Vec<BucketId>;

    /// The bucket's current `start_seq`; `Ok(None)` = not held locally.
    fn bucket_start_seq(&self, bucket_id: BucketId) -> Result<Option<u64>, GcError>;

    /// Highest finalized block the GC has read chain state at; 0 if none.
    fn last_scanned_block(&self) -> Result<u64, GcError>;

    /// Store the highest finalized block the GC has read chain state at.
    fn set_last_scanned_block(&self, block_number: u64) -> Result<(), GcError>;

    /// Whether the bucket was condemned (deleted on-chain or agreement gone).
    fn is_condemned(&self, bucket_id: BucketId) -> Result<bool, GcError>;

    /// Condemn the bucket and resume any removal that stopped part way.
    fn condemn_bucket(&self, bucket_id: BucketId) -> Result<(), GcError>;

    /// Create the bucket with this quota, or set the quota of an existing one.
    fn init_bucket_or_set_quota(&self, bucket_id: BucketId, max_bytes: u64) -> Result<(), GcError>;

    /// Prune every leaf below `new_start_seq`.
    fn delete_before(&self, bucket_id: BucketId, new_start_seq: u64) -> Result<(), GcError>;

    /// Pruned ranges awaiting physical erasure, oldest first.
    fn pruned_ranges(&self, bucket_id: BucketId) -> Result<Vec<PrunedRange>, GcError>;

    /// Physically erase the pruned range starting at `first_seq`.
    fn erase_pruned_range(
        &self,
        bucket_id: BucketId,
        first_seq: u64,
    ) -> Result<EraseOutcome, GcError>;
}

/// What a chain event requests from the GC.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum GcTrigger {
    /// Reconcile every local bucket: the follower resubscribed or events were lost.
    Rescan,
    /// Reconcile one bucket after a checkpoint, deletion or agreement change.
    Bucket(BucketId),
}

/// Source of GC triggers.
#[async_trait::async_trait]
pub trait GcEventSource: Send {
    /// The next trigger; `None` once the source is closed for good.
    async fn next(&mut self) -> Option<GcTrigger>;
}
