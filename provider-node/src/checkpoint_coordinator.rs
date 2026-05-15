//! Checkpoint Coordinator - Provider-initiated checkpoint coordination.
//!
//! This module provides a background service that coordinates with other
//! providers to autonomously submit checkpoints without requiring the
//! client to be online.

use crate::chain_stream::ChainStream;
use crate::{Error, ProviderState};
use codec::Encode;
use sp_core::{crypto::Ss58Codec, Pair, H256};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;
use storage_primitives::{BucketId, CheckpointProposal};
use subxt::dynamic::At;
use subxt::{OnlineClient, PolkadotConfig};
use subxt_signer::sr25519::Keypair;
use tokio::sync::mpsc;

/// Configuration for the checkpoint coordinator.
#[derive(Clone, Debug)]
pub struct CheckpointCoordinatorConfig {
    /// WebSocket URL for the parachain.
    pub chain_ws_url: String,
    /// How often to poll for checkpoint duties.
    pub poll_interval: Duration,
    /// Timeout for collecting signatures from peers.
    pub signature_timeout: Duration,
    /// Whether to automatically submit checkpoints when leader.
    pub auto_submit: bool,
    /// Seed phrase or derivation path for signing (e.g., "//Alice").
    /// Used to create the subxt signer directly (avoids key conversion issues).
    pub seed: Option<String>,
}

