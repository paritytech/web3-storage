//! Challenge Responder - Automated response to on-chain challenges.
//!
//! This module provides a background service that monitors the blockchain
//! for challenges against this provider and automatically responds with
//! the required proof data.

use crate::{Error, ProviderState};
use sp_core::{Pair, H256};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;
use storage_primitives::BucketId;
use subxt::{OnlineClient, PolkadotConfig};
use subxt_signer::sr25519::Keypair;
use tokio::sync::mpsc;

/// Configuration for the challenge responder.
#[derive(Clone, Debug)]
pub struct ChallengeResponderConfig {
    /// WebSocket URL for the parachain.
    pub chain_ws_url: String,
    /// How often to poll for challenges (if not using subscriptions).
    pub poll_interval: Duration,
    /// Maximum time to spend gathering proof data.
    pub proof_timeout: Duration,
    /// Whether to automatically respond to challenges.
    pub auto_respond: bool,
}

impl Default for ChallengeResponderConfig {
    fn default() -> Self {
        Self {
            chain_ws_url: "ws://127.0.0.1:2222".to_string(),
            poll_interval: Duration::from_secs(6), // ~1 block
            proof_timeout: Duration::from_secs(30),
            auto_respond: true,
        }
    }
}

/// Information about a detected challenge.
#[derive(Clone, Debug)]
pub struct DetectedChallenge {
    /// Bucket being challenged.
    pub bucket_id: BucketId,
    /// Challenge deadline (block number).
    pub deadline: u32,
    /// Challenge index within the deadline.
    pub index: u16,
    /// MMR root being challenged.
    pub mmr_root: H256,
    /// Start sequence of the commitment.
    pub start_seq: u64,
    /// Leaf index in the MMR to prove.
    pub leaf_index: u64,
    /// Chunk index within the leaf to prove.
    pub chunk_index: u64,
    /// Challenger's account.
    pub challenger: String,
    /// Block number when challenge was created.
    pub created_at_block: u32,
}

/// Result of responding to a challenge.
#[derive(Clone, Debug)]
pub enum ChallengeResponseResult {
    /// Successfully submitted response.
    Success {
        challenge_id: (u32, u16),
        block_hash: H256,
    },
    /// Failed to gather proof data.
    ProofGenerationFailed {
        challenge_id: (u32, u16),
        error: String,
    },
    /// Failed to submit response transaction.
    SubmissionFailed {
        challenge_id: (u32, u16),
        error: String,
    },
    /// Challenge data not found locally.
    DataNotFound {
        challenge_id: (u32, u16),
        bucket_id: BucketId,
        leaf_index: u64,
    },
}

/// Commands for controlling the responder.
#[derive(Debug)]
pub enum ResponderCommand {
    /// Stop the responder.
    Stop,
    /// Pause automatic responses.
    Pause,
    /// Resume automatic responses.
    Resume,
    /// Manually respond to a specific challenge.
    RespondTo(DetectedChallenge),
}

/// Handle for controlling the challenge responder.
pub struct ChallengeResponderHandle {
    command_tx: mpsc::Sender<ResponderCommand>,
    running: Arc<AtomicBool>,
}

impl ChallengeResponderHandle {
    /// Check if the responder is running.
    pub fn is_running(&self) -> bool {
        self.running.load(Ordering::SeqCst)
    }

    /// Stop the responder.
    pub async fn stop(&self) -> Result<(), Error> {
        self.command_tx
            .send(ResponderCommand::Stop)
            .await
            .map_err(|_| Error::Internal("Responder channel closed".to_string()))
    }

    /// Pause automatic responses.
    pub async fn pause(&self) -> Result<(), Error> {
        self.command_tx
            .send(ResponderCommand::Pause)
            .await
            .map_err(|_| Error::Internal("Responder channel closed".to_string()))
    }

    /// Resume automatic responses.
    pub async fn resume(&self) -> Result<(), Error> {
        self.command_tx
            .send(ResponderCommand::Resume)
            .await
            .map_err(|_| Error::Internal("Responder channel closed".to_string()))
    }
}

