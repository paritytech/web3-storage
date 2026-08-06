// SPDX-License-Identifier: Apache-2.0

//! Replica Sync Coordinator - Autonomous replica synchronization service.
//!
//! This module provides a background service that:
//! 1. Reacts to agreement/checkpoint events fanned out by the chain-state
//!    coordinator (with a bootstrap scan on every (re)subscribe and a slow
//!    safety-net interval as backstop)
//! 2. Detects when new data is available to sync
//! 3. Performs top-down MMR traversal to fetch missing data from primaries
//! 4. Submits `confirm_replica_sync` transactions to receive payment
//! 5. Handles historical roots matching for late syncs

use crate::sync::ReplicaSync;
use crate::Error;
use provider_chain::chain_events::{BlockEvent, BlockEventRx};
use provider_storage::StorageBackend;
use sp_core::H256;
use sp_runtime::AccountId32;
use std::collections::HashMap;
use std::str::FromStr;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;
use storage_primitives::BucketId;
use tokio::sync::{broadcast, mpsc, oneshot};

/// Configuration for the replica sync coordinator.
#[derive(Clone, Debug)]
pub struct ReplicaSyncCoordinatorConfig {
    /// Safety-net interval between duty reconciliation passes. Duties are
    /// normally discovered event-driven; zero disables the safety net.
    pub poll_interval: Duration,
    /// Timeout for a sync operation (default: 5 minutes).
    pub sync_timeout: Duration,
    /// Maximum concurrent bucket syncs (default: 3).
    pub max_concurrent_syncs: usize,
    /// Whether to automatically submit confirm_replica_sync.
    pub auto_confirm: bool,
}

impl Default for ReplicaSyncCoordinatorConfig {
    fn default() -> Self {
        Self {
            poll_interval: Duration::from_secs(600),
            sync_timeout: Duration::from_secs(300),
            max_concurrent_syncs: 3,
            auto_confirm: true,
        }
    }
}

/// Information about a replica sync duty.
#[derive(Clone, Debug)]
pub struct SyncDuty {
    /// Bucket needing sync.
    pub bucket_id: BucketId,
    /// Target MMR root from the latest checkpoint.
    pub target_mmr_root: H256,
    /// Target leaf count.
    pub target_leaf_count: u64,
    /// Primary provider endpoints to sync from.
    pub primary_endpoints: Vec<String>,
    /// Available sync balance for this agreement.
    pub sync_balance: u128,
    /// Price per sync operation.
    pub sync_price: u128,
    /// Minimum blocks between syncs.
    pub min_sync_interval: u64,
    /// Last sync info (root, block) if any.
    pub last_sync: Option<(H256, u64)>,
}

/// Result of a replica sync operation.
#[derive(Clone, Debug)]
pub enum SyncResult {
    /// Successfully synced and confirmed on-chain.
    Success {
        bucket_id: BucketId,
        mmr_root: H256,
        position_matched: u8,
        payment: u128,
    },
    /// Sync balance insufficient for payment.
    InsufficientBalance {
        bucket_id: BucketId,
        required: u128,
        available: u128,
    },
    /// Sync interval has not elapsed since last sync.
    SyncIntervalNotElapsed {
        bucket_id: BucketId,
        blocks_remaining: u64,
    },
    /// All primary providers unavailable.
    PrimaryUnavailable {
        bucket_id: BucketId,
        tried: Vec<String>,
    },
    /// Local state doesn't match expected root after sync.
    VerificationFailed { bucket_id: BucketId, reason: String },
    /// Failed to submit confirm_replica_sync transaction.
    SubmissionFailed { bucket_id: BucketId, error: String },
    /// Already synced to this root.
    AlreadySynced { bucket_id: BucketId, mmr_root: H256 },
    /// No data to sync yet.
    NoDataToSync { bucket_id: BucketId },
}

/// Commands for controlling the coordinator.
#[derive(Debug)]
pub enum SyncCommand {
    /// Stop the coordinator.
    Stop,
    /// Pause automatic syncs.
    Pause,
    /// Resume automatic syncs.
    Resume,
    /// Force sync for a specific bucket.
    ForceSync { bucket_id: BucketId },
    /// Get current status.
    Status {
        response_tx: oneshot::Sender<SyncCoordinatorStatus>,
    },
}

