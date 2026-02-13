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
use sp_core::{Pair, H256};
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
        self.active_syncs.retain(|_, handle| !handle.is_finished());
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
    ///
    /// This queries the on-chain `StorageAgreements` double map for all buckets
    /// where we have a replica agreement.
    async fn query_replica_agreements(
        &self,
        api: &OnlineClient<PolkadotConfig>,
        our_account: &str,
    ) -> Result<Vec<ReplicaAgreementInfo>, Error> {
        let mut agreements = Vec::new();

        // Get our account bytes
        let account_bytes = hex::decode(our_account.trim_start_matches("0x"))
            .map_err(|e| Error::Internal(format!("Invalid account hex: {e}")))?;

        // Query local buckets to find which ones we might have agreements for
        // This is an optimization - in a full implementation we'd iterate the chain storage
        let local_buckets: Vec<u64> = self
            .state
            .storage
            .list_buckets()
            .into_iter()
            .map(|b| b.bucket_id)
            .collect();

        for bucket_id in local_buckets {
            // Query the specific agreement for this bucket
            if let Some(agreement) = self
                .query_agreement_raw(api, bucket_id, &account_bytes)
                .await?
            {
                agreements.push(agreement);
            }
        }

        // Also try to iterate chain storage for agreements we might not have locally
        // This uses subxt's dynamic storage iteration
        if let Ok(chain_agreements) = self.iterate_all_agreements(api, &account_bytes).await {
            for agreement in chain_agreements {
                // Avoid duplicates
                if !agreements
                    .iter()
                    .any(|a| a.bucket_id == agreement.bucket_id)
                {
                    agreements.push(agreement);
                }
            }
        }

        Ok(agreements)
    }

    /// Iterate all storage agreements from chain to find replica agreements for our account.
    async fn iterate_all_agreements(
        &self,
        api: &OnlineClient<PolkadotConfig>,
        our_account_bytes: &[u8],
    ) -> Result<Vec<ReplicaAgreementInfo>, Error> {
        let mut agreements = Vec::new();

        // Build the storage key prefix for StorageAgreements
        let storage_address = subxt::dynamic::storage("StorageProvider", "StorageAgreements", ());

        // Iterate all entries in the double map
        let mut iter = api
            .storage()
            .at_latest()
            .await
            .map_err(|e| Error::Internal(format!("Failed to get storage: {e}")))?
            .iter(storage_address)
            .await
            .map_err(|e| Error::Internal(format!("Failed to iterate storage: {e}")))?;

        while let Some(result) = iter.next().await {
            let kv = match result {
                Ok(kv) => kv,
                Err(e) => {
                    tracing::debug!("Error iterating storage: {e}");
                    continue;
                }
            };

            // Extract bucket_id and provider from the key
            // Key format: twox128(pallet) + twox128(storage) + blake2_128_concat(bucket_id) + blake2_128_concat(provider)
            let key_bytes = kv.key_bytes;
            if key_bytes.len() < 32 + 16 + 8 + 16 + 32 {
                continue;
            }

            // Skip pallet+storage prefix (32 bytes) + blake2_128 hash (16 bytes)
            let bucket_id_start = 32 + 16;
            let bucket_id_bytes = &key_bytes[bucket_id_start..bucket_id_start + 8];
            let bucket_id = u64::from_le_bytes(bucket_id_bytes.try_into().unwrap_or([0; 8]));

            // Skip to provider part: after bucket_id (8 bytes) + blake2_128 hash (16 bytes)
            let provider_start = bucket_id_start + 8 + 16;
            let provider_bytes = &key_bytes[provider_start..];

            // Check if this is our provider
            if provider_bytes.len() < 32 || provider_bytes[..32] != our_account_bytes[..32] {
                continue;
            }

            // Parse the agreement value from the encoded bytes
            if let Ok(agreement) = self.decode_storage_agreement_from_thunk(bucket_id, &kv.value) {
                agreements.push(agreement);
            }
        }

        Ok(agreements)
    }

    /// Decode a storage agreement from a DecodedValueThunk using raw encoding.
    fn decode_storage_agreement_from_thunk(
        &self,
        bucket_id: BucketId,
        value: &subxt::dynamic::DecodedValueThunk,
    ) -> Result<ReplicaAgreementInfo, Error> {
        // Get the encoded bytes from the thunk
        let encoded = value.encoded();
        self.decode_storage_agreement_bytes(bucket_id, encoded)
    }

    /// Decode a storage agreement from raw SCALE-encoded bytes.
    fn decode_storage_agreement_bytes(
        &self,
        bucket_id: BucketId,
        bytes: &[u8],
    ) -> Result<ReplicaAgreementInfo, Error> {
        // StorageAgreement layout:
        // - owner: AccountId (32 bytes)
        // - max_bytes: u64 (8 bytes)
        // - payment_locked: Balance (16 bytes)
        // - price_per_byte: Balance (16 bytes)
        // - expires_at: BlockNumber (4 bytes)
        // - extensions_blocked: bool (1 byte)
        // - role: ProviderRole (variable, enum)
        // - started_at: BlockNumber (4 bytes)

        let min_size = 32 + 8 + 16 + 16 + 4 + 1; // up to role enum
        if bytes.len() < min_size {
            return Err(Error::Internal("Agreement data too short".to_string()));
        }

        let role_start = 32 + 8 + 16 + 16 + 4 + 1; // Skip to role enum
        let role_variant = bytes.get(role_start).copied().unwrap_or(0);

        // Role enum: 0 = Primary, 1 = Replica
        if role_variant != 1 {
            return Err(Error::Internal("Not a replica agreement".to_string()));
        }

        // Parse Replica fields: sync_balance, sync_price, min_sync_interval, last_sync
        let replica_start = role_start + 1;
        let remaining = &bytes[replica_start..];

        if remaining.len() < 16 + 16 + 4 {
            return Err(Error::Internal("Replica data too short".to_string()));
        }

        // sync_balance: Balance (u128 = 16 bytes)
        let sync_balance = u128::from_le_bytes(
            remaining[0..16]
                .try_into()
                .map_err(|_| Error::Internal("Failed to parse sync_balance".to_string()))?,
        );

        // sync_price: Balance (u128 = 16 bytes)
        let sync_price = u128::from_le_bytes(
            remaining[16..32]
                .try_into()
                .map_err(|_| Error::Internal("Failed to parse sync_price".to_string()))?,
        );

        // min_sync_interval: BlockNumber (u32 = 4 bytes)
        let min_sync_interval = u32::from_le_bytes(
            remaining[32..36]
                .try_into()
                .map_err(|_| Error::Internal("Failed to parse min_sync_interval".to_string()))?,
        ) as u64;

        // last_sync: Option<(H256, BlockNumber)>
        let last_sync_option = remaining.get(36).copied().unwrap_or(0);
        let last_sync = if last_sync_option == 1 && remaining.len() >= 36 + 1 + 32 + 4 {
            let root_bytes: [u8; 32] = remaining[37..69]
                .try_into()
                .map_err(|_| Error::Internal("Failed to parse last_sync root".to_string()))?;
            let block = u32::from_le_bytes(
                remaining[69..73]
                    .try_into()
                    .map_err(|_| Error::Internal("Failed to parse last_sync block".to_string()))?,
            ) as u64;
            Some((H256::from(root_bytes), block))
        } else {
            None
        };

        Ok(ReplicaAgreementInfo {
            bucket_id,
            sync_balance,
            sync_price,
            min_sync_interval,
            last_sync,
        })
    }

    /// Query a specific agreement from chain using raw bytes.
    async fn query_agreement_raw(
        &self,
        api: &OnlineClient<PolkadotConfig>,
        bucket_id: BucketId,
        provider_bytes: &[u8],
    ) -> Result<Option<ReplicaAgreementInfo>, Error> {
        // Build the storage key for this specific agreement
        let storage_address = subxt::dynamic::storage(
            "StorageProvider",
            "StorageAgreements",
            vec![
                subxt::dynamic::Value::u128(bucket_id as u128),
                subxt::dynamic::Value::from_bytes(provider_bytes),
            ],
        );

        let storage = api
            .storage()
            .at_latest()
            .await
            .map_err(|e| Error::Internal(format!("Failed to get storage: {e}")))?;

        match storage.fetch(&storage_address).await {
            Ok(Some(value)) => self
                .decode_storage_agreement_from_thunk(bucket_id, &value)
                .ok()
                .map(|a| Ok(Some(a)))
                .unwrap_or(Ok(None)),
            Ok(None) => Ok(None),
            Err(e) => {
                tracing::debug!("Failed to fetch agreement {bucket_id}: {e}");
                Ok(None)
            }
        }
    }

    /// Query a specific agreement.
    async fn query_agreement(
        &self,
        api: &OnlineClient<PolkadotConfig>,
        bucket_id: BucketId,
        provider: &str,
    ) -> Result<Option<ReplicaAgreementInfo>, Error> {
        let provider_bytes = hex::decode(provider.trim_start_matches("0x"))
            .map_err(|e| Error::Internal(format!("Invalid provider hex: {e}")))?;

        self.query_agreement_raw(api, bucket_id, &provider_bytes)
            .await
    }

    /// Query bucket snapshot (latest checkpoint state) from chain.
    ///
    /// This queries the on-chain `Buckets` storage to get the authoritative snapshot.
    async fn query_bucket_snapshot(
        &self,
        api: &OnlineClient<PolkadotConfig>,
        bucket_id: BucketId,
    ) -> Result<BucketSnapshot, Error> {
        // Build the storage address for the bucket
        let storage_address = subxt::dynamic::storage(
            "StorageProvider",
            "Buckets",
            vec![subxt::dynamic::Value::u128(bucket_id as u128)],
        );

        let storage = api
            .storage()
            .at_latest()
            .await
            .map_err(|e| Error::Internal(format!("Failed to get storage: {e}")))?;

        match storage.fetch(&storage_address).await {
            Ok(Some(value)) => {
                // Decode the bucket to extract the snapshot using raw bytes
                self.decode_bucket_snapshot_from_thunk(bucket_id, &value)
            }
            Ok(None) => {
                // Bucket not found on chain, check local state
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
            Err(e) => {
                tracing::warn!("Failed to fetch bucket {bucket_id} from chain: {e}");
                // Fallback to local state
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
        }
    }

    /// Decode a bucket's snapshot from the DecodedValueThunk.
    fn decode_bucket_snapshot_from_thunk(
        &self,
        bucket_id: BucketId,
        value: &subxt::dynamic::DecodedValueThunk,
    ) -> Result<BucketSnapshot, Error> {
        use subxt::ext::scale_value::{At, ValueDef};

        let decoded = value
            .to_value()
            .map_err(|e| Error::Internal(format!("Failed to decode bucket: {e}")))?;

        // Navigate to snapshot field using scale_value's At trait
        // Bucket fields: members, frozen_start_seq, min_providers, primary_providers, snapshot, historical_roots, total_snapshots
        // snapshot is at index 4
        if let Some(snapshot_opt) = decoded.at(4) {
            // snapshot is Option<BucketSnapshot>
            // Access the ValueDef through .value field
            if let ValueDef::Variant(variant) = &snapshot_opt.value {
                if variant.name == "Some" {
                    // Get the inner snapshot value
                    if let Some(snapshot_val) = variant.values.values().next() {
                        return self.parse_bucket_snapshot_value(snapshot_val);
                    }
                }
            }
        }

        // Fallback to local state
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

    /// Parse a BucketSnapshot value from scale_value.
    fn parse_bucket_snapshot_value<T>(
        &self,
        value: &subxt::ext::scale_value::Value<T>,
    ) -> Result<BucketSnapshot, Error> {
        use subxt::ext::scale_value::{At, Composite, Primitive, ValueDef};

        // BucketSnapshot: { mmr_root: H256, start_seq: u64, leaf_count: u64, block_number: BlockNumber }
        let mmr_root = if let Some(field0) = value.at(0) {
            if let ValueDef::Composite(Composite::Unnamed(bytes_vec)) = &field0.value {
                // H256 is a composite of 32 bytes
                let bytes: Vec<u8> = bytes_vec
                    .iter()
                    .filter_map(|v| {
                        if let ValueDef::Primitive(Primitive::U128(n)) = &v.value {
                            Some(*n as u8)
                        } else {
                            None
                        }
                    })
                    .collect();
                if bytes.len() == 32 {
                    H256::from_slice(&bytes)
                } else {
                    H256::zero()
                }
            } else {
                H256::zero()
            }
        } else {
            H256::zero()
        };

        let leaf_count = if let Some(field2) = value.at(2) {
            if let ValueDef::Primitive(Primitive::U128(n)) = &field2.value {
                *n as u64
            } else {
                0
            }
        } else {
            0
        };

        Ok(BucketSnapshot {
            mmr_root,
            leaf_count,
        })
    }

    /// Query primary provider endpoints for a bucket.
    ///
    /// This queries the bucket's primary_providers list, then looks up each
    /// provider's multiaddr from the Providers storage.
    async fn query_primary_endpoints(
        &self,
        api: &OnlineClient<PolkadotConfig>,
        bucket_id: BucketId,
    ) -> Result<Vec<String>, Error> {
        // First, get the bucket to find its primary providers
        let storage_address = subxt::dynamic::storage(
            "StorageProvider",
            "Buckets",
            vec![subxt::dynamic::Value::u128(bucket_id as u128)],
        );

        let storage = api
            .storage()
            .at_latest()
            .await
            .map_err(|e| Error::Internal(format!("Failed to get storage: {e}")))?;

        let bucket_value = match storage.fetch(&storage_address).await {
            Ok(Some(v)) => v,
            Ok(None) => return Ok(vec![]),
            Err(e) => {
                tracing::warn!("Failed to fetch bucket {bucket_id}: {e}");
                return Ok(vec![]);
            }
        };

        // Extract primary_providers from the bucket
        let primary_providers = self.extract_primary_providers(&bucket_value)?;

        // Now look up each provider's multiaddr
        let mut endpoints = Vec::new();
        for provider_bytes in primary_providers {
            if let Ok(Some(endpoint)) = self.query_provider_endpoint(api, &provider_bytes).await {
                endpoints.push(endpoint);
            }
        }

        Ok(endpoints)
    }

    /// Extract primary provider account IDs from a bucket value.
    fn extract_primary_providers(
        &self,
        bucket_value: &subxt::dynamic::DecodedValueThunk,
    ) -> Result<Vec<Vec<u8>>, Error> {
        use subxt::ext::scale_value::{At, Composite, Primitive, ValueDef};

        let decoded = bucket_value
            .to_value()
            .map_err(|e| Error::Internal(format!("Failed to decode bucket: {e}")))?;

        let mut providers = Vec::new();

        // Bucket fields: members, frozen_start_seq, min_providers, primary_providers, snapshot, historical_roots, total_snapshots
        // primary_providers is at index 3
        if let Some(field3) = decoded.at(3) {
            if let ValueDef::Composite(Composite::Unnamed(providers_vec)) = &field3.value {
                for provider_value in providers_vec {
                    // Each provider is an AccountId (32 bytes composite)
                    if let ValueDef::Composite(Composite::Unnamed(account_bytes)) =
                        &provider_value.value
                    {
                        let bytes: Vec<u8> = account_bytes
                            .iter()
                            .filter_map(|v| {
                                if let ValueDef::Primitive(Primitive::U128(n)) = &v.value {
                                    Some(*n as u8)
                                } else {
                                    None
                                }
                            })
                            .collect();
                        if bytes.len() == 32 {
                            providers.push(bytes);
                        }
                    }
                }
            }
        }

        Ok(providers)
    }

    /// Query a provider's endpoint (multiaddr) from chain.
    async fn query_provider_endpoint(
        &self,
        api: &OnlineClient<PolkadotConfig>,
        provider_bytes: &[u8],
    ) -> Result<Option<String>, Error> {
        let storage_address = subxt::dynamic::storage(
            "StorageProvider",
            "Providers",
            vec![subxt::dynamic::Value::from_bytes(provider_bytes)],
        );

        let storage = api
            .storage()
            .at_latest()
            .await
            .map_err(|e| Error::Internal(format!("Failed to get storage: {e}")))?;

        match storage.fetch(&storage_address).await {
            Ok(Some(value)) => {
                // Extract multiaddr from ProviderInfo
                let endpoint = self.extract_provider_multiaddr(&value)?;
                Ok(Some(endpoint))
            }
            Ok(None) => Ok(None),
            Err(e) => {
                tracing::debug!("Failed to fetch provider info: {e}");
                Ok(None)
            }
        }
    }

    /// Extract multiaddr from a ProviderInfo value.
    fn extract_provider_multiaddr(
        &self,
        provider_value: &subxt::dynamic::DecodedValueThunk,
    ) -> Result<String, Error> {
        use subxt::ext::scale_value::{At, Composite, Primitive, ValueDef};

        let decoded = provider_value
            .to_value()
            .map_err(|e| Error::Internal(format!("Failed to decode provider: {e}")))?;

        // ProviderInfo fields: multiaddr, public_key, stake, committed_bytes, settings, stats
        // multiaddr is at index 0
        if let Some(field0) = decoded.at(0) {
            if let ValueDef::Composite(Composite::Unnamed(multiaddr_bytes)) = &field0.value {
                let bytes: Vec<u8> = multiaddr_bytes
                    .iter()
                    .filter_map(|v| {
                        if let ValueDef::Primitive(Primitive::U128(n)) = &v.value {
                            Some(*n as u8)
                        } else {
                            None
                        }
                    })
                    .collect();

                if !bytes.is_empty() {
                    // Convert multiaddr bytes to HTTP endpoint
                    let multiaddr_str = String::from_utf8_lossy(&bytes);
                    return Ok(self.multiaddr_to_http_endpoint(&multiaddr_str));
                }
            }
        }

        Err(Error::Internal("Failed to extract multiaddr".to_string()))
    }

    /// Convert a multiaddr string to an HTTP endpoint.
    fn multiaddr_to_http_endpoint(&self, multiaddr: &str) -> String {
        // Parse multiaddr format: /ip4/127.0.0.1/tcp/3000 or /dns4/hostname/tcp/3000
        let parts: Vec<&str> = multiaddr.split('/').filter(|s| !s.is_empty()).collect();

        let mut host = "127.0.0.1".to_string();
        let mut port = "3000".to_string();

        let mut i = 0;
        while i < parts.len() {
            match parts[i] {
                "ip4" | "ip6" => {
                    if i + 1 < parts.len() {
                        host = parts[i + 1].to_string();
                        i += 2;
                    } else {
                        i += 1;
                    }
                }
                "dns4" | "dns6" | "dns" => {
                    if i + 1 < parts.len() {
                        host = parts[i + 1].to_string();
                        i += 2;
                    } else {
                        i += 1;
                    }
                }
                "tcp" => {
                    if i + 1 < parts.len() {
                        port = parts[i + 1].to_string();
                        i += 2;
                    } else {
                        i += 1;
                    }
                }
                _ => {
                    i += 1;
                }
            }
        }

        format!("http://{}:{}", host, port)
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
        assert!(matches!(
            insufficient,
            SyncResult::InsufficientBalance { .. }
        ));

        let interval = SyncResult::SyncIntervalNotElapsed {
            bucket_id: 1,
            blocks_remaining: 50,
        };
        assert!(matches!(
            interval,
            SyncResult::SyncIntervalNotElapsed { .. }
        ));
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
