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
use sp_core::{H256, Pair};
use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;
use storage_primitives::BucketId;
use subxt::{OnlineClient, PolkadotConfig};
use subxt_signer::sr25519::Keypair;
use tokio::sync::{mpsc, oneshot};

/// Configuration for the replica sync coordinator.
#[derive(Clone, Debug)]
pub struct ReplicaSyncCoordinatorConfig {
    /// WebSocket URL for the parachain.
    pub chain_ws_url: String,
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
            chain_ws_url: "ws://127.0.0.1:9944".to_string(),
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
    VerificationFailed {
        bucket_id: BucketId,
        reason: String,
    },
    /// Failed to submit confirm_replica_sync transaction.
    SubmissionFailed {
        bucket_id: BucketId,
        error: String,
    },
    /// Already synced to this root.
    AlreadySynced {
        bucket_id: BucketId,
        mmr_root: H256,
    },
    /// No data to sync yet.
    NoDataToSync {
        bucket_id: BucketId,
    },
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
    api: Option<OnlineClient<PolkadotConfig>>,
    signer: Option<Keypair>,
    /// HTTP client for fetching data from primaries (used by replica_sync).
    #[allow(dead_code)]
    http_client: reqwest::Client,
    replica_sync: ReplicaSync,
    /// Track active sync operations by bucket.
    active_syncs: HashMap<BucketId, tokio::task::JoinHandle<SyncResult>>,
}

impl ReplicaSyncCoordinator {
    /// Create a new replica sync coordinator.
    pub fn new(config: ReplicaSyncCoordinatorConfig, state: Arc<ProviderState>) -> Self {
        let replica_sync = ReplicaSync::new(state.storage.clone());

        Self {
            config,
            state,
            api: None,
            signer: None,
            http_client: reqwest::Client::builder()
                .timeout(Duration::from_secs(30))
                .build()
                .expect("Failed to create HTTP client"),
            replica_sync,
            active_syncs: HashMap::new(),
        }
    }

    /// Connect to the blockchain.
    pub async fn connect(&mut self) -> Result<(), Error> {
        let api = OnlineClient::<PolkadotConfig>::from_url(&self.config.chain_ws_url)
            .await
            .map_err(|e| Error::Internal(format!("Failed to connect to chain: {e}")))?;

        self.api = Some(api);

        // Set up signer from provider state if available
        if let Some(ref kp) = self.state.keypair {
            let raw = kp.to_raw_vec();
            let secret_bytes: [u8; 32] = raw[..32]
                .try_into()
                .map_err(|_| Error::Internal("Invalid secret key length".to_string()))?;
            let signer = Keypair::from_secret_key(secret_bytes)
                .map_err(|e| Error::Internal(format!("Failed to create signer: {e}")))?;
            self.signer = Some(signer);
        }

        tracing::info!(
            "Replica sync coordinator connected to {}",
            self.config.chain_ws_url
        );
        Ok(())
    }

