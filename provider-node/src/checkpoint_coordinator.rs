//! Checkpoint Coordinator - Provider-initiated checkpoint coordination.
//!
//! This module provides a background service that coordinates with other
//! providers to autonomously submit checkpoints without requiring the
//! client to be online.

use crate::{Error, ProviderState};
use codec::Encode;
use sp_core::{crypto::Ss58Codec, Pair, H256};
use std::future::Future;
use std::pin::Pin;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;
use storage_primitives::{BucketId, CheckpointProposal};
use tokio::sync::mpsc;

/// Configuration for the checkpoint coordinator.
#[derive(Clone, Debug)]
pub struct CheckpointCoordinatorConfig {
    /// How often to poll for checkpoint duties.
    pub poll_interval: Duration,
    /// Timeout for collecting signatures from peers.
    pub signature_timeout: Duration,
    /// Whether to automatically submit checkpoints when leader.
    pub auto_submit: bool,
}

impl Default for CheckpointCoordinatorConfig {
    fn default() -> Self {
        Self {
            poll_interval: Duration::from_secs(6), // ~1 block
            signature_timeout: Duration::from_secs(30),
            auto_submit: true,
        }
    }
}

/// Information about a checkpoint duty.
#[derive(Clone, Debug)]
pub struct CheckpointDuty {
    /// Bucket needing a checkpoint.
    pub bucket_id: BucketId,
    /// Current checkpoint window number.
    pub window: u64,
    /// Current MMR root for the bucket.
    pub mmr_root: H256,
    /// Start sequence number.
    pub start_seq: u64,
    /// Number of leaves in the MMR.
    pub leaf_count: u64,
    /// Whether this provider is the leader for this window.
    pub is_leader: bool,
    /// List of peer provider endpoints.
    pub peer_endpoints: Vec<String>,
    /// Interval in blocks.
    pub interval: u32,
    /// Grace period in blocks.
    pub grace_period: u32,
}

/// Result of a checkpoint coordination attempt.
#[derive(Clone, Debug)]
pub enum CheckpointResult {
    /// Successfully submitted checkpoint.
    Success {
        bucket_id: BucketId,
        window: u64,
        mmr_root: H256,
        signers: Vec<String>,
    },
    /// Not enough signatures collected.
    InsufficientSignatures {
        bucket_id: BucketId,
        window: u64,
        collected: usize,
        required: usize,
    },
    /// Failed to submit checkpoint transaction.
    SubmissionFailed {
        bucket_id: BucketId,
        window: u64,
        error: String,
    },
    /// Not the leader and within grace period.
    NotLeader { bucket_id: BucketId, window: u64 },
    /// Checkpoint already submitted for this window.
    AlreadySubmitted { bucket_id: BucketId, window: u64 },
}

/// Trait abstracting chain interactions for the checkpoint coordinator.
#[allow(clippy::type_complexity)]
pub trait CheckpointChainClient: Send + Sync {
    /// Get the current block number.
    fn get_current_block(&self) -> Pin<Box<dyn Future<Output = Result<u64, Error>> + Send + '_>>;

    /// Fetch checkpoint config (interval, grace_period) for a bucket.
    /// Returns `None` if no config exists on chain.
    fn fetch_checkpoint_config(
        &self,
        bucket_id: BucketId,
    ) -> Pin<Box<dyn Future<Output = Result<Option<(u32, u32)>, Error>> + Send + '_>>;

    /// Submit a checkpoint transaction with collected signatures.
    fn submit_checkpoint(
        &self,
        duty: &CheckpointDuty,
        signatures: Vec<(String, String)>,
    ) -> Pin<Box<dyn Future<Output = Result<H256, Error>> + Send + '_>>;
}

/// Production implementation that talks to the chain via subxt.
pub struct SubxtCheckpointChainClient {
    api: subxt::OnlineClient<subxt::PolkadotConfig>,
    signer: subxt_signer::sr25519::Keypair,
}

