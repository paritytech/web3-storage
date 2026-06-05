//! Challenge Responder - Automated response to on-chain challenges.
//!
//! This module provides a background service that monitors the blockchain
//! for challenges against this provider and automatically responds with
//! the required proof data.

use crate::{Error, ProviderState};
use sp_core::H256;
use std::future::Future;
use std::pin::Pin;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;
use storage_primitives::BucketId;
use tokio::sync::mpsc;

/// Configuration for the challenge responder.
#[derive(Clone, Debug)]
pub struct ChallengeResponderConfig {
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

/// Trait abstracting chain interactions for the challenge responder.
pub trait ChallengeChainClient: Send + Sync {
    /// Poll the chain for active challenges targeting this provider.
    fn poll_challenges(
        &self,
    ) -> Pin<Box<dyn Future<Output = Result<Vec<DetectedChallenge>, Error>> + Send + '_>>;

    /// Submit a challenge response transaction.
    fn submit_response(
        &self,
        challenge_id: (u32, u16),
        chunk_data: Vec<u8>,
        mmr_proof: storage_primitives::MmrProof,
        chunk_proof: storage_primitives::MerkleProof,
    ) -> Pin<Box<dyn Future<Output = Result<H256, Error>> + Send + '_>>;
}

/// Production implementation that talks to the chain via subxt.
pub struct SubxtChallengeChainClient {
    api: subxt::OnlineClient<subxt::PolkadotConfig>,
    signer: subxt_signer::sr25519::Keypair,
}

impl SubxtChallengeChainClient {
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
            "Challenge responder signer: {}",
            sp_core::crypto::AccountId32::from(signer.public_key().0).to_string()
        );
        tracing::info!("Challenge responder connected to {}", chain_ws_url);

        Ok(Self { api, signer })
    }
}

impl ChallengeChainClient for SubxtChallengeChainClient {
    fn poll_challenges(
        &self,
    ) -> Pin<Box<dyn Future<Output = Result<Vec<DetectedChallenge>, Error>> + Send + '_>> {
        Box::pin(async move {
            let _storage = self
                .api
                .storage()
                .at_latest()
                .await
                .map_err(|e| Error::Internal(format!("Failed to get storage: {e}")))?;

            // TODO: Implement proper storage query for Challenges
            // For now, return empty - challenges would be detected via events
            Ok(vec![])
        })
    }

    fn submit_response(
        &self,
        challenge_id: (u32, u16),
        chunk_data: Vec<u8>,
        mmr_proof: storage_primitives::MmrProof,
        chunk_proof: storage_primitives::MerkleProof,
    ) -> Pin<Box<dyn Future<Output = Result<H256, Error>> + Send + '_>> {
        Box::pin(async move {
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
                    ("chunk_data", subxt::dynamic::Value::from_bytes(&chunk_data)),
                    ("mmr_proof", mmr_proof_val),
                    ("chunk_proof", chunk_proof_val),
                ],
            );

            let tx = subxt::dynamic::tx(
                "StorageProvider",
                "respond_to_challenge",
                vec![challenge_id_val, response_val],
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
    chain_client: Box<dyn ChallengeChainClient>,
}

impl ChallengeResponder {
    /// Create a new challenge responder.
    pub fn new(
        config: ChallengeResponderConfig,
        state: Arc<ProviderState>,
        chain_client: Box<dyn ChallengeChainClient>,
    ) -> Self {
        Self {
            config,
            state,
            chain_client,
        }
    }

    /// Start the challenge responder background service.
    pub async fn start(
        self,
        callback: Option<Arc<dyn Fn(ChallengeResponseResult) + Send + Sync>>,
    ) -> Result<ChallengeResponderHandle, Error> {
        let (command_tx, command_rx) = mpsc::channel::<ResponderCommand>(32);
        let running = Arc::new(AtomicBool::new(true));
        let running_clone = running.clone();

        tokio::spawn(async move {
            self.run_loop(command_rx, running_clone, callback).await;
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

                    match self.chain_client.poll_challenges().await {
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
            .chain_client
            .submit_response(challenge_id, chunk_data, mmr_proof, chunk_proof)
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
}