impl Default for CheckpointCoordinatorConfig {
    fn default() -> Self {
        Self {
            chain_ws_url: "ws://127.0.0.1:2222".to_string(),
            poll_interval: Duration::from_secs(6), // ~1 block
            signature_timeout: Duration::from_secs(30),
            auto_submit: true,
            seed: None,
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
    api: OnlineClient<PolkadotConfig>,
    signer: Option<Keypair>,
    http_client: reqwest::Client,
}

impl CheckpointCoordinator {
    /// Create a new checkpoint coordinator with an already-connected chain client.
    pub fn new(
        config: CheckpointCoordinatorConfig,
        state: Arc<ProviderState>,
        api: OnlineClient<PolkadotConfig>,
    ) -> Result<Self, Error> {
        let signer = match config.seed.as_deref() {
            Some(seed) => {
                let uri: subxt_signer::SecretUri = seed
                    .parse()
                    .map_err(|e| Error::Internal(format!("Invalid seed URI: {e}")))?;
                let kp = Keypair::from_uri(&uri)
                    .map_err(|e| Error::Internal(format!("Failed to create signer: {e}")))?;
                tracing::info!(
                    "Checkpoint coordinator signer: {}",
                    sp_core::crypto::AccountId32::from(kp.public_key().0).to_ss58check()
                );
                Some(kp)
            }
            None => None,
        };

        Ok(Self {
            config,
            state,
            api,
            signer,
            http_client: reqwest::Client::builder()
                .timeout(Duration::from_secs(10))
                .build()
                .expect("Failed to create HTTP client"),
        })
    }

    /// Start the checkpoint coordinator background service.
    pub async fn start(
        self,
        callback: Option<Arc<dyn Fn(CheckpointResult) + Send + Sync>>,
    ) -> Result<CheckpointCoordinatorHandle, Error> {
        let (command_tx, command_rx) = mpsc::channel::<CoordinatorCommand>(32);
        let running = Arc::new(AtomicBool::new(true));
        let running_clone = running.clone();

        let coordinator = self;

        let running_exit = running.clone();
        tokio::spawn(async move {
            coordinator
                .run_loop(command_rx, running_clone, callback)
                .await;
            tracing::error!("Checkpoint coordinator run_loop exited unexpectedly!");
            running_exit.store(false, Ordering::SeqCst);
        });

        Ok(CheckpointCoordinatorHandle {
            command_tx,
            running,
        })
    }

    /// Main coordinator loop — drained by a [`ChainStream`] of finalized blocks.
    async fn run_loop(
        self,
        mut command_rx: mpsc::Receiver<CoordinatorCommand>,
        running: Arc<AtomicBool>,
        callback: Option<Arc<dyn Fn(CheckpointResult) + Send + Sync>>,
    ) {
        let mut paused = false;
        tracing::info!("Checkpoint coordinator started");

        let mut stream = ChainStream::new(self.api.clone(), self.config.poll_interval);

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
                next = stream.next() => {
                    if next.is_none() {
                        tracing::warn!("Checkpoint coordinator: chain stream ended");
                        running.store(false, Ordering::SeqCst);
                        break;
                    }

                    if paused || !self.config.auto_submit {
                        continue;
                    }

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
    ///
    /// Queries the chain for the current block and checkpoint config,
    /// then builds a duty from local storage state.
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

        // Query chain for current block number and checkpoint config
        let api = &self.api;

        let current_block = {
            let block = api
                .blocks()
                .at_latest()
                .await
                .map_err(|e| Error::Internal(format!("Failed to get latest block: {e}")))?;
            block.number() as u64
        };

        // Query checkpoint config from chain storage
        let config_query = subxt::dynamic::storage(
            "StorageProvider",
            "CheckpointConfigs",
            vec![subxt::dynamic::Value::u128(bucket_id as u128)],
        );
        let storage = api
            .storage()
            .at_latest()
            .await
            .map_err(|e| Error::Internal(format!("Failed to get storage: {e}")))?;
        let (interval, grace_period) = match storage
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
                (interval, grace_period)
            }
            None => (100u32, 20u32), // defaults
        };

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
        match self.submit_checkpoint(duty, signatures).await {
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

    /// Submit the checkpoint to the chain.
    async fn submit_checkpoint(
        &self,
        duty: &CheckpointDuty,
        signatures: Vec<(String, String)>,
    ) -> Result<H256, Error> {
        let api = &self.api;

        let signer = self
            .signer
            .as_ref()
            .ok_or_else(|| Error::Internal("No signer configured".to_string()))?;

        // Build signature tuples for the extrinsic
        let mut sig_values = Vec::with_capacity(signatures.len());
        for (account, sig) in &signatures {
            // Account is SS58 — decode to raw 32-byte AccountId
            let account_id: sp_core::crypto::AccountId32 =
                sp_core::crypto::Ss58Codec::from_ss58check(account).map_err(|e| {
                    Error::Internal(format!("Invalid SS58 account '{account}': {e:?}"))
                })?;
            let account_bytes: [u8; 32] = account_id.into();

            // Signature is hex-encoded with 0x prefix
            let sig_bytes = hex::decode(sig.trim_start_matches("0x"))
                .map_err(|e| Error::Internal(format!("Invalid signature hex: {e}")))?;

            sig_values.push(subxt::dynamic::Value::unnamed_composite(vec![
                // AccountId32
                subxt::dynamic::Value::from_bytes(account_bytes),
                // MultiSignature::Sr25519(signature)
                subxt::dynamic::Value::unnamed_variant(
                    "Sr25519",
                    vec![subxt::dynamic::Value::from_bytes(sig_bytes)],
                ),
            ]));
        }

        // Build the extrinsic
        let tx = subxt::dynamic::tx(
            "StorageProvider",
            "provider_checkpoint",
            vec![
                // bucket_id
                subxt::dynamic::Value::u128(duty.bucket_id as u128),
                // mmr_root
                subxt::dynamic::Value::from_bytes(duty.mmr_root.as_bytes()),
                // start_seq
                subxt::dynamic::Value::u128(duty.start_seq as u128),
                // leaf_count
                subxt::dynamic::Value::u128(duty.leaf_count as u128),
                // window
                subxt::dynamic::Value::u128(duty.window as u128),
                // signatures
                subxt::dynamic::Value::unnamed_composite(sig_values),
            ],
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

        Ok(H256::zero())
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

    #[test]
    fn test_config_default() {
        let config = CheckpointCoordinatorConfig::default();
        assert_eq!(config.chain_ws_url, "ws://127.0.0.1:2222");
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
}