impl SubxtCheckpointChainClient {
    /// Connect to the chain and create a signer from the seed URI.
    pub async fn connect(chain_ws_url: &str, seed: &str) -> Result<Self, Error> {
        let api = subxt::OnlineClient::<subxt::PolkadotConfig>::from_url(chain_ws_url)
            .await
            .map_err(|e| Error::Internal(format!("Failed to connect to chain: {e}")))?;

        let uri: subxt_signer::SecretUri = seed
            .parse()
            .map_err(|e| Error::Internal(format!("Invalid seed URI: {e}")))?;
        let signer = subxt_signer::sr25519::Keypair::from_uri(&uri)
            .map_err(|e| Error::Internal(format!("Failed to create signer: {e}")))?;

        tracing::info!(
            "Checkpoint coordinator signer: {}",
            sp_core::crypto::AccountId32::from(signer.public_key().0).to_ss58check()
        );
        tracing::info!("Checkpoint coordinator connected to {}", chain_ws_url);

        Ok(Self { api, signer })
    }
}

impl CheckpointChainClient for SubxtCheckpointChainClient {
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

    fn fetch_checkpoint_config(
        &self,
        bucket_id: BucketId,
    ) -> Pin<Box<dyn Future<Output = Result<Option<(u32, u32)>, Error>> + Send + '_>> {
        Box::pin(async move {
            use subxt::dynamic::At;

            let config_query = subxt::dynamic::storage(
                "StorageProvider",
                "CheckpointConfigs",
                vec![subxt::dynamic::Value::u128(bucket_id as u128)],
            );
            let storage = self
                .api
                .storage()
                .at_latest()
                .await
                .map_err(|e| Error::Internal(format!("Failed to get storage: {e}")))?;

            match storage
                .fetch(&config_query)
                .await
                .map_err(|e| Error::Internal(format!("Failed to fetch config: {e}")))?
            {
                Some(val) => {
                    let decoded = val
                        .to_value()
                        .map_err(|e| Error::Internal(format!("Failed to decode config: {e}")))?;
                    let interval = decoded
                        .at("interval")
                        .and_then(|v| v.as_u128())
                        .unwrap_or(100) as u32;
                    let grace_period = decoded
                        .at("grace_period")
                        .and_then(|v| v.as_u128())
                        .unwrap_or(20) as u32;
                    Ok(Some((interval, grace_period)))
                }
                None => Ok(None),
            }
        })
    }

    fn submit_checkpoint(
        &self,
        duty: &CheckpointDuty,
        signatures: Vec<(String, String)>,
    ) -> Pin<Box<dyn Future<Output = Result<H256, Error>> + Send + '_>> {
        let bucket_id = duty.bucket_id;
        let mmr_root = duty.mmr_root;
        let start_seq = duty.start_seq;
        let leaf_count = duty.leaf_count;
        let window = duty.window;

        Box::pin(async move {
            // Build signature tuples for the extrinsic
            let mut sig_values = Vec::with_capacity(signatures.len());
            for (account, sig) in &signatures {
                let account_id: sp_core::crypto::AccountId32 =
                    sp_core::crypto::Ss58Codec::from_ss58check(account).map_err(|e| {
                        Error::Internal(format!("Invalid SS58 account '{account}': {e:?}"))
                    })?;
                let account_bytes: [u8; 32] = account_id.into();

                let sig_bytes = hex::decode(sig.trim_start_matches("0x"))
                    .map_err(|e| Error::Internal(format!("Invalid signature hex: {e}")))?;

                sig_values.push(subxt::dynamic::Value::unnamed_composite(vec![
                    subxt::dynamic::Value::from_bytes(account_bytes),
                    subxt::dynamic::Value::unnamed_variant(
                        "Sr25519",
                        vec![subxt::dynamic::Value::from_bytes(sig_bytes)],
                    ),
                ]));
            }

            let tx = subxt::dynamic::tx(
                "StorageProvider",
                "provider_checkpoint",
                vec![
                    subxt::dynamic::Value::u128(bucket_id as u128),
                    subxt::dynamic::Value::from_bytes(mmr_root.as_bytes()),
                    subxt::dynamic::Value::u128(start_seq as u128),
                    subxt::dynamic::Value::u128(leaf_count as u128),
                    subxt::dynamic::Value::u128(window as u128),
                    subxt::dynamic::Value::unnamed_composite(sig_values),
                ],
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

            Ok(H256::zero())
        })
    }
}

/// Commands for controlling the coordinator.
#[derive(Debug)]
pub enum CoordinatorCommand {
    /// Stop the coordinator.
    Stop,
    /// Pause automatic checkpoints.
    Pause,
    /// Resume automatic checkpoints.
    Resume,
    /// Force checkpoint for a specific bucket.
    ForceCheckpoint(BucketId),
}