/// Challenge responder service.
pub struct ChallengeResponder {
    config: ChallengeResponderConfig,
    state: Arc<ProviderState>,
    api: Option<OnlineClient<PolkadotConfig>>,
    signer: Option<Keypair>,
}

impl ChallengeResponder {
    /// Create a new challenge responder.
    pub fn new(config: ChallengeResponderConfig, state: Arc<ProviderState>) -> Self {
        Self {
            config,
            state,
            api: None,
            signer: None,
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
            // Convert sp_core keypair to subxt_signer keypair
            // sr25519 to_raw_vec returns 64 bytes (seed + nonce), we need just the first 32
            let raw = kp.to_raw_vec();
            let secret_bytes: [u8; 32] = raw[..32]
                .try_into()
                .map_err(|_| Error::Internal("Invalid secret key length".to_string()))?;
            let signer = Keypair::from_secret_key(secret_bytes)
                .map_err(|e| Error::Internal(format!("Failed to create signer: {}", e)))?;
            self.signer = Some(signer);
        }

        tracing::info!(
            "Challenge responder connected to {}",
            self.config.chain_ws_url
        );
        Ok(())
    }

    /// Start the challenge responder background service.
    pub async fn start(
        self,
        callback: Option<Arc<dyn Fn(ChallengeResponseResult) + Send + Sync>>,
    ) -> Result<ChallengeResponderHandle, Error> {
        if self.api.is_none() {
            return Err(Error::Internal("Not connected to chain".to_string()));
        }

        let (command_tx, command_rx) = mpsc::channel::<ResponderCommand>(32);
        let running = Arc::new(AtomicBool::new(true));
        let running_clone = running.clone();

        let responder = self;

        tokio::spawn(async move {
            responder
                .run_loop(command_rx, running_clone, callback)
                .await;
        });

        Ok(ChallengeResponderHandle {
            command_tx,
            running,
        })
    }

    /// Main responder loop.
    async fn run_loop(
        self,
        mut command_rx: mpsc::Receiver<ResponderCommand>,
        running: Arc<AtomicBool>,
        callback: Option<Arc<dyn Fn(ChallengeResponseResult) + Send + Sync>>,
    ) {
        let mut paused = false;
        let mut interval = tokio::time::interval(self.config.poll_interval);

        tracing::info!("Challenge responder started");

        loop {
            tokio::select! {
                cmd = command_rx.recv() => {
                    match cmd {
                        Some(ResponderCommand::Stop) | None => {
                            tracing::info!("Challenge responder stopping");
                            running.store(false, Ordering::SeqCst);
                            break;
                        }
                        Some(ResponderCommand::Pause) => {
                            tracing::info!("Challenge responder paused");
                            paused = true;
                        }
                        Some(ResponderCommand::Resume) => {
                            tracing::info!("Challenge responder resumed");
                            paused = false;
                        }
                        Some(ResponderCommand::RespondTo(challenge)) => {
                            let result = self.respond_to_challenge(&challenge).await;
                            if let Some(ref cb) = callback {
                                cb(result);
                            }
                        }
                    }
                }
                _ = interval.tick() => {
                    if paused || !self.config.auto_respond {
                        continue;
                    }

                    // Poll for challenges
                    match self.poll_challenges().await {
                        Ok(challenges) => {
                            for challenge in challenges {
                                tracing::info!(
                                    "Detected challenge for bucket {} (deadline: {}, index: {})",
                                    challenge.bucket_id,
                                    challenge.deadline,
                                    challenge.index
                                );

                                let result = self.respond_to_challenge(&challenge).await;
                                if let Some(ref cb) = callback {
                                    cb(result);
                                }
                            }
                        }
                        Err(e) => {
                            tracing::warn!("Failed to poll for challenges: {}", e);
                        }
                    }
                }
            }
        }
    }

