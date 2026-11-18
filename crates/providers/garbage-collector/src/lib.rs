// SPDX-License-Identifier: Apache-2.0

//! Garbage-collection coordinator: chain-truth-driven physical erasure.
//!
//! Deletion is two-phased. A prune (`/delete` + checkpoint) or an on-chain
//! bucket teardown only *stashes* leaves locally; this coordinator later
//! erases each stashed range once liability for it has provably passed:
//!
//! - **canonical checkpoint** — the bucket's on-chain snapshot `start_seq` has
//!   reached the range's `new_start_seq` (the design's pruning rule), or the
//!   bucket/our agreement is gone from the chain entirely;
//! - **deletion receipt** — an admin-signed deletion authorization is held,
//!   so a `challenge_offchain` citing an older signed commitment is answered
//!   with the durable `Deleted` defense instead of the erased bytes;
//! - **no pending challenges** against this provider on the bucket.
//!
//! The stash persisted in the storage backend is the work queue, and every
//! decision re-reads chain truth (fail closed — an unreadable chain skips the
//! bucket until the next pass). A chain read is also refused when it is older
//! than one already made: the storage backend keeps the highest finalized
//! block read (`last_scanned_block`), and a bucket row missing for an id at or
//! above `NextBucketId` means the read predates the bucket. A condemned bucket
//! is erased only by a pass that reads a newer block than the pass that first
//! saw its obligation gone. The block number check does not detect a
//! different chain: on a chain below `last_scanned_block` the GC skips every
//! bucket until that height is reached.
//!
//! Events drive targeted reconciles; a safety-net interval and the follower's
//! `Resubscribed` bootstrap rescue anything missed, including the one-shot
//! `BucketDeleted` (whose chain row is gone afterwards, detected as "local
//! bucket with no chain row").
//!
//! The coordinator reaches the chain, local storage and chain events only
//! through [`GcChainClient`], [`GcStorage`] and [`GcEventSource`]; the
//! provider node supplies the implementations.

mod error;
mod traits;

pub use error::{BoxError, GcError};
pub use traits::{
    CanonicalBucketState, EraseOutcome, GcChainClient, GcEventSource, GcStorage, GcTrigger,
    PrunedRange,
};

use parking_lot::Mutex;
use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;
use storage_primitives::BucketId;

/// GC coordinator configuration.
#[derive(Clone, Debug)]
pub struct GcCoordinatorConfig {
    /// Safety-net full-rescan interval (also the startup bootstrap pass).
    pub scan_interval: Duration,
}

impl Default for GcCoordinatorConfig {
    fn default() -> Self {
        Self {
            scan_interval: Duration::from_secs(600),
        }
    }
}

/// Handle to the running coordinator task.
pub struct GcCoordinatorHandle {
    abort: tokio::task::AbortHandle,
}

impl GcCoordinatorHandle {
    /// Stop the coordinator task.
    pub fn stop(&self) {
        self.abort.abort();
    }
}

/// The GC coordinator service. See the module docs for the algorithm.
pub struct GcCoordinator {
    config: GcCoordinatorConfig,
    storage: Arc<dyn GcStorage>,
    chain: Arc<dyn GcChainClient>,
    /// For each condemned bucket, the block of the first pass that saw its
    /// obligation gone.
    obligation_gone_at: Mutex<HashMap<BucketId, u64>>,
}

impl GcCoordinator {
    /// Create a new GC coordinator.
    pub fn new(
        config: GcCoordinatorConfig,
        storage: Arc<dyn GcStorage>,
        chain: Arc<dyn GcChainClient>,
    ) -> Self {
        Self {
            config,
            storage,
            chain,
            obligation_gone_at: Mutex::new(HashMap::new()),
        }
    }

    /// Start the background task. `events` must be subscribed before the
    /// block follower starts, so the initial `Resubscribed` bootstrap event
    /// cannot be missed.
    pub fn start(self, events: Box<dyn GcEventSource>) -> GcCoordinatorHandle {
        let task = tokio::spawn(self.run_loop(events));
        GcCoordinatorHandle {
            abort: task.abort_handle(),
        }
    }