/// Overall coordinator status.
#[derive(Clone, Debug)]
pub struct SyncCoordinatorStatus {
    /// Whether coordinator is running.
    pub running: bool,
    /// Whether coordinator is paused.
    pub paused: bool,
    /// Number of active sync operations.
    pub active_syncs: usize,
    /// Buckets being tracked as replica.
    pub tracked_buckets: Vec<BucketId>,
}

/// Information about a replica agreement from chain.
#[derive(Clone, Debug)]
pub struct ReplicaAgreementInfo {
    pub bucket_id: BucketId,
    pub sync_balance: u128,
    pub sync_price: u128,
    pub min_sync_interval: u64,
    pub last_sync: Option<(H256, u64)>,
}

/// Bucket snapshot from chain.
#[derive(Clone, Debug)]
pub struct BucketSnapshot {
    pub mmr_root: H256,
    pub leaf_count: u64,
}

/// Trait abstracting chain interactions for the replica sync coordinator.
#[async_trait::async_trait]
pub trait ReplicaSyncChainClient: Send + Sync {
    /// Get the current block number.
    async fn get_current_block(&self) -> Result<u64, Error>;

    /// Fetch replica agreements for this provider.
    async fn fetch_replica_agreements(
        &self,
        provider_account: &str,
        local_buckets: Vec<BucketId>,
    ) -> Result<Vec<ReplicaAgreementInfo>, Error>;

    /// Fetch the bucket snapshot (latest checkpoint state) from chain.
    async fn fetch_bucket_snapshot(&self, bucket_id: BucketId) -> Result<BucketSnapshot, Error>;

    /// Fetch primary provider HTTP endpoints for a bucket.
    async fn fetch_primary_endpoints(&self, bucket_id: BucketId) -> Result<Vec<String>, Error>;

    /// Submit a confirm_replica_sync extrinsic.
    async fn submit_sync_confirmation(
        &self,
        bucket_id: BucketId,
        target_mmr_root: H256,
    ) -> Result<(u8, u128), Error>;
}

#[async_trait::async_trait]
impl<T: ReplicaSyncChainClient> ReplicaSyncChainClient for Arc<T> {
    async fn get_current_block(&self) -> Result<u64, Error> {
        self.as_ref().get_current_block().await
    }

    async fn fetch_replica_agreements(
        &self,
        provider_account: &str,
        local_buckets: Vec<BucketId>,
    ) -> Result<Vec<ReplicaAgreementInfo>, Error> {
        self.as_ref()
            .fetch_replica_agreements(provider_account, local_buckets)
            .await
    }

    async fn fetch_bucket_snapshot(&self, bucket_id: BucketId) -> Result<BucketSnapshot, Error> {
        self.as_ref().fetch_bucket_snapshot(bucket_id).await
    }

    async fn fetch_primary_endpoints(&self, bucket_id: BucketId) -> Result<Vec<String>, Error> {
        self.as_ref().fetch_primary_endpoints(bucket_id).await
    }

    async fn submit_sync_confirmation(
        &self,
        bucket_id: BucketId,
        target_mmr_root: H256,
    ) -> Result<(u8, u128), Error> {
        self.as_ref()
            .submit_sync_confirmation(bucket_id, target_mmr_root)
            .await
    }
}

/// Handle for controlling the replica sync coordinator.
pub struct ReplicaSyncCoordinatorHandle {
    command_tx: mpsc::Sender<SyncCommand>,
    running: Arc<AtomicBool>,
}

impl ReplicaSyncCoordinatorHandle {
    /// Check if the coordinator is running.
    pub fn is_running(&self) -> bool {
        self.running.load(Ordering::SeqCst)
    }

    /// Stop the coordinator.
    pub async fn stop(&self) -> Result<(), Error> {
        self.command_tx
            .send(SyncCommand::Stop)
            .await
            .map_err(|_| Error::Internal("Coordinator channel closed".to_string()))
    }

