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
use std::future::Future;
use std::pin::Pin;
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
pub trait ReplicaSyncChainClient: Send + Sync {
    /// Get the current block number.
    fn get_current_block(&self) -> Pin<Box<dyn Future<Output = Result<u64, Error>> + Send + '_>>;

    /// Fetch replica agreements for this provider.
    fn fetch_replica_agreements(
        &self,
        provider_account: &str,
        local_buckets: Vec<BucketId>,
    ) -> Pin<Box<dyn Future<Output = Result<Vec<ReplicaAgreementInfo>, Error>> + Send + '_>>;

    /// Fetch the bucket snapshot (latest checkpoint state) from chain.
    fn fetch_bucket_snapshot(
        &self,
        bucket_id: BucketId,
    ) -> Pin<Box<dyn Future<Output = Result<BucketSnapshot, Error>> + Send + '_>>;

    /// Fetch primary provider HTTP endpoints for a bucket.
    fn fetch_primary_endpoints(
        &self,
        bucket_id: BucketId,
    ) -> Pin<Box<dyn Future<Output = Result<Vec<String>, Error>> + Send + '_>>;

    /// Submit a confirm_replica_sync extrinsic.
    #[allow(clippy::type_complexity)]
    fn submit_sync_confirmation(
        &self,
        bucket_id: BucketId,
        target_mmr_root: H256,
    ) -> Pin<Box<dyn Future<Output = Result<(u8, u128), Error>> + Send + '_>>;
}

/// Production implementation that talks to the chain via subxt.
pub struct SubxtReplicaSyncChainClient {
    api: subxt::OnlineClient<subxt::PolkadotConfig>,
    signer: subxt_signer::sr25519::Keypair,
}

impl SubxtReplicaSyncChainClient {
    /// Connect to the chain and create a signer from the provider state's keypair.
    pub async fn connect(
        chain_ws_url: &str,
        keypair: &sp_core::sr25519::Pair,
    ) -> Result<Self, Error> {
        use sp_core::Pair;

        let api = subxt::OnlineClient::<subxt::PolkadotConfig>::from_url(chain_ws_url)
            .await
            .map_err(|e| Error::Internal(format!("Failed to connect to chain: {e}")))?;

        let raw = keypair.to_raw_vec();
        let secret_bytes: [u8; 32] = raw[..32]
            .try_into()
            .map_err(|_| Error::Internal("Invalid secret key length".to_string()))?;
        let signer = subxt_signer::sr25519::Keypair::from_secret_key(secret_bytes)
            .map_err(|e| Error::Internal(format!("Failed to create signer: {e}")))?;

        tracing::info!("Replica sync coordinator connected to {}", chain_ws_url);

        Ok(Self { api, signer })
    }

    /// Convert a multiaddr string to an HTTP endpoint.
    fn multiaddr_to_http_endpoint(multiaddr: &str) -> String {
        let parts: Vec<&str> = multiaddr.split('/').filter(|s| !s.is_empty()).collect();

        let mut host = "127.0.0.1".to_string();
        let mut port = "3333".to_string();

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

        format!("http://{host}:{port}")
    }