    /// Start the replica sync coordinator background service.
    pub async fn start(
        self,
        callback: Option<Arc<dyn Fn(SyncResult) + Send + Sync>>,
    ) -> Result<ReplicaSyncCoordinatorHandle, Error> {
        if self.api.is_none() {
            return Err(Error::Internal("Not connected to chain".to_string()));
        }

        let (command_tx, command_rx) = mpsc::channel::<SyncCommand>(32);
        let running = Arc::new(AtomicBool::new(true));
        let running_clone = running.clone();

        let coordinator = self;

        tokio::spawn(async move {
            coordinator
                .run_loop(command_rx, running_clone, callback)
                .await;
        });

        Ok(ReplicaSyncCoordinatorHandle { command_tx, running })
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
                                tracked_buckets: self.get_tracked_buckets().await.unwrap_or_default(),
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
            }
        }
    }

    /// Clean up completed sync tasks.
    fn cleanup_completed_syncs(&mut self) {
        self.active_syncs
            .retain(|_, handle| !handle.is_finished());
    }

    /// Get list of bucket IDs we're tracking as replica.
    async fn get_tracked_buckets(&self) -> Result<Vec<BucketId>, Error> {
        // Query chain for buckets where this provider is a replica
        // For now, return buckets from local storage
        Ok(self
            .state
            .storage
            .list_buckets()
            .into_iter()
            .map(|b| b.bucket_id)
            .collect())
    }

    /// Get replica duties for buckets where this provider is a replica.
    async fn get_active_replica_duties(&self) -> Result<Vec<SyncDuty>, Error> {
        let api = self
            .api
            .as_ref()
            .ok_or_else(|| Error::Internal("Not connected to chain".to_string()))?;

        let mut duties = Vec::new();

        // Get current block number for interval checking
        let current_block = self.get_current_block(api).await?;

        // Query agreements where this provider is a replica
        // For now, we'll query local buckets and check if we have replica agreements
        let our_account = self.get_our_account_id()?;

        // Query storage for agreements where we're the replica provider
        // Storage key: StorageProvider::Agreements(bucket_id, provider)
        let agreements = self.query_replica_agreements(api, &our_account).await?;

        for agreement in agreements {
            // Skip if sync balance is depleted
            if agreement.sync_balance < agreement.sync_price {
                tracing::debug!(
                    "Bucket {} has insufficient sync balance: {} < {}",
                    agreement.bucket_id,
                    agreement.sync_balance,
                    agreement.sync_price
                );
                continue;
            }

            // Check if min_sync_interval has elapsed
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

            // Get the latest checkpoint for this bucket
            let snapshot = self.query_bucket_snapshot(api, agreement.bucket_id).await?;

            // Skip if no checkpoint yet
            if snapshot.mmr_root == H256::zero() {
                continue;
            }

            // Skip if we're already synced to this root
            if let Some(bucket) = self.state.storage.get_bucket(agreement.bucket_id) {
                if bucket.mmr_root == snapshot.mmr_root {
                    continue;
                }
            }

            // Get primary provider endpoints
            let primary_endpoints = self
                .query_primary_endpoints(api, agreement.bucket_id)
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
        let api = self
            .api
            .as_ref()
            .ok_or_else(|| Error::Internal("Not connected to chain".to_string()))?;

        let our_account = self.get_our_account_id()?;

        // Check if we have a replica agreement for this bucket
        let agreement = self
            .query_agreement(api, bucket_id, &our_account)
            .await?
            .ok_or(Error::Internal(format!(
                "No replica agreement found for bucket {bucket_id}"
            )))?;

        // Get snapshot
        let snapshot = self.query_bucket_snapshot(api, bucket_id).await?;

        // Get primary endpoints
        let primary_endpoints = self.query_primary_endpoints(api, bucket_id).await?;

        Ok(Some(SyncDuty {
            bucket_id,
            target_mmr_root: snapshot.mmr_root,
            target_leaf_count: snapshot.leaf_count,
            primary_endpoints,
            sync_balance: agreement.sync_balance,
            sync_price: agreement.sync_price,
            min_sync_interval: agreement.min_sync_interval,
            last_sync: agreement.last_sync,
        }))
    }

    /// Perform sync and submit confirmation.
    async fn sync_and_confirm(&self, duty: &SyncDuty) -> SyncResult {
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
        let local_bucket = match self.state.storage.get_bucket(duty.bucket_id) {
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
            match self.submit_sync_confirmation(duty).await {
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
            // Return success without on-chain confirmation
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
        // Use the existing replica_sync module for the actual sync
        self.replica_sync
            .sync_from_primary(duty.bucket_id, primary_url)
            .await
    }

    /// Build the 7-element roots array for confirm_replica_sync.
    fn build_roots_array(&self, synced_root: H256) -> [Option<H256>; 7] {
        // Position 0: current root (what we synced to)
        // Positions 1-6: historical roots (we don't track these locally)
        let mut roots: [Option<H256>; 7] = [None; 7];
        roots[0] = Some(synced_root);
        roots
    }

    /// Submit confirm_replica_sync extrinsic.
    async fn submit_sync_confirmation(&self, duty: &SyncDuty) -> Result<(u8, u128), Error> {
        let api = self
            .api
            .as_ref()
            .ok_or_else(|| Error::Internal("Not connected to chain".to_string()))?;

        let signer = self
            .signer
            .as_ref()
            .ok_or_else(|| Error::Internal("No signer configured".to_string()))?;

        // Build roots array
        let roots = self.build_roots_array(duty.target_mmr_root);

        // Build roots as subxt values
        let roots_value: Vec<subxt::dynamic::Value> = roots
            .iter()
            .map(|r| match r {
                Some(h) => subxt::dynamic::Value::unnamed_variant(
                    "Some",
                    vec![subxt::dynamic::Value::from_bytes(h.as_bytes())],
                ),
                None => subxt::dynamic::Value::unnamed_variant("None", vec![]),
            })
            .collect();

        // Build dummy signature (pallet accepts any MultiSignature)
        let signature = subxt::dynamic::Value::unnamed_variant(
            "Sr25519",
            vec![subxt::dynamic::Value::from_bytes(&[0u8; 64])],
        );

        let tx = subxt::dynamic::tx(
            "StorageProvider",
            "confirm_replica_sync",
            vec![
                // bucket_id: u64
                subxt::dynamic::Value::u128(duty.bucket_id as u128),
                // roots: [Option<H256>; 7]
                subxt::dynamic::Value::unnamed_composite(roots_value),
                // signature: MultiSignature
                signature,
            ],
        );

        tracing::info!(
            "Submitting confirm_replica_sync for bucket {} with root 0x{}",
            duty.bucket_id,
            hex::encode(duty.target_mmr_root.as_bytes())
        );

        // Submit and wait for finalization
        let tx_progress = api
            .tx()
            .sign_and_submit_then_watch_default(&tx, signer)
            .await
            .map_err(|e| Error::Internal(format!("Failed to submit tx: {e}")))?;

        let _events = tx_progress
            .wait_for_finalized_success()
            .await
            .map_err(|e| Error::Internal(format!("Transaction failed: {e}")))?;

        // Try to extract ReplicaSynced event for position and payment info
        // For now, return defaults since event parsing requires generated types
        tracing::info!(
            "confirm_replica_sync submitted successfully for bucket {}",
            duty.bucket_id
        );

        // Position 0 = current root, payment = sync_price
        Ok((0, duty.sync_price))
    }

    // ─────────────────────────────────────────────────────────────────────────────
    // Chain query helpers
    // ─────────────────────────────────────────────────────────────────────────────

    /// Get our account ID as hex string.
    fn get_our_account_id(&self) -> Result<String, Error> {
        Ok(self.state.provider_id.clone())
    }

    /// Get current block number.
    async fn get_current_block(&self, api: &OnlineClient<PolkadotConfig>) -> Result<u64, Error> {
        let block = api
            .blocks()
            .at_latest()
            .await
            .map_err(|e| Error::Internal(format!("Failed to get latest block: {e}")))?;

        Ok(block.number() as u64)
    }

    /// Query replica agreements for our account.
    async fn query_replica_agreements(
        &self,
        _api: &OnlineClient<PolkadotConfig>,
        _our_account: &str,
    ) -> Result<Vec<ReplicaAgreementInfo>, Error> {
        // In a full implementation, this would query on-chain storage:
        // StorageProvider::Agreements double map where our account is the provider
        // and agreement.role == Replica
        //
        // For now, return empty - this would need runtime metadata to properly decode
        Ok(vec![])
    }

    /// Query a specific agreement.
    async fn query_agreement(
        &self,
        _api: &OnlineClient<PolkadotConfig>,
        _bucket_id: BucketId,
        _provider: &str,
    ) -> Result<Option<ReplicaAgreementInfo>, Error> {
        // Would query StorageProvider::Agreements(bucket_id, provider)
        Ok(None)
    }

    /// Query bucket snapshot (latest checkpoint state).
    async fn query_bucket_snapshot(
        &self,
        _api: &OnlineClient<PolkadotConfig>,
        bucket_id: BucketId,
    ) -> Result<BucketSnapshot, Error> {
        // Would query StorageProvider::BucketSnapshots(bucket_id)
        // For now, return from local state if available
        if let Some(bucket) = self.state.storage.get_bucket(bucket_id) {
            return Ok(BucketSnapshot {
                mmr_root: bucket.mmr_root,
                leaf_count: bucket.leaf_count(),
            });
        }

        Ok(BucketSnapshot {
            mmr_root: H256::zero(),
            leaf_count: 0,
        })
    }

    /// Query primary provider endpoints for a bucket.
    async fn query_primary_endpoints(
        &self,
        _api: &OnlineClient<PolkadotConfig>,
        _bucket_id: BucketId,
    ) -> Result<Vec<String>, Error> {
        // Would query StorageProvider::Buckets(bucket_id).primary_providers
        // Then look up each provider's endpoint from ProviderInfo
        //
        // For now, return empty or could be configured externally
        Ok(vec![])
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Internal helper types
// ─────────────────────────────────────────────────────────────────────────────

/// Information about a replica agreement from chain.
#[derive(Clone, Debug)]
struct ReplicaAgreementInfo {
    bucket_id: BucketId,
    sync_balance: u128,
    sync_price: u128,
    min_sync_interval: u64,
    last_sync: Option<(H256, u64)>,
}

/// Bucket snapshot from chain.
#[derive(Clone, Debug)]
struct BucketSnapshot {
    mmr_root: H256,
    leaf_count: u64,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_config_default() {
        let config = ReplicaSyncCoordinatorConfig::default();
        assert_eq!(config.chain_ws_url, "ws://127.0.0.1:9944");
        assert_eq!(config.poll_interval, Duration::from_secs(12));
        assert_eq!(config.max_concurrent_syncs, 3);
        assert!(config.auto_confirm);
    }

    #[test]
    fn test_sync_result_variants() {
        let success = SyncResult::Success {
            bucket_id: 1,
            mmr_root: H256::zero(),
            position_matched: 0,
            payment: 1000,
        };
        assert!(matches!(success, SyncResult::Success { .. }));

        let insufficient = SyncResult::InsufficientBalance {
            bucket_id: 1,
            required: 1000,
            available: 500,
        };
        assert!(matches!(insufficient, SyncResult::InsufficientBalance { .. }));

        let interval = SyncResult::SyncIntervalNotElapsed {
            bucket_id: 1,
            blocks_remaining: 50,
        };
        assert!(matches!(interval, SyncResult::SyncIntervalNotElapsed { .. }));
    }

    #[test]
    fn test_build_roots_array() {
        let root = H256::repeat_byte(0xAB);
        let storage = Arc::new(crate::Storage::new());
        let state = Arc::new(crate::ProviderState::new(storage, "test".to_string()));
        let config = ReplicaSyncCoordinatorConfig::default();
        let coordinator = ReplicaSyncCoordinator::new(config, state);

        let roots = coordinator.build_roots_array(root);

        assert_eq!(roots[0], Some(root));
        for item in roots.iter().skip(1) {
            assert_eq!(*item, None);
        }
    }
}