    async fn run_loop(self, mut events: Box<dyn GcEventSource>) {
        // A closed event source (follower gone) yields `None` on every
        // poll; disarm the events select arm then, or the loop busy-spins.
        let mut events_open = true;
        // The first tick fires immediately, doubling as the startup pass.
        let mut interval = tokio::time::interval(self.config.scan_interval);

        tracing::info!("GC coordinator started");

        loop {
            tokio::select! {
                trigger = events.next(), if events_open => {
                    match trigger {
                        Some(GcTrigger::Rescan) => {
                            self.rescan().await;
                        }
                        Some(GcTrigger::Bucket(bucket_id)) => {
                            if self.storage.bucket_start_seq(bucket_id).ok().flatten().is_some() {
                                self.reconcile_bucket(bucket_id).await;
                            }
                        }
                        None => {
                            events_open = false;
                        }
                    }
                }
                _ = interval.tick() => {
                    self.rescan().await;
                }
            }
        }
    }

    /// Reconcile every locally-known bucket against chain truth.
    async fn rescan(&self) {
        for bucket_id in self.storage.bucket_ids() {
            self.reconcile_bucket(bucket_id).await;
        }
    }

    /// Reconcile one bucket, logging instead of propagating failures — the
    /// next event or safety-net pass retries.
    async fn reconcile_bucket(&self, bucket_id: BucketId) {
        if let Err(e) = self.try_reconcile(bucket_id).await {
            tracing::warn!(bucket_id, error = %e, "GC reconcile skipped");
        }
    }

    /// One reconcile pass over a bucket: read chain truth, bring the local
    /// bucket row in line with it, then erase every stashed range whose
    /// liability has ended.
    async fn try_reconcile(&self, bucket_id: BucketId) -> Result<(), GcError> {
        let Some(canonical_bucket_state) = self.read_canonical_bucket_state(bucket_id).await?
        else {
            return Ok(());
        };

        let agreement_max = if canonical_bucket_state.exists {
            self.chain.fetch_agreement_max_bytes(bucket_id).await?
        } else {
            None
        };

        // Obligation ends when the bucket row or our agreement row is gone.
        let obligation_gone = !canonical_bucket_state.exists || agreement_max.is_none();
        if !obligation_gone {
            self.obligation_gone_at.lock().remove(&bucket_id);
        }

        self.sync_local_bucket(
            bucket_id,
            &canonical_bucket_state,
            obligation_gone,
            agreement_max,
        )?;
        self.erase_ripe_ranges(bucket_id, &canonical_bucket_state, obligation_gone)
            .await
    }

    /// Read the bucket's chain state and advance `last_scanned_block` to the
    /// block it was read at. `None` when the read must not be acted on: it is
    /// older than a block already read, or it predates the bucket's creation.
    async fn read_canonical_bucket_state(
        &self,
        bucket_id: BucketId,
    ) -> Result<Option<CanonicalBucketState>, GcError> {
        let canonical_bucket_state = self.chain.fetch_canonical_bucket(bucket_id).await?;

        let last_scanned_block = self.storage.last_scanned_block()?;
        if canonical_bucket_state.block_number < last_scanned_block {
            tracing::warn!(
                bucket_id,
                block = canonical_bucket_state.block_number,
                last_scanned_block,
                "GC: canonical bucket state is older than one already read, skipping bucket"
            );
            return Ok(None);
        }
        if !canonical_bucket_state.exists && bucket_id >= canonical_bucket_state.next_bucket_id {
            tracing::warn!(
                bucket_id,
                block = canonical_bucket_state.block_number,
                next_bucket_id = canonical_bucket_state.next_bucket_id,
                "GC: bucket was not created yet at the read block, skipping bucket"
            );
            return Ok(None);
        }
        if canonical_bucket_state.block_number > last_scanned_block {
            self.storage
                .set_last_scanned_block(canonical_bucket_state.block_number)?;
        }
        Ok(Some(canonical_bucket_state))
    }

    /// Bring the local bucket row in line with chain truth: condemn it once
    /// the obligation is gone, otherwise apply the agreement quota and
    /// converge on canonical prunes. An already condemned bucket only resumes
    /// a removal that stopped part way.
    fn sync_local_bucket(
        &self,
        bucket_id: BucketId,
        canonical_bucket_state: &CanonicalBucketState,
        obligation_gone: bool,
        agreement_max: Option<u64>,
    ) -> Result<(), GcError> {
        if self.storage.is_condemned(bucket_id)? {
            // Resumes a removal that stopped part way; a no-op otherwise.
            return self.storage.condemn_bucket(bucket_id);
        }
        if obligation_gone {
            tracing::info!(bucket_id, "GC: no on-chain obligation, condemning bucket");
            return self.storage.condemn_bucket(bucket_id);
        }
        if let Some(max_bytes) = agreement_max {
            // Quota truth lives in the agreement; cheap no-op when equal.
            self.storage
                .init_bucket_or_set_quota(bucket_id, max_bytes)?;
            self.converge_on_canonical_prune(bucket_id, canonical_bucket_state)?;
        }
        Ok(())
    }