    /// Pause automatic syncs.
    pub async fn pause(&self) -> Result<(), Error> {
        self.command_tx
            .send(SyncCommand::Pause)
            .await
            .map_err(|_| Error::Internal("Coordinator channel closed".to_string()))
    }

    /// Resume automatic syncs.
    pub async fn resume(&self) -> Result<(), Error> {
        self.command_tx
            .send(SyncCommand::Resume)
            .await
            .map_err(|_| Error::Internal("Coordinator channel closed".to_string()))
    }

    /// Force a sync for a specific bucket.
    pub async fn force_sync(&self, bucket_id: BucketId) -> Result<(), Error> {
        self.command_tx
            .send(SyncCommand::ForceSync { bucket_id })
            .await
            .map_err(|_| Error::Internal("Coordinator channel closed".to_string()))
    }

    /// Get current coordinator status.
    pub async fn status(&self) -> Result<SyncCoordinatorStatus, Error> {
        let (response_tx, response_rx) = oneshot::channel();
        self.command_tx
            .send(SyncCommand::Status { response_tx })
            .await
            .map_err(|_| Error::Internal("Coordinator channel closed".to_string()))?;

        response_rx
            .await
            .map_err(|_| Error::Internal("Status response channel closed".to_string()))
    }
}

/// Replica sync coordinator service.
pub struct ReplicaSyncCoordinator {
    config: ReplicaSyncCoordinatorConfig,
    storage: Arc<dyn StorageBackend>,
    provider_id: String,
    chain_client: Box<dyn ReplicaSyncChainClient>,
    replica_sync: ReplicaSync,
    /// Track active sync operations by bucket.
    active_syncs: HashMap<BucketId, tokio::task::JoinHandle<SyncResult>>,
}

impl ReplicaSyncCoordinator {
    /// Create a new replica sync coordinator.
    pub fn new(
        config: ReplicaSyncCoordinatorConfig,
        storage: Arc<dyn StorageBackend>,
        provider_id: String,
        chain_client: Box<dyn ReplicaSyncChainClient>,
    ) -> Self {
        let replica_sync = ReplicaSync::new(storage.clone());

        Self {
            config,
            storage,
            provider_id,
            chain_client,
            replica_sync,
            active_syncs: HashMap::new(),
        }
    }

    /// Start the replica sync coordinator background service.
    ///
    /// `events_rx` is a subscription to the chain-state coordinator's block
    /// event fan-out; duty passes run on relevant agreement/checkpoint
    /// events, on `Resubscribed` / lag, and on the safety-net interval.
    pub async fn start(
        self,
        events_rx: BlockEventRx,
        callback: Option<Arc<dyn Fn(SyncResult) + Send + Sync>>,
    ) -> Result<ReplicaSyncCoordinatorHandle, Error> {
        let (command_tx, command_rx) = mpsc::channel::<SyncCommand>(32);
        let running = Arc::new(AtomicBool::new(true));
        let running_clone = running.clone();

        tokio::spawn(async move {
            self.run_loop(command_rx, events_rx, running_clone, callback)
                .await;
        });

        Ok(ReplicaSyncCoordinatorHandle {
            command_tx,
            running,
        })
    }

    /// Whether a broadcast event can create or advance a sync duty for us.
    ///
    /// Replica duties appear when we get a replica agreement and progress
    /// when a bucket we hold locally is checkpointed. Everything else is
    /// noise for this coordinator.
    fn is_relevant_event(&self, event: &BlockEvent, our_account: &Option<AccountId32>) -> bool {
        match event {
            BlockEvent::ReplicaAgreementEstablished { provider, .. } => {
                our_account.as_ref().is_none_or(|me| me == provider)
            }
            BlockEvent::BucketCheckpointed { bucket_id } => {
                self.storage.get_bucket(*bucket_id).is_some()
            }
            _ => false,
        }
    }