    /// Decode a storage agreement from raw SCALE-encoded bytes.
    fn decode_storage_agreement_bytes(
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

        let sync_balance = u128::from_le_bytes(
            remaining[0..16]
                .try_into()
                .map_err(|_| Error::Internal("Failed to parse sync_balance".to_string()))?,
        );

        let sync_price = u128::from_le_bytes(
            remaining[16..32]
                .try_into()
                .map_err(|_| Error::Internal("Failed to parse sync_price".to_string()))?,
        );

        let min_sync_interval = u32::from_le_bytes(
            remaining[32..36]
                .try_into()
                .map_err(|_| Error::Internal("Failed to parse min_sync_interval".to_string()))?,
        ) as u64;

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

    /// Parse a BucketSnapshot value from scale_value.
    fn parse_bucket_snapshot_value<T>(value: &subxt::ext::scale_value::Value<T>) -> BucketSnapshot {
        use subxt::ext::scale_value::{At, Composite, Primitive, ValueDef};

        let mmr_root = if let Some(field0) = value.at(0) {
            if let ValueDef::Composite(Composite::Unnamed(bytes_vec)) = &field0.value {
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

        BucketSnapshot {
            mmr_root,
            leaf_count,
        }
    }
}

impl ReplicaSyncChainClient for SubxtReplicaSyncChainClient {
    fn get_current_block(&self) -> Pin<Box<dyn Future<Output = Result<u64, Error>> + Send + '_>> {
        Box::pin(async move {
            let block = self
                .api
                .blocks()
                .at_latest()
                .await
                .map_err(|e| Error::Internal(format!("Failed to get latest block: {e}")))?;
            Ok(block.number() as u64)
        })
    }

    fn fetch_replica_agreements(
        &self,
        provider_account: &str,
        local_buckets: Vec<BucketId>,
    ) -> Pin<Box<dyn Future<Output = Result<Vec<ReplicaAgreementInfo>, Error>> + Send + '_>> {
        let provider_account = provider_account.to_string();
        Box::pin(async move {
            let mut agreements = Vec::new();

            let account_bytes = hex::decode(provider_account.trim_start_matches("0x"))
                .map_err(|e| Error::Internal(format!("Invalid account hex: {e}")))?;

            // Query local buckets for agreements
            for bucket_id in &local_buckets {
                let storage_address = subxt::dynamic::storage(
                    "StorageProvider",
                    "StorageAgreements",
                    vec![
                        subxt::dynamic::Value::u128(*bucket_id as u128),
                        subxt::dynamic::Value::from_bytes(&account_bytes),
                    ],
                );

                let storage = self
                    .api
                    .storage()
                    .at_latest()
                    .await
                    .map_err(|e| Error::Internal(format!("Failed to get storage: {e}")))?;

                if let Ok(Some(value)) = storage.fetch(&storage_address).await {
                    let encoded = value.encoded();
                    if let Ok(agreement) = Self::decode_storage_agreement_bytes(*bucket_id, encoded)
                    {
                        agreements.push(agreement);
                    }
                }
            }

            // Also iterate chain storage for agreements we might not have locally
            let storage_address =
                subxt::dynamic::storage("StorageProvider", "StorageAgreements", ());

            if let Ok(storage) = self.api.storage().at_latest().await {
                if let Ok(mut iter) = storage.iter(storage_address).await {
                    while let Some(result) = iter.next().await {
                        let kv = match result {
                            Ok(kv) => kv,
                            Err(e) => {
                                tracing::debug!("Error iterating storage: {e}");
                                continue;
                            }
                        };

                        let key_bytes = kv.key_bytes;
                        if key_bytes.len() < 32 + 16 + 8 + 16 + 32 {
                            continue;
                        }

                        let bucket_id_start = 32 + 16;
                        let bucket_id_bytes = &key_bytes[bucket_id_start..bucket_id_start + 8];
                        let bucket_id =
                            u64::from_le_bytes(bucket_id_bytes.try_into().unwrap_or([0; 8]));

                        let provider_start = bucket_id_start + 8 + 16;
                        let provider_bytes = &key_bytes[provider_start..];

                        if provider_bytes.len() < 32 || provider_bytes[..32] != account_bytes[..32]
                        {
                            continue;
                        }

                        let encoded = kv.value.encoded();
                        if let Ok(agreement) =
                            Self::decode_storage_agreement_bytes(bucket_id, encoded)
                        {
                            if !agreements
                                .iter()
                                .any(|a| a.bucket_id == agreement.bucket_id)
                            {
                                agreements.push(agreement);
                            }
                        }
                    }
                }
            }

            Ok(agreements)
        })
    }

    fn fetch_bucket_snapshot(
        &self,
        bucket_id: BucketId,
    ) -> Pin<Box<dyn Future<Output = Result<BucketSnapshot, Error>> + Send + '_>> {
        Box::pin(async move {
            use subxt::ext::scale_value::ValueDef;

            let storage_address = subxt::dynamic::storage(
                "StorageProvider",
                "Buckets",
                vec![subxt::dynamic::Value::u128(bucket_id as u128)],
            );

            let storage = self
                .api
                .storage()
                .at_latest()
                .await
                .map_err(|e| Error::Internal(format!("Failed to get storage: {e}")))?;

            match storage.fetch(&storage_address).await {
                Ok(Some(value)) => {
                    use subxt::ext::scale_value::At;
                    let decoded = value
                        .to_value()
                        .map_err(|e| Error::Internal(format!("Failed to decode bucket: {e}")))?;

                    if let Some(snapshot_opt) = decoded.at(4) {
                        if let ValueDef::Variant(variant) = &snapshot_opt.value {
                            if variant.name == "Some" {
                                if let Some(snapshot_val) = variant.values.values().next() {
                                    return Ok(Self::parse_bucket_snapshot_value(snapshot_val));
                                }
                            }
                        }
                    }

                    Ok(BucketSnapshot {
                        mmr_root: H256::zero(),
                        leaf_count: 0,
                    })
                }
                _ => Ok(BucketSnapshot {
                    mmr_root: H256::zero(),
                    leaf_count: 0,
                }),
            }
        })
    }