    /// Apply a canonical prune this node did not perform locally (replicas
    /// never receive `/delete`; a restored node may lag).
    fn converge_on_canonical_prune(
        &self,
        bucket_id: BucketId,
        canonical_bucket_state: &CanonicalBucketState,
    ) -> Result<(), GcError> {
        let (Some(canonical_start_seq), Some(local_start_seq)) = (
            canonical_bucket_state.canonical_start_seq,
            self.storage.bucket_start_seq(bucket_id)?,
        ) else {
            return Ok(());
        };
        if canonical_start_seq > local_start_seq {
            tracing::info!(
                bucket_id,
                canonical_start_seq,
                local_start_seq,
                "GC: converging local start_seq on canonical prune"
            );
            self.storage.delete_before(bucket_id, canonical_start_seq)?;
        }
        Ok(())
    }

    /// Erasure pass over the stash. A range may be erased once liability for
    /// it has provably ended, which happens one of two ways:
    ///
    /// - the whole obligation is gone (bucket deleted / agreement over),
    ///   re-confirmed against chain truth by a later pass (see
    ///   [`Self::obligation_gone_confirmed`]): nothing on the bucket is
    ///   challengeable anymore; or
    /// - the canonical checkpoint has passed the range (so
    ///   `challenge_checkpoint` cannot reach it) AND an admin-signed deletion
    ///   receipt is held (so a `challenge_offchain` citing any older signed
    ///   commitment is answered with the durable `Deleted` defense instead of
    ///   the erased bytes).
    ///
    /// Either way, challenges already open must be resolved first.
    async fn erase_ripe_ranges(
        &self,
        bucket_id: BucketId,
        canonical_bucket_state: &CanonicalBucketState,
        obligation_gone: bool,
    ) -> Result<(), GcError> {
        let ranges = self.storage.pruned_ranges(bucket_id)?;
        if ranges.is_empty() {
            return Ok(());
        }
        let condemned = self.storage.is_condemned(bucket_id)?;
        let obligation_gone_confirmed = condemned
            && obligation_gone
            && self.obligation_gone_confirmed(bucket_id, canonical_bucket_state.block_number);
        // Lazily fetched once per pass, only when a range is otherwise ripe.
        let mut pending_challenges: Option<bool> = None;

        for range in ranges {
            let liability_ended = if condemned {
                obligation_gone_confirmed
            } else {
                Self::checkpoint_passed_with_receipt(canonical_bucket_state, &range)
            };
            if !liability_ended {
                continue;
            }

            let has_pending = match pending_challenges {
                Some(p) => p,
                None => {
                    let p = self.chain.has_pending_challenges(bucket_id).await?;
                    pending_challenges = Some(p);
                    p
                }
            };
            if has_pending {
                tracing::info!(bucket_id, "GC: pending challenge, erasure deferred");
                continue;
            }

            self.erase_range(bucket_id, &range)?;
        }
        if condemned && self.storage.bucket_start_seq(bucket_id)?.is_none() {
            self.obligation_gone_at.lock().remove(&bucket_id);
        }
        Ok(())
    }

    /// Whether a pass at `block_number` may erase a condemned bucket: it
    /// must read a newer block than the pass that first saw the obligation
    /// gone. The first such pass only records its block.
    fn obligation_gone_confirmed(&self, bucket_id: BucketId, block_number: u64) -> bool {
        let mut gone_at = self.obligation_gone_at.lock();
        let first_block = *gone_at.entry(bucket_id).or_insert(block_number);
        block_number > first_block
    }

    /// The pruning rule for a live bucket: the canonical checkpoint reached
    /// the range's `new_start_seq` and a deletion receipt covers the range.
    fn checkpoint_passed_with_receipt(
        canonical_bucket_state: &CanonicalBucketState,
        range: &PrunedRange,
    ) -> bool {
        canonical_bucket_state
            .canonical_start_seq
            .is_some_and(|c| c >= range.new_start_seq)
            && range.has_receipt
    }

    /// Physically erase one stashed range and log the outcome.
    fn erase_range(&self, bucket_id: BucketId, range: &PrunedRange) -> Result<(), GcError> {
        let outcome = self
            .storage
            .erase_pruned_range(bucket_id, range.first_seq)?;
        tracing::info!(
            bucket_id,
            first_seq = range.first_seq,
            end_seq = range.end_seq,
            nodes_deleted = outcome.nodes_deleted,
            bytes_freed = outcome.bytes_freed,
            "GC: pruned range physically erased"
        );
        Ok(())
    }
}