    /// Main coordinator loop.
    async fn run_loop(
        mut self,
        mut command_rx: mpsc::Receiver<SyncCommand>,
        mut events_rx: BlockEventRx,
        running: Arc<AtomicBool>,
        callback: Option<Arc<dyn Fn(SyncResult) + Send + Sync>>,
    ) {
        let mut paused = false;
        // A closed broadcast channel (follower gone) yields `Closed` on every
        // poll; disarm the events select arm then, or the loop busy-spins.
        let mut events_open = true;
        let our_account = AccountId32::from_str(&self.provider_id).ok();
        // The safety-net interval's first tick fires immediately, doubling as
        // the startup bootstrap pass (duties accrued while the node was
        // down). With the safety net disabled, the bootstrap pass comes from
        // the follower's `Resubscribed` event on its first connect instead.
        let safety_net = !self.config.poll_interval.is_zero();
        let mut interval = tokio::time::interval(if safety_net {
            self.config.poll_interval
        } else {
            Duration::from_secs(3600)
        });

        tracing::info!("Replica sync coordinator started");

        loop {
            tokio::select! {
                // Prefer control commands over event/scan work, so a
                // Pause/Stop queued right after start() is honored first.
                biased;

                cmd = command_rx.recv() => {
                    match cmd {
                        Some(SyncCommand::Stop) | None => {
                            tracing::info!("Replica sync coordinator stopping");
                            running.store(false, Ordering::SeqCst);
                            break;
                        }
                        Some(SyncCommand::Pause) => {
                            tracing::info!("Replica sync coordinator paused");
                            paused = true;
                        }
                        Some(SyncCommand::Resume) => {
                            tracing::info!("Replica sync coordinator resumed");
                            paused = false;
                        }
                        Some(SyncCommand::ForceSync { bucket_id }) => {
                            tracing::info!("Force sync requested for bucket {bucket_id}");
                            if let Ok(Some(duty)) = self.get_sync_duty(bucket_id).await {
                                let result = self.sync_and_confirm(&duty).await;
                                if let Some(ref cb) = callback {
                                    cb(result);
                                }
                            }
                        }
                        Some(SyncCommand::Status { response_tx }) => {
                            let status = SyncCoordinatorStatus {
                                running: running.load(Ordering::SeqCst),
                                paused,
                                active_syncs: self.active_syncs.len(),
                                tracked_buckets: self.get_tracked_buckets(),
                            };
                            let _ = response_tx.send(status);
                        }
                    }
                }
                // While paused, stop consuming so events stay queued instead of
                // being dropped. Replaying them on resume is safe: a duty pass
                // reconciles against live chain state, so a stale event costs at
                // most one redundant pass. A pause longer than the channel's
                // capacity surfaces as `Lagged` below, which does the same.
                event = events_rx.recv(), if events_open && !paused => {
                    if matches!(event, Err(broadcast::error::RecvError::Closed)) {
                        events_open = false;
                        continue;
                    }
                    // Unlike `paused`, this is permanent config: drain and drop,
                    // since no later state change makes these actionable.
                    if !self.config.auto_confirm {
                        continue;
                    }
                    match event {
                        Ok(BlockEvent::Resubscribed { .. })
                        | Err(broadcast::error::RecvError::Lagged(_)) => {
                            self.run_duty_pass(&callback).await;
                        }
                        Ok(event) if self.is_relevant_event(&event, &our_account) => {
                            self.run_duty_pass(&callback).await;
                        }
                        Ok(_) | Err(broadcast::error::RecvError::Closed) => {}
                    }
                }
                _ = interval.tick() => {
                    if paused || !self.config.auto_confirm || !safety_net {
                        continue;
                    }
                    self.run_duty_pass(&callback).await;
                }
            }
        }
    }

