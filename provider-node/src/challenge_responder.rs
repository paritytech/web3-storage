// SPDX-License-Identifier: GPL-3.0-only

//! Challenge Responder - Automated response to on-chain challenges.
//!
//! This module provides a background service that reacts to
//! `ChallengeCreated` events (fanned out by the chain-state coordinator)
//! against this provider and automatically responds with the required proof
//! data. A full `Challenges` scan runs at startup and on every stream
//! (re)subscription to catch challenges raised while the node was down, plus
//! on a slow safety-net interval — a missed challenge means getting slashed,
//! so the event path is backstopped rather than trusted blindly.

use crate::chain_events::BlockEvent;
use crate::{Error, ProviderState};
use sp_core::H256;
use sp_runtime::AccountId32;
use std::str::FromStr;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;
use storage_primitives::BucketId;
use tokio::sync::{broadcast, mpsc};

/// Configuration for the challenge responder.
#[derive(Clone, Debug)]
pub struct ChallengeResponderConfig {
    /// Safety-net interval between full `Challenges` reconciliation scans.
    /// Challenges are normally handled event-driven; zero disables the scan.
    pub poll_interval: Duration,
    /// Maximum time to spend gathering proof data.
    pub proof_timeout: Duration,
    /// Whether to automatically respond to challenges.
    pub auto_respond: bool,
}

impl Default for ChallengeResponderConfig {
    fn default() -> Self {
        Self {
            poll_interval: Duration::from_secs(300),
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
    async fn poll_challenges(&self) -> Result<Vec<DetectedChallenge>, Error>;

    /// Point-read a single challenge by id, `None` if it is gone (already
    /// responded / reaped) or targets another provider. Backs the
    /// event-driven path, where `ChallengeCreated` carries the id but not
    /// the proof parameters.
    async fn fetch_challenge(
        &self,
        deadline: u32,
        index: u16,
    ) -> Result<Option<DetectedChallenge>, Error>;

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

    async fn fetch_challenge(
        &self,
        deadline: u32,
        index: u16,
    ) -> Result<Option<DetectedChallenge>, Error> {
        self.as_ref().fetch_challenge(deadline, index).await
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
    ///
    /// `events_rx` is a subscription to the chain-state coordinator's block
    /// event fan-out; the responder reacts to `ChallengeCreated` events and
    /// reconciles with a full scan on `Resubscribed` / lag / the safety-net
    /// interval.
    pub async fn start(
        self,
        events_rx: broadcast::Receiver<BlockEvent>,
        callback: Option<Arc<dyn Fn(ChallengeResponseResult) + Send + Sync>>,
    ) -> Result<ChallengeResponderHandle, Error> {
        let (command_tx, command_rx) = mpsc::channel::<ResponderCommand>(32);
        let running = Arc::new(AtomicBool::new(true));
        let running_clone = running.clone();

        tokio::spawn(async move {
            self.run_loop(command_rx, events_rx, running_clone, callback)
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
        mut events_rx: broadcast::Receiver<BlockEvent>,
        running: Arc<AtomicBool>,
        callback: Option<Arc<dyn Fn(ChallengeResponseResult) + Send + Sync>>,
    ) {
        let mut paused = false;
        // A closed broadcast channel (follower gone) yields `Closed` on every
        // poll; disarm the events select arm then, or the loop busy-spins.
        let mut events_open = true;
        // Only challenges against our own account are actionable; with an
        // unparseable provider id the point-read filter still protects us.
        let our_account = AccountId32::from_str(&self.state.provider_id).ok();
        // The safety-net interval's first tick fires immediately, doubling as
        // the startup bootstrap scan (challenges raised while the node was
        // down). With the safety net disabled, the bootstrap scan comes from
        // the follower's `Resubscribed` event on its first connect instead.
        let safety_net = !self.config.poll_interval.is_zero();
        let mut interval = tokio::time::interval(if safety_net {
            self.config.poll_interval
        } else {
            Duration::from_secs(3600)
        });

        tracing::info!("Challenge responder started");

        loop {
            tokio::select! {
                // Prefer control commands over event/scan work, so a
                // Pause/Stop queued right after start() is honored first.
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
                event = events_rx.recv(), if events_open => {
                    if matches!(event, Err(broadcast::error::RecvError::Closed)) {
                        events_open = false;
                        continue;
                    }
                    if paused || !self.config.auto_respond {
                        continue;
                    }
                    match event {
                        Ok(BlockEvent::ChallengeCreated { deadline, index, provider, .. }) => {
                            if our_account.as_ref().is_some_and(|me| *me != provider) {
                                continue;
                            }
                            match self.chain_client.fetch_challenge(deadline, index).await {
                                Ok(Some(challenge)) => {
                                    tracing::info!(
                                        "Challenge event for bucket {} (deadline: {}, index: {})",
                                        challenge.bucket_id,
                                        deadline,
                                        index
                                    );
                                    let result = self.respond_to_challenge(&challenge).await;
                                    if let Some(ref cb) = callback {
                                        cb(result);
                                    }
                                }
                                Ok(None) => {}
                                Err(e) => {
                                    tracing::warn!(
                                        "Failed to fetch challenge {deadline}/{index} after event: {e}"
                                    );
                                }
                            }
                        }
                        Ok(BlockEvent::Resubscribed { .. }) | Err(broadcast::error::RecvError::Lagged(_)) => {
                            // Events may have been missed: reconcile with a scan.
                            self.scan_and_respond(&callback).await;
                        }
                        Ok(_) | Err(broadcast::error::RecvError::Closed) => {}
                    }
                }
                _ = interval.tick() => {
                    if paused || !self.config.auto_respond || !safety_net {
                        continue;
                    }
                    self.scan_and_respond(&callback).await;
                }
            }
        }
    }

    /// Full `Challenges` scan; respond to everything targeting this provider.
    async fn scan_and_respond(
        &self,
        callback: &Option<Arc<dyn Fn(ChallengeResponseResult) + Send + Sync>>,
    ) {
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
///
/// Exposed (`#[doc(hidden)]`) only so the fixed-offset layout can be exercised
/// from an integration test against the encoded `Challenge<T>` bytes — it is
/// not part of the crate's stable public API.
#[doc(hidden)]
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
/// `#[doc(hidden)] pub` so the fixed-offset layout is reachable from an
/// integration test; it is an internal helper, not stable public API.
#[doc(hidden)]
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