/// Handle for controlling the checkpoint coordinator.
pub struct CheckpointCoordinatorHandle {
    command_tx: mpsc::Sender<CoordinatorCommand>,
    running: Arc<AtomicBool>,
}

impl CheckpointCoordinatorHandle {
    /// Check if the coordinator is running.
    pub fn is_running(&self) -> bool {
        self.running.load(Ordering::SeqCst)
    }

    /// Stop the coordinator.
    pub async fn stop(&self) -> Result<(), Error> {
        self.command_tx
            .send(CoordinatorCommand::Stop)
            .await
            .map_err(|_| Error::Internal("Coordinator channel closed".to_string()))
    }

    /// Pause automatic checkpoints.
    pub async fn pause(&self) -> Result<(), Error> {
        self.command_tx
            .send(CoordinatorCommand::Pause)
            .await
            .map_err(|_| Error::Internal("Coordinator channel closed".to_string()))
    }

    /// Resume automatic checkpoints.
    pub async fn resume(&self) -> Result<(), Error> {
        self.command_tx
            .send(CoordinatorCommand::Resume)
            .await
            .map_err(|_| Error::Internal("Coordinator channel closed".to_string()))
    }

    /// Force a checkpoint submission for a specific bucket.
    pub async fn force_checkpoint(&self, bucket_id: BucketId) -> Result<(), Error> {
        self.command_tx
            .send(CoordinatorCommand::ForceCheckpoint(bucket_id))
            .await
            .map_err(|_| Error::Internal("Coordinator channel closed".to_string()))
    }

    /// Get a clone of the command sender (for sharing with the HTTP API).
    pub fn command_sender(&self) -> mpsc::Sender<CoordinatorCommand> {
        self.command_tx.clone()
    }
}

/// Checkpoint coordinator service.
pub struct CheckpointCoordinator {
    config: CheckpointCoordinatorConfig,
    state: Arc<ProviderState>,
    chain_client: Box<dyn CheckpointChainClient>,
    http_client: reqwest::Client,
}

impl CheckpointCoordinator {
    /// Create a new checkpoint coordinator.
    pub fn new(
        config: CheckpointCoordinatorConfig,
        state: Arc<ProviderState>,
        chain_client: Box<dyn CheckpointChainClient>,
    ) -> Self {
        Self {
            config,
            state,
            chain_client,
            http_client: reqwest::Client::builder()
                .timeout(Duration::from_secs(10))
                .build()
                .expect("Failed to create HTTP client"),
        }
    }

    /// Start the checkpoint coordinator background service.
    pub async fn start(
        self,
        callback: Option<Arc<dyn Fn(CheckpointResult) + Send + Sync>>,
    ) -> Result<CheckpointCoordinatorHandle, Error> {
        let (command_tx, command_rx) = mpsc::channel::<CoordinatorCommand>(32);
        let running = Arc::new(AtomicBool::new(true));
        let running_clone = running.clone();

        let running_exit = running.clone();
        tokio::spawn(async move {
            self.run_loop(command_rx, running_clone, callback).await;
            tracing::error!("Checkpoint coordinator run_loop exited unexpectedly!");
            running_exit.store(false, Ordering::SeqCst);
        });

        Ok(CheckpointCoordinatorHandle {
            command_tx,
            running,
        })
    }

