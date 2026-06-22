// SPDX-License-Identifier: GPL-3.0-only

//! Replica Sync Coordinator - Autonomous replica synchronization service.
//!
//! This module provides a background service that:
//! 1. Subscribes to checkpoint events on-chain
//! 2. Detects when new data is available to sync
//! 3. Performs top-down MMR traversal to fetch missing data from primaries
//! 4. Submits `confirm_replica_sync` transactions to receive payment
//! 5. Handles historical roots matching for late syncs

use crate::replica_sync::ReplicaSync;
use crate::{Error, ProviderState};
use sp_core::H256;
use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;
use storage_primitives::BucketId;
use tokio::sync::{mpsc, oneshot};

/// Configuration for the replica sync coordinator.
#[derive(Clone, Debug)]
pub struct ReplicaSyncCoordinatorConfig {
    /// How often to poll for sync duties (default: 12 seconds = ~2 blocks).
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
            poll_interval: Duration::from_secs(12),
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
    /// Primary provider endpoints — used as fallback when no peer replicas are
    /// available.
    pub primary_endpoints: Vec<String>,
    /// Other replica endpoints for this bucket. When non-empty, these are tried
    /// first so the primary is not hit by every replica on every sync cycle.
    pub peer_replica_endpoints: Vec<String>,
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

    /// Fetch HTTP endpoints of other replicas for a bucket (excludes this node's
    /// own account so we don't sync from ourselves).
    async fn fetch_peer_replica_endpoints(
        &self,
        bucket_id: BucketId,
        own_account: &str,
    ) -> Result<Vec<String>, Error>;

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

    async fn fetch_peer_replica_endpoints(
        &self,
        bucket_id: BucketId,
        own_account: &str,
    ) -> Result<Vec<String>, Error> {
        self.as_ref()
            .fetch_peer_replica_endpoints(bucket_id, own_account)
            .await
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
    state: Arc<ProviderState>,
    chain_client: Box<dyn ReplicaSyncChainClient>,
    replica_sync: ReplicaSync,
    /// Track active sync operations by bucket.
    active_syncs: HashMap<BucketId, tokio::task::JoinHandle<SyncResult>>,
}

impl ReplicaSyncCoordinator {
    /// Create a new replica sync coordinator.
    pub fn new(
        config: ReplicaSyncCoordinatorConfig,
        state: Arc<ProviderState>,
        chain_client: Box<dyn ReplicaSyncChainClient>,
    ) -> Self {
        let replica_sync = ReplicaSync::new(state.storage.clone());

        Self {
            config,
            state,
            chain_client,
            replica_sync,
            active_syncs: HashMap::new(),
        }
    }

    /// Start the replica sync coordinator background service.
    pub async fn start(
        self,
        callback: Option<Arc<dyn Fn(SyncResult) + Send + Sync>>,
    ) -> Result<ReplicaSyncCoordinatorHandle, Error> {
        let (command_tx, command_rx) = mpsc::channel::<SyncCommand>(32);
        let running = Arc::new(AtomicBool::new(true));
        let running_clone = running.clone();

        tokio::spawn(async move {
            self.run_loop(command_rx, running_clone, callback).await;
        });

        Ok(ReplicaSyncCoordinatorHandle {
            command_tx,
            running,
        })
    }