    /// One duty pass: reconcile agreements against local state and start
    /// syncs for every bucket that needs one (bounded by
    /// `max_concurrent_syncs`).
    ///
    /// Runs the full agreement fetch even when triggered by a single-bucket
    /// event: relevant events are rare (new replica agreement, checkpoint on
    /// a held bucket), so this stays off the hot path while keeping exactly
    /// one duty-discovery code path.
    async fn run_duty_pass(&mut self, callback: &Option<Arc<dyn Fn(SyncResult) + Send + Sync>>) {
        // Clean up completed syncs
        self.cleanup_completed_syncs();

        // Get active replica duties
        match self.get_active_replica_duties().await {
            Ok(duties) => {
                for duty in duties {
                    // Skip if already syncing this bucket
                    if self.active_syncs.contains_key(&duty.bucket_id) {
                        continue;
                    }

                    // Skip if at max concurrent syncs
                    if self.active_syncs.len() >= self.config.max_concurrent_syncs {
                        break;
                    }

                    tracing::info!(
                        "Starting sync for bucket {} (target root: 0x{})",
                        duty.bucket_id,
                        hex::encode(duty.target_mmr_root.as_bytes())
                    );

                    let result = self.sync_and_confirm(&duty).await;
                    if let Some(ref cb) = callback {
                        cb(result);
                    }
                }
            }
            Err(e) => {
                tracing::warn!("Failed to get replica duties: {e}");
            }
        }
    }

    /// Clean up completed sync tasks.
    fn cleanup_completed_syncs(&mut self) {
        self.active_syncs.retain(|_, handle| !handle.is_finished());
    }

    /// Get list of bucket IDs we're tracking as replica.
    fn get_tracked_buckets(&self) -> Vec<BucketId> {
        self.storage
            .list_buckets()
            .into_iter()
            .map(|b| b.bucket_id)
            .collect()
    }

    /// Get replica duties for buckets where this provider is a replica.
    pub async fn get_active_replica_duties(&self) -> Result<Vec<SyncDuty>, Error> {
        let mut duties = Vec::new();

        let anchor_block = self.chain_client.get_current_block().await?;

        let local_buckets: Vec<u64> = self
            .storage
            .list_buckets()
            .into_iter()
            .map(|b| b.bucket_id)
            .collect();

        let provider_account = self.provider_id.clone();

        let agreements = self
            .chain_client
            .fetch_replica_agreements(&provider_account, local_buckets)
            .await?;

        for agreement in agreements {
            if agreement.sync_balance < agreement.sync_price {
                tracing::debug!(
                    "Bucket {} has insufficient sync balance: {} < {}",
                    agreement.bucket_id,
                    agreement.sync_balance,
                    agreement.sync_price
                );
                continue;
            }

            if let Some((_, last_block)) = agreement.last_sync {
                let elapsed = anchor_block.saturating_sub(last_block);
                if elapsed < agreement.min_sync_interval {
                    tracing::debug!(
                        "Bucket {} sync interval not elapsed: {} < {}",
                        agreement.bucket_id,
                        elapsed,
                        agreement.min_sync_interval
                    );
                    continue;
                }
            }

            let snapshot = self
                .chain_client
                .fetch_bucket_snapshot(agreement.bucket_id)
                .await?;

            if snapshot.mmr_root == H256::zero() {
                continue;
            }

            if let Some(bucket) = self.storage.get_bucket(agreement.bucket_id) {
                if bucket.mmr_root == snapshot.mmr_root {
                    continue;
                }
            }

            let primary_endpoints = self
                .chain_client
                .fetch_primary_endpoints(agreement.bucket_id)
                .await?;

            duties.push(SyncDuty {
                bucket_id: agreement.bucket_id,
                target_mmr_root: snapshot.mmr_root,
                target_leaf_count: snapshot.leaf_count,
                primary_endpoints,
                sync_balance: agreement.sync_balance,
                sync_price: agreement.sync_price,
                min_sync_interval: agreement.min_sync_interval,
                last_sync: agreement.last_sync,
            });
        }

        Ok(duties)
    }

    /// Get sync duty for a specific bucket.
    async fn get_sync_duty(&self, bucket_id: BucketId) -> Result<Option<SyncDuty>, Error> {
        let snapshot = self.chain_client.fetch_bucket_snapshot(bucket_id).await?;

        let primary_endpoints = self.chain_client.fetch_primary_endpoints(bucket_id).await?;

        Ok(Some(SyncDuty {
            bucket_id,
            target_mmr_root: snapshot.mmr_root,
            target_leaf_count: snapshot.leaf_count,
            primary_endpoints,
            sync_balance: u128::MAX,
            sync_price: 0,
            min_sync_interval: 0,
            last_sync: None,
        }))
    }

