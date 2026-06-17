// SPDX-License-Identifier: GPL-3.0-only

//! Challenge Responder - Automated response to on-chain challenges.
//!
//! This module provides a background service that monitors the blockchain
//! for challenges against this provider and automatically responds with
//! the required proof data.

use crate::{Error, ProviderState};
use sp_core::H256;
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
#[async_trait::async_trait]
pub trait ChallengeChainClient: Send + Sync {
    /// Poll the chain for active challenges targeting this provider.
    async fn poll_challenges(&self) -> Result<Vec<DetectedChallenge>, Error>;

    /// Submit a challenge response transaction.
    async fn submit_response(
        &self,
        challenge_id: (u32, u16),
        chunk_data: Vec<u8>,
        mmr_proof: storage_primitives::MmrProof,
        chunk_proof: storage_primitives::MerkleProof,
    ) -> Result<H256, Error>;
}

#[async_trait::async_trait]
impl<T: ChallengeChainClient> ChallengeChainClient for Arc<T> {
    async fn poll_challenges(&self) -> Result<Vec<DetectedChallenge>, Error> {
        self.as_ref().poll_challenges().await
    }

    async fn submit_response(
        &self,
        challenge_id: (u32, u16),
        chunk_data: Vec<u8>,
        mmr_proof: storage_primitives::MmrProof,
        chunk_proof: storage_primitives::MerkleProof,
    ) -> Result<H256, Error> {
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

/// Manually-decoded view of a `Challenge` struct from raw SCALE bytes.
///
/// We avoid the `subxt::dynamic::Value` -> typed conversion because that
/// requires metadata-aware decoding of generic `BalanceOf<T>` etc. The byte
/// layout of `Challenge<T>` is stable for the deployed runtimes, so we read
/// fixed offsets.
pub(crate) struct DecodedChallenge {
    pub(crate) bucket_id: u64,
    pub(crate) challenger: [u8; 32],
    pub(crate) mmr_root: H256,
    pub(crate) start_seq: u64,
    pub(crate) leaf_index: u64,
    pub(crate) chunk_index: u64,
}

/// Decode a SCALE-encoded `Vec<Challenge>` from `Challenges` storage and
/// return only the entries whose `provider` field matches `our_bytes`,
/// alongside their original index in the vec (which is what the pallet uses
/// as `ChallengeId::index`).
///
/// Layout per `Challenge<T>` (see `pallet/src/lib.rs`):
///   bucket_id (u64)         — 8
///   provider (AccountId32)  — 32
///   challenger (AccountId32)— 32
///   mmr_root (H256)         — 32
///   start_seq (u64)         — 8
///   leaf_index (u64)        — 8
///   chunk_index (u64)       — 8
///   deposit (Balance u128)  — 16
/// Total per entry: 144 bytes.
pub(crate) fn decode_challenge_vec_for_provider(
    mut bytes: &[u8],
    our_bytes: &[u8; 32],
) -> Result<Vec<(u16, DecodedChallenge)>, &'static str> {
    use codec::Decode;

    let len = <codec::Compact<u32>>::decode(&mut bytes)
        .map_err(|_| "compact length prefix")?
        .0 as usize;

    const ENTRY_SIZE: usize = 144;
    if bytes.len() < len.saturating_mul(ENTRY_SIZE) {
        return Err("vec body shorter than length prefix implies");
    }

    let mut out = Vec::new();
    for i in 0..len {
        let off = i * ENTRY_SIZE;
        let entry = &bytes[off..off + ENTRY_SIZE];

        let provider = &entry[8..40];
        if provider != our_bytes {
            continue;
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

        out.push((
            i as u16,
            DecodedChallenge {
                bucket_id,
                challenger,
                mmr_root,
                start_seq,
                leaf_index,
                chunk_index,
            },
        ));
    }
    Ok(out)
}

#[cfg(test)]
mod scale_decoding_tests {
    use super::*;
    use codec::Encode;

    /// Round-trip: SCALE-encode a Vec of two pseudo-Challenges (the second
    /// matches our account), then verify the decoder picks out only the
    /// matching one with the correct index.
    #[test]
    fn decode_challenge_vec_filters_by_provider() {
        let our_bytes: [u8; 32] = [9u8; 32];
        let other_bytes: [u8; 32] = [1u8; 32];

        // Build raw bytes that match the pallet's `Challenge` layout. We hand-
        // roll this rather than rely on the pallet's own struct so the test
        // would catch a layout drift between the two crates.
        fn build(provider: [u8; 32]) -> Vec<u8> {
            let mut buf = Vec::new();
            let bucket_id: u64 = 42;
            let challenger: [u8; 32] = [2u8; 32];
            let mmr_root: [u8; 32] = [3u8; 32];
            let start_seq: u64 = 100;
            let leaf_index: u64 = 7;
            let chunk_index: u64 = 0;
            let deposit: u128 = 1_000_000_000_000;
            buf.extend_from_slice(&bucket_id.to_le_bytes());
            buf.extend_from_slice(&provider);
            buf.extend_from_slice(&challenger);
            buf.extend_from_slice(&mmr_root);
            buf.extend_from_slice(&start_seq.to_le_bytes());
            buf.extend_from_slice(&leaf_index.to_le_bytes());
            buf.extend_from_slice(&chunk_index.to_le_bytes());
            buf.extend_from_slice(&deposit.to_le_bytes());
            buf
        }

        let entry_other = build(other_bytes);
        let entry_us = build(our_bytes);
        let mut payload = codec::Compact(2u32).encode();
        payload.extend(entry_other);
        payload.extend(entry_us);

        let matched = decode_challenge_vec_for_provider(&payload, &our_bytes).expect("decodes");
        assert_eq!(matched.len(), 1, "only our entry should match");
        let (idx, challenge) = &matched[0];
        assert_eq!(*idx, 1u16, "matched entry is at position 1");
        assert_eq!(challenge.bucket_id, 42);
        assert_eq!(challenge.start_seq, 100);
        assert_eq!(challenge.leaf_index, 7);
    }
}