    /// Poll for active challenges against this provider.
    async fn poll_challenges(&self) -> Result<Vec<DetectedChallenge>, Error> {
        let api = self
            .api
            .as_ref()
            .ok_or_else(|| Error::Internal("Not connected to chain".to_string()))?;

        // Query Challenges storage
        // This is a simplified version - in production, we'd use proper storage queries
        // and filter for challenges targeting this provider
        let _storage = api
            .storage()
            .at_latest()
            .await
            .map_err(|e| Error::Internal(format!("Failed to get storage: {}", e)))?;

        // TODO: Implement proper storage query for Challenges
        // For now, return empty - challenges would be detected via events
        Ok(vec![])
    }

    /// Respond to a specific challenge.
    async fn respond_to_challenge(&self, challenge: &DetectedChallenge) -> ChallengeResponseResult {
        let challenge_id = (challenge.deadline, challenge.index);

        tracing::info!(
            "Responding to challenge {:?} for bucket {}",
            challenge_id,
            challenge.bucket_id
        );

        // Step 1: Generate MMR proof (includes the leaf with data_root)
        let mmr_proof = match self
            .state
            .storage
            .get_mmr_proof(challenge.bucket_id, challenge.leaf_index)
        {
            Ok(proof) => proof,
            Err(e) => {
                tracing::error!("Failed to generate MMR proof: {}", e);
                return ChallengeResponseResult::ProofGenerationFailed {
                    challenge_id,
                    error: e.to_string(),
                };
            }
        };

        // Step 2: Get chunk data and Merkle proof using data_root from MMR leaf
        let data_root = mmr_proof.leaf.data_root;
        let (chunk_data, chunk_proof) = match self
            .state
            .storage
            .get_chunk_at_index(data_root, challenge.chunk_index)
        {
            Ok(data) => data,
            Err(e) => {
                tracing::error!("Failed to get chunk data: {}", e);
                return ChallengeResponseResult::DataNotFound {
                    challenge_id,
                    bucket_id: challenge.bucket_id,
                    leaf_index: challenge.leaf_index,
                };
            }
        };

        // Step 3: Submit response transaction
        match self
            .submit_response(challenge_id, &chunk_data, &mmr_proof, &chunk_proof)
            .await
        {
            Ok(block_hash) => {
                tracing::info!(
                    "Successfully responded to challenge {:?} in block {:?}",
                    challenge_id,
                    block_hash
                );
                ChallengeResponseResult::Success {
                    challenge_id,
                    block_hash,
                }
            }
            Err(e) => {
                tracing::error!("Failed to submit response: {}", e);
                ChallengeResponseResult::SubmissionFailed {
                    challenge_id,
                    error: e.to_string(),
                }
            }
        }
    }