    /// Main coordinator loop.
    async fn run_loop(
        mut self,
        mut command_rx: mpsc::Receiver<SyncCommand>,
        running: Arc<AtomicBool>,
        callback: Option<Arc<dyn Fn(SyncResult) + Send + Sync>>,
    ) {
        let mut paused = false;
        let mut interval = tokio::time::interval(self.config.poll_interval);

        tracing::info!("Replica sync coordinator started");

        loop {
            tokio::select! {
                // Prefer control commands over the poll tick: the interval's
                // first tick fires immediately, so an unbiased select could
                // service a poll before a Pause/Stop queued right after start().
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
                _ = interval.tick() => {
                    if paused || !self.config.auto_confirm {
                        continue;
                    }

                    // Clean up completed syncs
                    self.cleanup_completed_syncs();

                    // Get active replica duties
                    match self.get_active_replica_duties().await {
                        Ok(duties) => {
                            for duty in duties {
                                tracing::info!("self.active_syncs.contains_key(&duty.bucket_id) {}", self.active_syncs.contains_key(&duty.bucket_id));
                                // Skip if already syncing this bucket
                                if self.active_syncs.contains_key(&duty.bucket_id) {
                                    continue;
                                }

                                tracing::info!("self.active_syncs.len() >= self.config.max_concurrent_syncs: {}", self.active_syncs.len() >= self.config.max_concurrent_syncs);
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
                                tracing::info!("Sync result: {:?}", result);
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
            }
        }
    }

    /// Clean up completed sync tasks.
    fn cleanup_completed_syncs(&mut self) {
        self.active_syncs.retain(|_, handle| !handle.is_finished());
    }

    /// Get list of bucket IDs we're tracking as replica.
    fn get_tracked_buckets(&self) -> Vec<BucketId> {
        self.state
            .storage
            .list_buckets()
            .into_iter()
            .map(|b| b.bucket_id)
            .collect()
    }

    /// Get replica duties for buckets where this provider is a replica.
    pub async fn get_active_replica_duties(&self) -> Result<Vec<SyncDuty>, Error> {
        let mut duties = Vec::new();

        let current_block = self.chain_client.get_current_block().await?;

        // Buckets for which this node holds a replica agreement — sourced from
        // the chain-state coordinator (populated on ReplicaAgreementEstablished
        // events) so a fresh node with no local data can still discover its duties.
        let tracked_buckets: Vec<BucketId> = self
            .state
            .chain_state
            .replica_buckets
            .read()
            .iter()
            .copied()
            .collect();

        tracing::debug!("tracked_buckets: {:#?}", tracked_buckets);
        let provider_account = self.state.provider_id.clone();

        let agreements = self
            .chain_client
            .fetch_replica_agreements(&provider_account, tracked_buckets)
            .await?;

        tracing::debug!("agreements: {:#?}", agreements);
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
                let elapsed = current_block.saturating_sub(last_block);
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

            if let Some(bucket) = self.state.storage.get_bucket(agreement.bucket_id) {
                tracing::debug!("Latest snapshot.mmr_root {:?}\nStorage bucket snapshot.mmr_root {}", snapshot.mmr_root, bucket.mmr_root);
                if bucket.mmr_root == snapshot.mmr_root {
                    tracing::debug!("synced to latest state!");
                    continue;
                }
            }

            let primary_endpoints = self
                .chain_client
                .fetch_primary_endpoints(agreement.bucket_id)
                .await?;

            let peer_replica_endpoints = self
                .chain_client
                .fetch_peer_replica_endpoints(agreement.bucket_id, &provider_account)
                .await
                .unwrap_or_default();

            duties.push(SyncDuty {
                bucket_id: agreement.bucket_id,
                target_mmr_root: snapshot.mmr_root,
                target_leaf_count: snapshot.leaf_count,
                primary_endpoints,
                peer_replica_endpoints,
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

        let peer_replica_endpoints = self
            .chain_client
            .fetch_peer_replica_endpoints(bucket_id, &self.state.provider_id)
            .await
            .unwrap_or_default();

        Ok(Some(SyncDuty {
            bucket_id,
            target_mmr_root: snapshot.mmr_root,
            target_leaf_count: snapshot.leaf_count,
            primary_endpoints,
            peer_replica_endpoints,
            sync_balance: u128::MAX,
            sync_price: 0,
            min_sync_interval: 0,
            last_sync: None,
        }))
    }

    /// Perform sync and submit confirmation.
    pub async fn sync_and_confirm(&self, duty: &SyncDuty) -> SyncResult {
        // Check if we already have this root
        if let Some(bucket) = self.state.storage.get_bucket(duty.bucket_id) {
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

        // Prefer syncing from a peer replica when others are available: this
        // offloads the primary and spreads transfer cost across the replica set.
        // Fall back to the primary only when no peer succeeds.
        //
        // Selection rule:
        //   - no peer replicas  → sync directly from primary (as before)
        //   - peer(s) exist     → try peers first; if all fail, try primary
        let candidates: Vec<(&str, bool)> = duty
            .peer_replica_endpoints
            .iter()
            .map(|e| (e.as_str(), false))
            .chain(duty.primary_endpoints.iter().map(|e| (e.as_str(), true)))
            .collect();

        let mut tried_endpoints = Vec::new();
        let mut sync_success = false;

        for (endpoint, is_primary) in &candidates {
            tried_endpoints.push(endpoint.to_string());
            match self.sync_from_primary(duty, endpoint).await {
                Ok(synced_root) if synced_root != H256::zero() => {
                    sync_success = true;
                    // The primary's live root may be ahead of the checkpoint root
                    // (more uploads since last checkpoint). That is fine: the MMR is
                    // append-only, so the replica holds all checkpointed data and the
                    // confirmation is submitted for duty.target_mmr_root which the
                    // pallet validates against its historical-roots list.
                    tracing::info!(
                        "Successfully synced bucket {} from {} ({}): synced=0x{} target=0x{}",
                        duty.bucket_id,
                        endpoint,
                        if *is_primary {
                            "primary"
                        } else {
                            "peer replica"
                        },
                        hex::encode(synced_root.as_bytes()),
                        hex::encode(duty.target_mmr_root.as_bytes())
                    );
                    break;
                }
                Ok(_) => {
                    tracing::warn!(
                        "Sync returned zero root for bucket {} from {}",
                        duty.bucket_id,
                        endpoint
                    );
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
        let _local_bucket = match self.state.storage.get_bucket(duty.bucket_id) {
            Some(b) => b,
            None => {
                return SyncResult::VerificationFailed {
                    bucket_id: duty.bucket_id,
                    reason: "Bucket not found after sync".to_string(),
                };
            }
        };

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
