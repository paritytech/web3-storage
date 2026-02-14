//! Checkpoint Coordinator - Provider-initiated checkpoint coordination.
//!
//! This module provides a background service that coordinates with other
//! providers to autonomously submit checkpoints without requiring the
//! client to be online.

use crate::{Error, ProviderState};
use codec::Encode;
use sp_core::{Pair, H256};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;
use storage_primitives::{BucketId, CheckpointProposal};
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
}

impl Default for CheckpointCoordinatorConfig {
    fn default() -> Self {
        Self {
            chain_ws_url: "ws://127.0.0.1:9944".to_string(),
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
}

/// Checkpoint coordinator service.
pub struct CheckpointCoordinator {
    config: CheckpointCoordinatorConfig,
    state: Arc<ProviderState>,
    api: Option<OnlineClient<PolkadotConfig>>,
    signer: Option<Keypair>,
    http_client: reqwest::Client,
}

impl CheckpointCoordinator {
    /// Create a new checkpoint coordinator.
    pub fn new(config: CheckpointCoordinatorConfig, state: Arc<ProviderState>) -> Self {
        Self {
            config,
            state,
            api: None,
            signer: None,
            http_client: reqwest::Client::builder()
                .timeout(Duration::from_secs(10))
                .build()
                .expect("Failed to create HTTP client"),
        }
    }

    /// Connect to the blockchain.
    pub async fn connect(&mut self) -> Result<(), Error> {
        let api = OnlineClient::<PolkadotConfig>::from_url(&self.config.chain_ws_url)
            .await
            .map_err(|e| Error::Internal(format!("Failed to connect to chain: {}", e)))?;

        self.api = Some(api);

        // Set up signer from provider state if available
        if let Some(ref kp) = self.state.keypair {
            let raw = kp.to_raw_vec();
            let secret_bytes: [u8; 32] = raw[..32]
                .try_into()
                .map_err(|_| Error::Internal("Invalid secret key length".to_string()))?;
            let signer = Keypair::from_secret_key(secret_bytes)
                .map_err(|e| Error::Internal(format!("Failed to create signer: {}", e)))?;
            self.signer = Some(signer);
        }

        tracing::info!(
            "Checkpoint coordinator connected to {}",
            self.config.chain_ws_url
        );
        Ok(())
    }

    /// Start the checkpoint coordinator background service.
    pub async fn start(
        self,
        callback: Option<Arc<dyn Fn(CheckpointResult) + Send + Sync>>,
    ) -> Result<CheckpointCoordinatorHandle, Error> {
        if self.api.is_none() {
            return Err(Error::Internal("Not connected to chain".to_string()));
        }

        let (command_tx, command_rx) = mpsc::channel::<CoordinatorCommand>(32);
        let running = Arc::new(AtomicBool::new(true));
        let running_clone = running.clone();

        let coordinator = self;

        tokio::spawn(async move {
            coordinator
                .run_loop(command_rx, running_clone, callback)
                .await;
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
                            if let Ok(Some(duty)) = self.get_checkpoint_duty(bucket_id).await {
                                let result = self.coordinate_checkpoint(&duty).await;
                                if let Some(ref cb) = callback {
                                    cb(result);
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
            None => return Ok(None),
        };

        // Build duty from local state
        // Note: In production, this would also query chain for config
        let duty = CheckpointDuty {
            bucket_id,
            window: 0, // Would calculate from current block
            mmr_root: bucket.mmr_root,
            start_seq: bucket.start_seq,
            leaf_count: bucket.leaf_count(),
            is_leader: true, // Would calculate based on window and provider index
            peer_endpoints: vec![], // Would get from chain
            interval: 100,
            grace_period: 20,
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
        let url = format!("{}/checkpoint/sign", endpoint);

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
            .map_err(|e| Error::Internal(format!("HTTP request failed: {}", e)))?;

        if !response.status().is_success() {
            return Err(Error::Internal(format!(
                "Peer returned error: {}",
                response.status()
            )));
        }

        response
            .json::<SignProposalResponse>()
            .await
            .map_err(|e| Error::Internal(format!("Failed to parse response: {}", e)))
    }

    /// Submit the checkpoint to the chain.
    async fn submit_checkpoint(
        &self,
        duty: &CheckpointDuty,
        signatures: Vec<(String, String)>,
    ) -> Result<H256, Error> {
        let api = self
            .api
            .as_ref()
            .ok_or_else(|| Error::Internal("Not connected to chain".to_string()))?;

        let signer = self
            .signer
            .as_ref()
            .ok_or_else(|| Error::Internal("No signer configured".to_string()))?;

        // Build signature tuples for the extrinsic
        let sig_values: Vec<_> = signatures
            .iter()
            .map(|(account, sig)| {
                subxt::dynamic::Value::unnamed_composite(vec![
                    // Account ID
                    subxt::dynamic::Value::from_bytes(
                        &hex::decode(account.trim_start_matches("0x")).unwrap_or_default(),
                    ),
                    // Signature (Sr25519)
                    subxt::dynamic::Value::unnamed_variant(
                        "Sr25519",
                        vec![subxt::dynamic::Value::from_bytes(
                            &hex::decode(sig.trim_start_matches("0x")).unwrap_or_default(),
                        )],
                    ),
                ])
            })
            .collect();

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
            .map_err(|e| Error::Internal(format!("Failed to submit tx: {}", e)))?;

        let _events = tx_progress
            .wait_for_finalized_success()
            .await
            .map_err(|e| Error::Internal(format!("Transaction failed: {}", e)))?;

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
        assert_eq!(config.chain_ws_url, "ws://127.0.0.1:9944");
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
