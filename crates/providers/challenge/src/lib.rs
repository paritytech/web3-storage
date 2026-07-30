// SPDX-License-Identifier: Apache-2.0

//! Challenge Responder - Automated response to on-chain challenges.
//!
//! This crate provides a background service that monitors the blockchain
//! for challenges against this provider and automatically responds with
//! the required proof data.

use sp_core::H256;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;
use storage_primitives::{BucketId, MerkleProof, MmrProof};
use tokio::sync::mpsc;

/// Errors surfaced by the challenge responder.
#[derive(Debug, thiserror::Error)]
pub enum ChallengeError {
    /// Chain interaction failed (polling or submitting).
    #[error("chain error: {0}")]
    Chain(String),
    /// Local storage could not produce the requested proof data.
    #[error("storage error: {0}")]
    Storage(String),
    /// Internal service error (e.g. control channel closed).
    #[error("internal error: {0}")]
    Internal(String),
}

/// Local proof data the responder needs to answer a challenge.
///
/// Implemented by the provider node's storage backend; kept narrow so this
/// crate stays decoupled from the full storage engine.
pub trait ChallengeProofSource: Send + Sync {
    /// Generate an MMR proof for the given leaf of a bucket's commitment.
    fn get_mmr_proof(
        &self,
        bucket_id: BucketId,
        leaf_index: u64,
    ) -> Result<MmrProof, ChallengeError>;

    /// Fetch a chunk and its Merkle proof under the given data root.
    fn get_chunk_at_index(
        &self,
        data_root: H256,
        chunk_index: u64,
    ) -> Result<(Vec<u8>, MerkleProof), ChallengeError>;
}

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
#[async_trait::async_trait]
pub trait ChallengeChainClient: Send + Sync {
    /// Poll the chain for active challenges targeting this provider.
    async fn poll_challenges(&self) -> Result<Vec<DetectedChallenge>, ChallengeError>;

    /// Submit a challenge response transaction.
    async fn submit_response(
        &self,
        challenge_id: (u32, u16),
        chunk_data: Vec<u8>,
        mmr_proof: MmrProof,
        chunk_proof: MerkleProof,
    ) -> Result<H256, ChallengeError>;
}

#[async_trait::async_trait]
impl<T: ChallengeChainClient> ChallengeChainClient for Arc<T> {
    async fn poll_challenges(&self) -> Result<Vec<DetectedChallenge>, ChallengeError> {
        self.as_ref().poll_challenges().await
    }

    async fn submit_response(
        &self,
        challenge_id: (u32, u16),
        chunk_data: Vec<u8>,
        mmr_proof: MmrProof,
        chunk_proof: MerkleProof,
    ) -> Result<H256, ChallengeError> {
        self.as_ref()
            .submit_response(challenge_id, chunk_data, mmr_proof, chunk_proof)
            .await
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
    pub async fn stop(&self) -> Result<(), ChallengeError> {
        self.command_tx
            .send(ResponderCommand::Stop)
            .await
            .map_err(|_| ChallengeError::Internal("Responder channel closed".to_string()))
    }

    /// Pause automatic responses.
    pub async fn pause(&self) -> Result<(), ChallengeError> {
        self.command_tx
            .send(ResponderCommand::Pause)
            .await
            .map_err(|_| ChallengeError::Internal("Responder channel closed".to_string()))
    }

    /// Resume automatic responses.
    pub async fn resume(&self) -> Result<(), ChallengeError> {
        self.command_tx
            .send(ResponderCommand::Resume)
            .await
            .map_err(|_| ChallengeError::Internal("Responder channel closed".to_string()))
    }
}

/// Challenge responder service.
pub struct ChallengeResponder {
    config: ChallengeResponderConfig,
    proof_source: Arc<dyn ChallengeProofSource>,
    chain_client: Box<dyn ChallengeChainClient>,
}

impl ChallengeResponder {
    /// Create a new challenge responder.
    pub fn new(
        config: ChallengeResponderConfig,
        proof_source: Arc<dyn ChallengeProofSource>,
        chain_client: Box<dyn ChallengeChainClient>,
    ) -> Self {
        Self {
            config,
            proof_source,
            chain_client,
        }
    }