    /// Main coordinator loop.
    async fn run_loop(
        self,
        mut command_rx: mpsc::Receiver<CoordinatorCommand>,
        running: Arc<AtomicBool>,
        callback: Option<Arc<dyn Fn(CheckpointResult) + Send + Sync>>,
    ) {
        let mut paused = false;
        let mut interval = tokio::time::interval(self.config.poll_interval);

        tracing::info!("Checkpoint coordinator started");

        loop {
            tokio::select! {
                cmd = command_rx.recv() => {
                    match cmd {
                        Some(CoordinatorCommand::Stop) | None => {
                            tracing::info!("Checkpoint coordinator stopping");
                            running.store(false, Ordering::SeqCst);
                            break;
                        }
                        Some(CoordinatorCommand::Pause) => {
                            tracing::info!("Checkpoint coordinator paused");
                            paused = true;
                        }
                        Some(CoordinatorCommand::Resume) => {
                            tracing::info!("Checkpoint coordinator resumed");
                            paused = false;
                        }
                        Some(CoordinatorCommand::ForceCheckpoint(bucket_id)) => {
                            tracing::info!("Force checkpoint requested for bucket {}", bucket_id);
                            match self.get_checkpoint_duty(bucket_id).await {
                                Ok(Some(duty)) => {
                                    let result = self.coordinate_checkpoint(&duty).await;
                                    tracing::info!("Force checkpoint result: {:?}", result);
                                    if let Some(ref cb) = callback {
                                        cb(result);
                                    }
                                }
                                Ok(None) => {
                                    tracing::warn!("No checkpoint duty found for bucket {}", bucket_id);
                                }
                                Err(e) => {
                                    tracing::error!("Failed to get checkpoint duty for bucket {}: {}", bucket_id, e);
                                }
                            }
                        }
                    }
                }
                _ = interval.tick() => {
                    if paused || !self.config.auto_submit {
                        continue;
                    }

                    // Get active checkpoint duties
                    match self.get_active_checkpoint_duties().await {
                        Ok(duties) => {
                            for duty in duties {
                                if duty.is_leader {
                                    tracing::info!(
                                        "Leader for checkpoint: bucket {} window {}",
                                        duty.bucket_id,
                                        duty.window
                                    );

                                    let result = self.coordinate_checkpoint(&duty).await;
                                    if let Some(ref cb) = callback {
                                        cb(result);
                                    }
                                }
                            }
                        }
                        Err(e) => {
                            tracing::warn!("Failed to get checkpoint duties: {}", e);
                        }
                    }
                }
            }
        }
    }

    /// Get checkpoint duties for buckets where this provider is involved.
    async fn get_active_checkpoint_duties(&self) -> Result<Vec<CheckpointDuty>, Error> {
        // TODO: Query chain for buckets where this provider is a primary provider
        // and where provider-initiated checkpoints are enabled.
        // For now, return empty - duties would be derived from on-chain state.
        Ok(vec![])
    }

    /// Get checkpoint duty for a specific bucket.
    async fn get_checkpoint_duty(
        &self,
        bucket_id: BucketId,
    ) -> Result<Option<CheckpointDuty>, Error> {
        // Get bucket data from local storage
        let bucket = match self.state.storage.get_bucket(bucket_id) {
            Some(b) => b,
            None => {
                tracing::warn!("Bucket {} not found in local storage", bucket_id);
                return Ok(None);
            }
        };

        if bucket.leaf_count == 0 {
            tracing::warn!("Bucket {} has no data (leaf_count=0)", bucket_id);
            return Ok(None);
        }

        let current_block = self.chain_client.get_current_block().await?;

        let (interval, grace_period) = self
            .chain_client
            .fetch_checkpoint_config(bucket_id)
            .await?
            .unwrap_or((100u32, 20u32));

        let window = if interval > 0 {
            current_block / interval as u64
        } else {
            0
        };

        tracing::info!(
            "Checkpoint duty: bucket={} block={} interval={} window={} mmr_root=0x{} leaves={}",
            bucket_id,
            current_block,
            interval,
            window,
            hex::encode(&bucket.mmr_root.as_bytes()[..4]),
            bucket.leaf_count
        );

        let duty = CheckpointDuty {
            bucket_id,
            window,
            mmr_root: bucket.mmr_root,
            start_seq: bucket.start_seq,
            leaf_count: bucket.leaf_count,
            is_leader: true, // Force checkpoint bypasses leader check
            peer_endpoints: vec![],
            interval,
            grace_period,
        };

        Ok(Some(duty))
    }

