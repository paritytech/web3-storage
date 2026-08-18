// SPDX-License-Identifier: GPL-3.0-only

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
//! The coordinator is stateless: the stash persisted in the storage backend
//! is the work queue, and every decision re-reads chain truth (fail closed —
//! an unreadable chain skips the bucket until the next pass). Events drive
//! targeted reconciles; a safety-net interval and the follower's
//! `Resubscribed` bootstrap rescue anything missed, including the one-shot
//! `BucketDeleted` (whose chain row is gone afterwards, detected as "local
//! bucket with no chain row").

use crate::{Error, ProviderState};
use provider_chain::{BlockEvent, BlockEventRx};
use std::sync::Arc;
use std::time::Duration;
use storage_primitives::BucketId;
use tokio::sync::broadcast;

/// Chain truth about one bucket, as the GC needs it.
#[derive(Clone, Debug, Default)]
pub struct CanonicalBucketState {
    /// Whether the bucket row is present on chain.
    pub exists: bool,
    /// `Bucket::frozen_start_seq` — deletion floor for frozen buckets.
    pub frozen_start_seq: Option<u64>,
    /// `snapshot.commitment.start_seq`; `None` = no snapshot yet.
    pub canonical_start_seq: Option<u64>,
}

/// Chain reads the GC depends on — bucket row, our agreement, pending
/// challenges: everything an erasure decision needs. `Err` means "could
/// not read", and the caller must skip or refuse rather than assume.
#[async_trait::async_trait]
pub trait GcChainClient: Send + Sync {
    /// Bucket row truth (existence, frozen floor, canonical start_seq).
    async fn fetch_canonical_bucket(
        &self,
        bucket_id: BucketId,
    ) -> Result<CanonicalBucketState, Error>;

    /// This provider's agreement `max_bytes` on the bucket;
    /// `Ok(None)` = no agreement row.
    async fn fetch_agreement_max_bytes(&self, bucket_id: BucketId) -> Result<Option<u64>, Error>;

    /// Whether any pending challenge targets this provider on the bucket.
    async fn has_pending_challenges(&self, bucket_id: BucketId) -> Result<bool, Error>;
}

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
    state: Arc<ProviderState>,
    chain: Arc<dyn GcChainClient>,
}

impl GcCoordinator {
    /// Create a new GC coordinator.
    pub fn new(
        config: GcCoordinatorConfig,
        state: Arc<ProviderState>,
        chain: Arc<dyn GcChainClient>,
    ) -> Self {
        Self {
            config,
            state,
            chain,
        }
    }

    /// Start the background task. `events_rx` must be subscribed before the
    /// block follower starts, so the initial `Resubscribed` bootstrap event
    /// cannot be missed.
    pub fn start(self, events_rx: BlockEventRx) -> GcCoordinatorHandle {
        let task = tokio::spawn(self.run_loop(events_rx));
        GcCoordinatorHandle {
            abort: task.abort_handle(),
        }
    }

    async fn run_loop(self, mut events_rx: BlockEventRx) {
        // A closed broadcast channel (follower gone) yields `Closed` on every
        // poll; disarm the events select arm then, or the loop busy-spins.
        let mut events_open = true;
        // The first tick fires immediately, doubling as the startup pass.
        let mut interval = tokio::time::interval(self.config.scan_interval);

        tracing::info!("GC coordinator started");

        loop {
            tokio::select! {
                event = events_rx.recv(), if events_open => {
                    match event {
                        Ok(BlockEvent::Resubscribed { .. })
                        | Err(broadcast::error::RecvError::Lagged(_)) => {
                            self.rescan().await;
                        }
                        Ok(BlockEvent::BucketCheckpointed { bucket_id, .. })
                        | Ok(BlockEvent::BucketDeleted { bucket_id })
                        | Ok(BlockEvent::AgreementChanged { bucket_id, .. }) => {
                            if self.state.storage.get_bucket(bucket_id).is_some() {
                                self.reconcile_bucket(bucket_id).await;
                            }
                        }
                        Ok(_) => {}
                        Err(broadcast::error::RecvError::Closed) => {
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
        for summary in self.state.storage.list_buckets() {
            self.reconcile_bucket(summary.bucket_id).await;
        }
    }

    /// Reconcile one bucket, logging instead of propagating failures — the
    /// next event or safety-net pass retries.
    async fn reconcile_bucket(&self, bucket_id: BucketId) {
        if let Err(e) = self.try_reconcile(bucket_id).await {
            tracing::warn!(bucket_id, error = %e, "GC reconcile skipped");
        }
    }

    async fn try_reconcile(&self, bucket_id: BucketId) -> Result<(), Error> {
        let chain = self.chain.fetch_canonical_bucket(bucket_id).await?;
        let agreement_max = if chain.exists {
            self.chain.fetch_agreement_max_bytes(bucket_id).await?
        } else {
            None
        };

        // Obligation ends when the bucket row or our agreement row is gone.
        let obligation_gone = !chain.exists || agreement_max.is_none();

        if !self.state.storage.is_condemned(bucket_id) {
            if obligation_gone {
                tracing::info!(bucket_id, "GC: no on-chain obligation, condemning bucket");
                self.state.storage.condemn_bucket(bucket_id)?;
            } else if let Some(max_bytes) = agreement_max {
                // Quota truth lives in the agreement; cheap no-op when equal.
                self.state.storage.set_bucket_quota(bucket_id, max_bytes)?;

                // Converge on canonical prunes we did not perform locally
                // (replicas never receive /delete; a restored node may lag).
                if let (Some(canonical), Some(info)) = (
                    chain.canonical_start_seq,
                    self.state.storage.get_bucket(bucket_id),
                ) {
                    if canonical > info.start_seq {
                        tracing::info!(
                            bucket_id,
                            canonical,
                            local = info.start_seq,
                            "GC: converging local start_seq on canonical prune"
                        );
                        self.state.storage.delete_before(bucket_id, canonical)?;
                    }
                }
            }
        }

        // Erasure pass over the stash. A range may be erased once liability
        // for it has provably ended, which happens one of two ways:
        //
        // - the whole obligation is gone (bucket deleted / agreement over),
        //   re-confirmed against chain truth this very pass — nothing on the
        //   bucket is challengeable anymore; or
        // - the canonical checkpoint has passed the range (so
        //   `challenge_checkpoint` cannot reach it) AND an admin-signed
        //   deletion receipt is held (so a `challenge_offchain` citing any
        //   older signed commitment is answered with the durable `Deleted`
        //   defense instead of the erased bytes).
        //
        // Either way, challenges already open must be resolved first.
        let ranges = self.state.storage.pruned_ranges(bucket_id);
        if ranges.is_empty() {
            return Ok(());
        }
        let condemned = self.state.storage.is_condemned(bucket_id);
        // Lazily fetched once per pass, only when a range is otherwise ripe.
        let mut pending_challenges: Option<bool> = None;

        for range in ranges {
            let liability_ended = if condemned {
                obligation_gone
            } else {
                chain
                    .canonical_start_seq
                    .is_some_and(|c| c >= range.new_start_seq)
                    && range.has_receipt
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

            let outcome = self
                .state
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
        }
        Ok(())
    }
}