    /// Start the challenge responder background service.
    pub async fn start(
        self,
        callback: Option<Arc<dyn Fn(ChallengeResponseResult) + Send + Sync>>,
    ) -> Result<ChallengeResponderHandle, ChallengeError> {
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
                // Prefer control commands over the poll tick: the interval's
                // first tick fires immediately, so an unbiased select could
                // service a poll before a Pause/Stop queued right after start().
                biased;

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
            .proof_source
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
            .proof_source
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

/// Manually-decoded view of a `Challenge` struct from raw SCALE bytes.
///
/// We avoid the `subxt::dynamic::Value` -> typed conversion because that
/// requires metadata-aware decoding of generic `BalanceOf<T>` etc. The byte
/// layout of `Challenge<T>` is stable for the deployed runtimes, so we read
/// fixed offsets.
///
/// Produced by [`decode_challenge_for_provider`] for [`ChallengeChainClient`]
/// implementations (e.g. the provider node's subxt client) when polling the
/// on-chain `Challenges` storage; also exercised directly by an integration
/// test against encoded `Challenge<T>` bytes.
pub struct DecodedChallenge {
    pub bucket_id: u64,
    pub challenger: [u8; 32],
    pub mmr_root: H256,
    pub start_seq: u64,
    pub leaf_index: u64,
    pub chunk_index: u64,
}

/// Total SCALE-encoded size of a single `Challenge<T>` value (fixed-width
/// fields only, see the layout below).
const CHALLENGE_ENTRY_SIZE: usize = 144;

/// Decode a single SCALE-encoded `Challenge` value from `Challenges` storage
/// (the map is now a `StorageDoubleMap<BlockNumber, u16, Challenge>`, so each
/// key holds exactly one challenge rather than a `Vec`). Returns `Some` iff
/// the decoded `provider` field matches `our_bytes`; `None` when the
/// challenge targets a different provider.
///
/// Layout of `Challenge<T>` (see `crates/pallets/storage-provider/src/lib.rs`):
///   bucket_id (u64)         — 8
///   provider (AccountId32)  — 32
///   challenger (AccountId32)— 32
///   mmr_root (H256)         — 32
///   start_seq (u64)         — 8
///   leaf_index (u64)        — 8
///   chunk_index (u64)       — 8
///   deposit (Balance u128)  — 16
/// Total: 144 bytes.
///
/// This is the crate's decode helper for [`ChallengeChainClient`]
/// implementations that read raw `Challenges` entries (the provider node's
/// subxt client depends on it in production); the fixed-offset layout is also
/// exercised directly by an integration test.
pub fn decode_challenge_for_provider(
    encoded: &[u8],
    our_bytes: &[u8; 32],
) -> Result<Option<DecodedChallenge>, &'static str> {
    if encoded.len() < CHALLENGE_ENTRY_SIZE {
        return Err("challenge value shorter than expected layout");
    }
    let entry = &encoded[..CHALLENGE_ENTRY_SIZE];

    let provider = &entry[8..40];
    if provider != our_bytes {
        return Ok(None);
    }

    let bucket_id = u64::from_le_bytes(entry[0..8].try_into().expect("8 bytes"));
    let mut challenger = [0u8; 32];
    challenger.copy_from_slice(&entry[40..72]);
    let mut root_bytes = [0u8; 32];
    root_bytes.copy_from_slice(&entry[72..104]);
    let mmr_root = H256::from(root_bytes);
    let start_seq = u64::from_le_bytes(entry[104..112].try_into().expect("8 bytes"));
    let leaf_index = u64::from_le_bytes(entry[112..120].try_into().expect("8 bytes"));
    let chunk_index = u64::from_le_bytes(entry[120..128].try_into().expect("8 bytes"));
    // deposit at entry[128..144] — not needed for the response.

    Ok(Some(DecodedChallenge {
        bucket_id,
        challenger,
        mmr_root,
        start_seq,
        leaf_index,
        chunk_index,
    }))
}