    /// Coordinate a checkpoint: collect signatures and submit.
    async fn coordinate_checkpoint(&self, duty: &CheckpointDuty) -> CheckpointResult {
        tracing::info!(
            "Coordinating checkpoint for bucket {} window {}",
            duty.bucket_id,
            duty.window
        );

        // Step 1: Create the checkpoint proposal
        let proposal = CheckpointProposal::new(
            duty.bucket_id,
            duty.mmr_root,
            duty.start_seq,
            duty.leaf_count,
            duty.window,
        );

        // Step 2: Sign the proposal ourselves
        let our_signature = match self.sign_proposal(&proposal) {
            Some(sig) => sig,
            None => {
                return CheckpointResult::SubmissionFailed {
                    bucket_id: duty.bucket_id,
                    window: duty.window,
                    error: "No signer configured".to_string(),
                };
            }
        };

        // Step 3: Collect signatures from peers
        let mut signatures = vec![(self.state.provider_id.clone(), our_signature)];

        for endpoint in &duty.peer_endpoints {
            match self.request_signature(endpoint, &proposal).await {
                Ok(response) => {
                    if response.agreed {
                        signatures.push((response.signer, response.signature));
                    } else {
                        tracing::warn!(
                            "Peer {} disagreed with proposal (their root: {:?})",
                            endpoint,
                            response.local_mmr_root
                        );
                    }
                }
                Err(e) => {
                    tracing::warn!("Failed to get signature from {}: {}", endpoint, e);
                }
            }
        }

        // Step 4: Check if we have enough signatures
        let min_required = 1; // Would get from chain (bucket.min_providers)
        if signatures.len() < min_required {
            return CheckpointResult::InsufficientSignatures {
                bucket_id: duty.bucket_id,
                window: duty.window,
                collected: signatures.len(),
                required: min_required,
            };
        }

        // Step 5: Submit the checkpoint
        let signers: Vec<String> = signatures.iter().map(|(s, _)| s.clone()).collect();
        match self.chain_client.submit_checkpoint(duty, signatures).await {
            Ok(_) => CheckpointResult::Success {
                bucket_id: duty.bucket_id,
                window: duty.window,
                mmr_root: duty.mmr_root,
                signers,
            },
            Err(e) => CheckpointResult::SubmissionFailed {
                bucket_id: duty.bucket_id,
                window: duty.window,
                error: e.to_string(),
            },
        }
    }

    /// Sign a checkpoint proposal.
    fn sign_proposal(&self, proposal: &CheckpointProposal) -> Option<String> {
        let keypair = self.state.keypair.as_ref()?;
        let encoded = proposal.encode();
        let signature = keypair.sign(&encoded);
        Some(format!("0x{}", hex::encode(signature.0)))
    }

    /// Request a signature from a peer provider.
    async fn request_signature(
        &self,
        endpoint: &str,
        proposal: &CheckpointProposal,
    ) -> Result<SignProposalResponse, Error> {
        let url = format!("{endpoint}/checkpoint/sign");

        let request = SignProposalRequest {
            bucket_id: proposal.bucket_id,
            mmr_root: format!("0x{}", hex::encode(proposal.mmr_root.as_bytes())),
            start_seq: proposal.start_seq,
            leaf_count: proposal.leaf_count,
            window: proposal.window,
        };

        let response = self
            .http_client
            .post(&url)
            .json(&request)
            .timeout(self.config.signature_timeout)
            .send()
            .await
            .map_err(|e| Error::Internal(format!("HTTP request failed: {e}")))?;

        if !response.status().is_success() {
            return Err(Error::Internal(format!(
                "Peer returned error: {}",
                response.status()
            )));
        }

        response
            .json::<SignProposalResponse>()
            .await
            .map_err(|e| Error::Internal(format!("Failed to parse response: {e}")))
    }
}

/// Request to sign a checkpoint proposal.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct SignProposalRequest {
    pub bucket_id: BucketId,
    pub mmr_root: String,
    pub start_seq: u64,
    pub leaf_count: u64,
    pub window: u64,
}

/// Response from signing a checkpoint proposal.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct SignProposalResponse {
    /// Signer's account ID.
    pub signer: String,
    /// Signature over the proposal (if agreed).
    pub signature: String,
    /// Whether the signer agreed with the proposal.
    pub agreed: bool,
    /// Signer's local MMR root (for debugging disagreements).
    pub local_mmr_root: Option<String>,
}

/// Query for checkpoint duty status.
#[derive(Debug, Clone, serde::Deserialize)]
pub struct CheckpointDutyQuery {
    pub bucket_id: BucketId,
}