    /// Submit the challenge response transaction.
    async fn submit_response(
        &self,
        challenge_id: (u32, u16),
        chunk_data: &[u8],
        mmr_proof: &storage_primitives::MmrProof,
        chunk_proof: &storage_primitives::MerkleProof,
    ) -> Result<H256, Error> {
        let api = self
            .api
            .as_ref()
            .ok_or_else(|| Error::Internal("Not connected to chain".to_string()))?;

        let signer = self
            .signer
            .as_ref()
            .ok_or_else(|| Error::Internal("No signer configured".to_string()))?;

        // Build ChallengeId
        let challenge_id_val = subxt::dynamic::Value::named_composite(vec![
            (
                "deadline",
                subxt::dynamic::Value::u128(challenge_id.0 as u128),
            ),
            ("index", subxt::dynamic::Value::u128(challenge_id.1 as u128)),
        ]);

        // Build MmrProof value
        let mmr_proof_val = subxt::dynamic::Value::named_composite(vec![
            (
                "peaks",
                subxt::dynamic::Value::unnamed_composite(
                    mmr_proof
                        .peaks
                        .iter()
                        .map(|p| subxt::dynamic::Value::from_bytes(p.as_bytes()))
                        .collect::<Vec<_>>(),
                ),
            ),
            (
                "leaf",
                subxt::dynamic::Value::named_composite(vec![
                    (
                        "data_root",
                        subxt::dynamic::Value::from_bytes(mmr_proof.leaf.data_root.as_bytes()),
                    ),
                    (
                        "data_size",
                        subxt::dynamic::Value::u128(mmr_proof.leaf.data_size as u128),
                    ),
                    (
                        "total_size",
                        subxt::dynamic::Value::u128(mmr_proof.leaf.total_size as u128),
                    ),
                ]),
            ),
            (
                "leaf_proof",
                subxt::dynamic::Value::named_composite(vec![
                    (
                        "siblings",
                        subxt::dynamic::Value::unnamed_composite(
                            mmr_proof
                                .leaf_proof
                                .siblings
                                .iter()
                                .map(|s| subxt::dynamic::Value::from_bytes(s.as_bytes()))
                                .collect::<Vec<_>>(),
                        ),
                    ),
                    (
                        "path",
                        subxt::dynamic::Value::unnamed_composite(
                            mmr_proof
                                .leaf_proof
                                .path
                                .iter()
                                .map(|b| subxt::dynamic::Value::bool(*b))
                                .collect::<Vec<_>>(),
                        ),
                    ),
                ]),
            ),
        ]);

        // Build chunk proof value
        let chunk_proof_val = subxt::dynamic::Value::named_composite(vec![
            (
                "siblings",
                subxt::dynamic::Value::unnamed_composite(
                    chunk_proof
                        .siblings
                        .iter()
                        .map(|s| subxt::dynamic::Value::from_bytes(s.as_bytes()))
                        .collect::<Vec<_>>(),
                ),
            ),
            (
                "path",
                subxt::dynamic::Value::unnamed_composite(
                    chunk_proof
                        .path
                        .iter()
                        .map(|b| subxt::dynamic::Value::bool(*b))
                        .collect::<Vec<_>>(),
                ),
            ),
        ]);

        // Build ChallengeResponse::Proof variant
        let response_val = subxt::dynamic::Value::named_variant(
            "Proof",
            vec![
                ("chunk_data", subxt::dynamic::Value::from_bytes(chunk_data)),
                ("mmr_proof", mmr_proof_val),
                ("chunk_proof", chunk_proof_val),
            ],
        );

        let tx = subxt::dynamic::tx(
            "StorageProvider",
            "respond_to_challenge",
            vec![challenge_id_val, response_val],
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_challenge_responder_config_default() {
        let config = ChallengeResponderConfig::default();
        assert_eq!(config.chain_ws_url, "ws://127.0.0.1:2222");
        assert_eq!(config.poll_interval, Duration::from_secs(6));
        assert!(config.auto_respond);
    }

    #[test]
    fn test_detected_challenge() {
        let challenge = DetectedChallenge {
            bucket_id: 1,
            deadline: 1000,
            index: 0,
            mmr_root: H256::zero(),
            start_seq: 0,
            leaf_index: 5,
            chunk_index: 0,
            challenger: "5GrwvaEF...".to_string(),
            created_at_block: 900,
        };

        assert_eq!(challenge.bucket_id, 1);
        assert_eq!(challenge.deadline, 1000);
        assert_eq!(challenge.leaf_index, 5);
    }

    #[test]
    fn test_challenge_response_result_variants() {
        let success = ChallengeResponseResult::Success {
            challenge_id: (1000, 0),
            block_hash: H256::zero(),
        };
        assert!(matches!(success, ChallengeResponseResult::Success { .. }));

        let proof_failed = ChallengeResponseResult::ProofGenerationFailed {
            challenge_id: (1000, 0),
            error: "test".to_string(),
        };
        assert!(matches!(
            proof_failed,
            ChallengeResponseResult::ProofGenerationFailed { .. }
        ));

        let not_found = ChallengeResponseResult::DataNotFound {
            challenge_id: (1000, 0),
            bucket_id: 1,
            leaf_index: 5,
        };
        assert!(matches!(
            not_found,
            ChallengeResponseResult::DataNotFound { .. }
        ));
    }

    #[test]
    fn test_responder_command_variants() {
        // Verify commands can be constructed
        let _stop = ResponderCommand::Stop;
        let _pause = ResponderCommand::Pause;
        let _resume = ResponderCommand::Resume;
    }
}