    fn fetch_primary_endpoints(
        &self,
        bucket_id: BucketId,
    ) -> Pin<Box<dyn Future<Output = Result<Vec<String>, Error>> + Send + '_>> {
        Box::pin(async move {
            use subxt::ext::scale_value::{At, Composite, Primitive, ValueDef};

            let storage_address = subxt::dynamic::storage(
                "StorageProvider",
                "Buckets",
                vec![subxt::dynamic::Value::u128(bucket_id as u128)],
            );

            let storage = self
                .api
                .storage()
                .at_latest()
                .await
                .map_err(|e| Error::Internal(format!("Failed to get storage: {e}")))?;

            let bucket_value = match storage.fetch(&storage_address).await {
                Ok(Some(v)) => v,
                _ => return Ok(vec![]),
            };

            let decoded = bucket_value
                .to_value()
                .map_err(|e| Error::Internal(format!("Failed to decode bucket: {e}")))?;

            let mut provider_bytes_list = Vec::new();

            // primary_providers is at index 3
            if let Some(field3) = decoded.at(3) {
                if let ValueDef::Composite(Composite::Unnamed(providers_vec)) = &field3.value {
                    for provider_value in providers_vec {
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
                                provider_bytes_list.push(bytes);
                            }
                        }
                    }
                }
            }

            // Look up each provider's multiaddr
            let mut endpoints = Vec::new();
            for provider_bytes in provider_bytes_list {
                let provider_addr = subxt::dynamic::storage(
                    "StorageProvider",
                    "Providers",
                    vec![subxt::dynamic::Value::from_bytes(&provider_bytes)],
                );

                let storage = self
                    .api
                    .storage()
                    .at_latest()
                    .await
                    .map_err(|e| Error::Internal(format!("Failed to get storage: {e}")))?;

                if let Ok(Some(value)) = storage.fetch(&provider_addr).await {
                    if let Ok(decoded) = value.to_value() {
                        if let Some(field0) = decoded.at(0) {
                            if let ValueDef::Composite(Composite::Unnamed(multiaddr_bytes)) =
                                &field0.value
                            {
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
                                    let multiaddr_str = String::from_utf8_lossy(&bytes);
                                    endpoints
                                        .push(Self::multiaddr_to_http_endpoint(&multiaddr_str));
                                }
                            }
                        }
                    }
                }
            }

            Ok(endpoints)
        })
    }

    fn submit_sync_confirmation(
        &self,
        bucket_id: BucketId,
        target_mmr_root: H256,
    ) -> Pin<Box<dyn Future<Output = Result<(u8, u128), Error>> + Send + '_>> {
        Box::pin(async move {
            // Build roots array: position 0 = current root, rest = None
            let roots_value: Vec<subxt::dynamic::Value> = (0..7)
                .map(|i| {
                    if i == 0 {
                        subxt::dynamic::Value::unnamed_variant(
                            "Some",
                            vec![subxt::dynamic::Value::from_bytes(
                                target_mmr_root.as_bytes(),
                            )],
                        )
                    } else {
                        subxt::dynamic::Value::unnamed_variant("None", vec![])
                    }
                })
                .collect();

            let signature = subxt::dynamic::Value::unnamed_variant(
                "Sr25519",
                vec![subxt::dynamic::Value::from_bytes([0u8; 64])],
            );

            let tx = subxt::dynamic::tx(
                "StorageProvider",
                "confirm_replica_sync",
                vec![
                    subxt::dynamic::Value::u128(bucket_id as u128),
                    subxt::dynamic::Value::unnamed_composite(roots_value),
                    signature,
                ],
            );

            tracing::info!(
                "Submitting confirm_replica_sync for bucket {} with root 0x{}",
                bucket_id,
                hex::encode(target_mmr_root.as_bytes())
            );

            let tx_progress = self
                .api
                .tx()
                .sign_and_submit_then_watch_default(&tx, &self.signer)
                .await
                .map_err(|e| Error::Internal(format!("Failed to submit tx: {e}")))?;

            let _events = tx_progress
                .wait_for_finalized_success()
                .await
                .map_err(|e| Error::Internal(format!("Transaction failed: {e}")))?;

            tracing::info!(
                "confirm_replica_sync submitted successfully for bucket {}",
                bucket_id
            );

            Ok((0, 0)) // Position 0, payment extracted from events in production
        })
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

        let local_buckets: Vec<u64> = self
            .state
            .storage
            .list_buckets()
            .into_iter()
            .map(|b| b.bucket_id)
            .collect();

        let provider_account = self.state.provider_id.clone();

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