/// Response with checkpoint duty information.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct CheckpointDutyResponse {
    pub bucket_id: BucketId,
    pub mmr_root: String,
    pub start_seq: u64,
    pub leaf_count: u64,
    pub ready: bool,
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::sync::Mutex;

    struct MockCheckpointChainClient {
        block_number: Mutex<u64>,
        config: Mutex<Option<(u32, u32)>>,
        submitted: Mutex<Vec<(BucketId, u64)>>,
        submit_result: Mutex<Result<H256, Error>>,
    }

    impl MockCheckpointChainClient {
        fn new(block: u64) -> Self {
            Self {
                block_number: Mutex::new(block),
                config: Mutex::new(Some((100, 20))),
                submitted: Mutex::new(Vec::new()),
                submit_result: Mutex::new(Ok(H256::zero())),
            }
        }

        fn with_submit_error(self, err: Error) -> Self {
            Self {
                submit_result: Mutex::new(Err(err)),
                ..self
            }
        }
    }

    impl CheckpointChainClient for MockCheckpointChainClient {
        fn get_current_block(
            &self,
        ) -> Pin<Box<dyn Future<Output = Result<u64, Error>> + Send + '_>> {
            Box::pin(async { Ok(*self.block_number.lock().await) })
        }

        fn fetch_checkpoint_config(
            &self,
            _bucket_id: BucketId,
        ) -> Pin<Box<dyn Future<Output = Result<Option<(u32, u32)>, Error>> + Send + '_>> {
            Box::pin(async { Ok(*self.config.lock().await) })
        }

        fn submit_checkpoint(
            &self,
            duty: &CheckpointDuty,
            _signatures: Vec<(String, String)>,
        ) -> Pin<Box<dyn Future<Output = Result<H256, Error>> + Send + '_>> {
            let bucket_id = duty.bucket_id;
            let window = duty.window;
            Box::pin(async move {
                self.submitted.lock().await.push((bucket_id, window));
                let mut result = self.submit_result.lock().await;
                // Clone the result so we can return it and preserve the original
                match &*result {
                    Ok(h) => Ok(*h),
                    Err(e) => {
                        let err = Error::Internal(e.to_string());
                        // Reset to Ok so subsequent calls don't fail
                        *result = Ok(H256::zero());
                        Err(err)
                    }
                }
            })
        }
    }

    impl CheckpointChainClient for Arc<MockCheckpointChainClient> {
        fn get_current_block(
            &self,
        ) -> Pin<Box<dyn Future<Output = Result<u64, Error>> + Send + '_>> {
            (**self).get_current_block()
        }

        fn fetch_checkpoint_config(
            &self,
            bucket_id: BucketId,
        ) -> Pin<Box<dyn Future<Output = Result<Option<(u32, u32)>, Error>> + Send + '_>> {
            (**self).fetch_checkpoint_config(bucket_id)
        }

        fn submit_checkpoint(
            &self,
            duty: &CheckpointDuty,
            signatures: Vec<(String, String)>,
        ) -> Pin<Box<dyn Future<Output = Result<H256, Error>> + Send + '_>> {
            (**self).submit_checkpoint(duty, signatures)
        }
    }

    fn test_state_with_seed() -> Arc<ProviderState> {
        let storage = Arc::new(crate::Storage::new());
        Arc::new(crate::ProviderState::with_seed(storage, "//Alice").unwrap())
    }

    fn test_state_with_bucket(bucket_id: BucketId) -> Arc<ProviderState> {
        let storage = Arc::new(crate::Storage::new());
        // Init bucket and store some data so the bucket has content
        storage.init_bucket(bucket_id, 1024 * 1024);
        let data = b"test data".to_vec();
        let hash = sp_core::hashing::blake2_256(&data);
        let data_root = H256::from(hash);
        let _ = storage.store_node(bucket_id, data_root, data, None);
        storage.commit(bucket_id, vec![data_root]).unwrap();
        Arc::new(crate::ProviderState::with_seed(storage, "//Alice").unwrap())
    }

    #[test]
    fn test_config_default() {
        let config = CheckpointCoordinatorConfig::default();
        assert_eq!(config.poll_interval, Duration::from_secs(6));
        assert!(config.auto_submit);
    }

    #[test]
    fn test_checkpoint_result_variants() {
        let success = CheckpointResult::Success {
            bucket_id: 1,
            window: 5,
            mmr_root: H256::zero(),
            signers: vec!["alice".to_string()],
        };
        assert!(matches!(success, CheckpointResult::Success { .. }));

        let insufficient = CheckpointResult::InsufficientSignatures {
            bucket_id: 1,
            window: 5,
            collected: 1,
            required: 3,
        };
        assert!(matches!(
            insufficient,
            CheckpointResult::InsufficientSignatures { .. }
        ));
    }

    #[test]
    fn test_sign_proposal_request_serialization() {
        let request = SignProposalRequest {
            bucket_id: 1,
            mmr_root: "0x0000000000000000000000000000000000000000000000000000000000000000"
                .to_string(),
            start_seq: 0,
            leaf_count: 10,
            window: 5,
        };

        let json = serde_json::to_string(&request).unwrap();
        assert!(json.contains("bucket_id"));
        assert!(json.contains("mmr_root"));
    }

    #[tokio::test]
    async fn test_no_bucket_data() {
        let mock = MockCheckpointChainClient::new(500);
        let state = test_state_with_seed();
        let config = CheckpointCoordinatorConfig::default();
        let coordinator = CheckpointCoordinator::new(config, state, Box::new(mock));

        // No bucket exists => None
        let duty = coordinator.get_checkpoint_duty(99).await.unwrap();
        assert!(duty.is_none());
    }

    #[tokio::test]
    async fn test_duty_found_submit_ok() {
        let mock = Arc::new(MockCheckpointChainClient::new(500));
        let state = test_state_with_bucket(1);
        let config = CheckpointCoordinatorConfig::default();
        let coordinator = CheckpointCoordinator::new(config, state, Box::new(Arc::clone(&mock)));

        let duty = coordinator.get_checkpoint_duty(1).await.unwrap().unwrap();
        assert_eq!(duty.bucket_id, 1);
        assert_eq!(duty.window, 5); // 500 / 100

        let result = coordinator.coordinate_checkpoint(&duty).await;
        assert!(matches!(result, CheckpointResult::Success { .. }));

        let submitted = mock.submitted.lock().await;
        assert_eq!(submitted.len(), 1);
        assert_eq!(submitted[0], (1, 5));
    }

    #[tokio::test]
    async fn test_submit_fails() {
        let mock = Arc::new(
            MockCheckpointChainClient::new(500)
                .with_submit_error(Error::Internal("tx failed".to_string())),
        );
        let state = test_state_with_bucket(1);
        let config = CheckpointCoordinatorConfig::default();
        let coordinator = CheckpointCoordinator::new(config, state, Box::new(Arc::clone(&mock)));

        let duty = coordinator.get_checkpoint_duty(1).await.unwrap().unwrap();
        let result = coordinator.coordinate_checkpoint(&duty).await;
        assert!(matches!(result, CheckpointResult::SubmissionFailed { .. }));
    }

    #[tokio::test]
    async fn test_pause_resume() {
        let mock = MockCheckpointChainClient::new(500);
        let state = test_state_with_seed();
        let config = CheckpointCoordinatorConfig {
            poll_interval: Duration::from_millis(50),
            ..Default::default()
        };
        let coordinator = CheckpointCoordinator::new(config, state, Box::new(mock));

        let handle = coordinator.start(None).await.unwrap();
        assert!(handle.is_running());

        handle.pause().await.unwrap();
        tokio::time::sleep(Duration::from_millis(100)).await;

        handle.resume().await.unwrap();
        tokio::time::sleep(Duration::from_millis(100)).await;

        handle.stop().await.unwrap();
        tokio::time::sleep(Duration::from_millis(50)).await;
        assert!(!handle.is_running());
    }

    #[tokio::test]
    async fn test_force_checkpoint() {
        let mock = Arc::new(MockCheckpointChainClient::new(500));
        let state = test_state_with_bucket(1);
        let config = CheckpointCoordinatorConfig {
            poll_interval: Duration::from_secs(60), // Long interval so auto doesn't trigger
            ..Default::default()
        };
        let coordinator = CheckpointCoordinator::new(config, state, Box::new(Arc::clone(&mock)));

        let handle = coordinator.start(None).await.unwrap();

        handle.force_checkpoint(1).await.unwrap();
        tokio::time::sleep(Duration::from_millis(200)).await;

        let submitted = mock.submitted.lock().await;
        assert_eq!(submitted.len(), 1);

        handle.stop().await.unwrap();
    }
}