    /// Perform sync and submit confirmation.
    pub async fn sync_and_confirm(&self, duty: &SyncDuty) -> SyncResult {
        // Check if we already have this root
        if let Some(bucket) = self.storage.get_bucket(duty.bucket_id) {
            if bucket.mmr_root == duty.target_mmr_root {
                return SyncResult::AlreadySynced {
                    bucket_id: duty.bucket_id,
                    mmr_root: duty.target_mmr_root,
                };
            }
        }

        // Check sync balance
        if duty.sync_balance < duty.sync_price {
            return SyncResult::InsufficientBalance {
                bucket_id: duty.bucket_id,
                required: duty.sync_price,
                available: duty.sync_balance,
            };
        }

        // No data to sync if target is zero
        if duty.target_mmr_root == H256::zero() {
            return SyncResult::NoDataToSync {
                bucket_id: duty.bucket_id,
            };
        }

        // Try syncing from each primary
        let mut tried_endpoints = Vec::new();
        let mut sync_success = false;

        for endpoint in &duty.primary_endpoints {
            tried_endpoints.push(endpoint.clone());

            match self.sync_from_primary(duty, endpoint).await {
                Ok(synced_root) => {
                    if synced_root == duty.target_mmr_root {
                        sync_success = true;
                        tracing::info!(
                            "Successfully synced bucket {} from {}: root = 0x{}",
                            duty.bucket_id,
                            endpoint,
                            hex::encode(synced_root.as_bytes())
                        );
                        break;
                    } else {
                        tracing::warn!(
                            "Sync mismatch for bucket {} from {}: expected 0x{}, got 0x{}",
                            duty.bucket_id,
                            endpoint,
                            hex::encode(duty.target_mmr_root.as_bytes()),
                            hex::encode(synced_root.as_bytes())
                        );
                    }
                }
                Err(e) => {
                    tracing::warn!(
                        "Failed to sync bucket {} from {}: {}",
                        duty.bucket_id,
                        endpoint,
                        e
                    );
                }
            }
        }

        if !sync_success {
            return SyncResult::PrimaryUnavailable {
                bucket_id: duty.bucket_id,
                tried: tried_endpoints,
            };
        }

        // Verify final state
        let local_bucket = match self.storage.get_bucket(duty.bucket_id) {
            Some(b) => b,
            None => {
                return SyncResult::VerificationFailed {
                    bucket_id: duty.bucket_id,
                    reason: "Bucket not found after sync".to_string(),
                };
            }
        };

        if local_bucket.mmr_root != duty.target_mmr_root {
            return SyncResult::VerificationFailed {
                bucket_id: duty.bucket_id,
                reason: format!(
                    "Root mismatch: expected 0x{}, got 0x{}",
                    hex::encode(duty.target_mmr_root.as_bytes()),
                    hex::encode(local_bucket.mmr_root.as_bytes())
                ),
            };
        }

        // Submit on-chain confirmation if auto_confirm is enabled
        if self.config.auto_confirm {
            match self
                .chain_client
                .submit_sync_confirmation(duty.bucket_id, duty.target_mmr_root)
                .await
            {
                Ok((position, payment)) => SyncResult::Success {
                    bucket_id: duty.bucket_id,
                    mmr_root: duty.target_mmr_root,
                    position_matched: position,
                    payment,
                },
                Err(e) => SyncResult::SubmissionFailed {
                    bucket_id: duty.bucket_id,
                    error: e.to_string(),
                },
            }
        } else {
            SyncResult::Success {
                bucket_id: duty.bucket_id,
                mmr_root: duty.target_mmr_root,
                position_matched: 0,
                payment: 0,
            }
        }
    }

    /// Sync data from a primary provider using top-down traversal.
    async fn sync_from_primary(&self, duty: &SyncDuty, primary_url: &str) -> Result<H256, Error> {
        self.replica_sync
            .sync_from_primary(duty.bucket_id, primary_url)
            .await
    }
}
