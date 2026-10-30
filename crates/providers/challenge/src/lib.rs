// SPDX-License-Identifier: Apache-2.0

//! Challenge Responder - Automated response to on-chain challenges.
//!
//! This crate provides a background service that reacts to
//! `ChallengeCreated` events (fanned out by the chain-state coordinator)
//! against this provider and automatically responds with the required proof
//! data. A full `Challenges` scan runs at startup and on every stream
//! (re)subscription to catch challenges raised while the node was down, plus
//! on a slow safety-net interval — a missed challenge means getting slashed,
//! so the event path is backstopped rather than trusted blindly.

use provider_events::{BlockEvent, BlockEventRx};
use sp_core::H256;
use sp_runtime::AccountId32;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;
use storage_primitives::{BucketId, ChunkLocation, Commitment, MmrProof};
use tokio::sync::{broadcast, mpsc};

/// What proof data a failure was about.
///
/// `get_mmr_proof` and `get_chunk_at_index` identify what they're looking
/// for differently (a bucket and leaf index vs. a data root and chunk
/// index), so a single error payload can't share fields across both without
/// fields that don't apply to one side or the other.
#[derive(Clone, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub enum ProofTarget {
    /// A leaf of a bucket's MMR commitment.
    MmrLeaf {
        bucket_id: BucketId,
        leaf_index: u64,
    },
    /// A chunk under a data root.
    Chunk { data_root: H256, chunk_index: u64 },
}

/// Errors surfaced by the challenge responder.
///
/// Each variant names one condition and implies one response to it, rather
/// than only naming which layer the failure came from.
/// [`ChallengeError::risks_slashing`] lets a caller act on that meaning
/// without matching every variant.
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum ChallengeError {
    /// The requested proof data is provably absent from local storage. The
    /// challenge cannot be answered; unless the data is restored before the
    /// deadline, the provider's entire stake is slashed. Retrying will not
    /// help.
    #[error("proof data is provably missing: {target:?}")]
    ProofDataMissing {
        /// What could not be found.
        target: ProofTarget,
    },
    /// The storage backend itself failed - a database read errored, or a
    /// stored record could not be decoded. The proof data may still be
    /// present. Operator action: inspect, restart, or restore the backend,
    /// then retry.
    #[error("storage backend unavailable for {target:?}: {detail}")]
    StorageUnavailable {
        /// What was being looked up when the backend failed.
        target: ProofTarget,
        /// The backend's own error, as text.
        detail: String,
    },
    /// A chain call failed: the chain was unreachable, rejected the call, or
    /// returned a value that could not be decoded.
    #[error("chain call failed: {detail}")]
    Chain {
        /// The failure, as text.
        detail: String,
    },
    /// The responder is shutting down; its control channel is closed.
    #[error("challenge responder is shutting down")]
    Shutdown,
}

impl ChallengeError {
    /// Whether this failure, left unresolved, leads to the provider's stake
    /// being slashed at the challenge deadline.
    pub fn risks_slashing(&self) -> bool {
        matches!(self, Self::ProofDataMissing { .. })
    }
}

/// Local proof data the responder needs to answer a challenge.
///
/// Backed by the provider node's storage backend; kept narrow so this crate
/// stays decoupled from the full storage engine.
pub trait ChallengeProofSource: Send + Sync {
    /// Generate an MMR proof for `leaf_index` against the exact cited
    /// commitment state (which later commits or prunes may have moved past).
    fn get_mmr_proof_for_commitment(
        &self,
        bucket_id: BucketId,
        commitment: &Commitment,
        leaf_index: u64,
    ) -> Result<MmrProof, ChallengeError>;

    /// Fetch a chunk and its Merkle proof (sibling hashes up to the data root)
    /// under the given data root.
    fn get_chunk_at_index(
        &self,
        data_root: H256,
        chunk_index: u64,
    ) -> Result<(Vec<u8>, Vec<H256>), ChallengeError>;
}

/// Configuration for the challenge responder.
#[derive(Clone, Debug)]
pub struct ChallengeResponderConfig {
    /// Account this responder acts for; used to filter `ChallengeCreated`
    /// events down to our own challenges. Validated by the caller before the
    /// responder starts.
    pub provider_account: AccountId32,
    /// Safety-net interval between full `Challenges` reconciliation scans.
    /// Challenges are normally handled event-driven; zero disables the scan.
    pub poll_interval: Duration,
    /// Maximum time to spend gathering proof data.
    pub proof_timeout: Duration,
    /// Whether to automatically respond to challenges.
    pub auto_respond: bool,
}

impl ChallengeResponderConfig {
    /// Config for `provider_account` with the default poll interval, proof
    /// timeout and auto-respond setting.
    pub fn new(provider_account: AccountId32) -> Self {
        Self {
            provider_account,
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
    /// The commitment being challenged; on-chain proof verification is bound
    /// to the exact `(target.leaf_index, commitment.leaf_count)` position.
    pub commitment: Commitment,
    /// Leaf + chunk to prove.
    pub target: ChunkLocation,
}

/// Result of responding to a challenge.
#[derive(Clone, Debug)]
pub enum ChallengeResponseResult {
    /// Successfully submitted response.
    Success {
        challenge_id: (u32, u16),
        block_hash: H256,
    },
    /// Gathering proof data failed for a reason other than the data being
    /// provably absent (e.g. the storage backend itself errored). Retryable -
    /// the next safety-net scan will try again.
    ProofGenerationFailed {
        challenge_id: (u32, u16),
        error: String,
    },
    /// Failed to submit response transaction.
    SubmissionFailed {
        challenge_id: (u32, u16),
        error: String,
    },
    /// The proof data is provably absent from local storage. Unless it is
    /// restored before the deadline, the provider's stake is slashed.
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

    /// Point-read a single challenge by id, `None` if it is gone (already
    /// responded / resolved) or targets another provider. Backs the
    /// event-driven path, where `ChallengeCreated` carries the id but not
    /// the proof parameters.
    async fn fetch_challenge(
        &self,
        deadline: u32,
        index: u16,
    ) -> Result<Option<DetectedChallenge>, ChallengeError>;

    /// Submit a challenge response transaction.
    async fn submit_response(
        &self,
        challenge_id: (u32, u16),
        response: ChallengeResponse,
    ) -> Result<H256, ChallengeError>;
}

/// A response the responder submits for a challenge.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ChallengeResponse {
    /// The challenged chunk, the leaf's MMR proof and the chunk's Merkle proof
    /// under the leaf's `data_root`.
    Proof {
        chunk_data: Vec<u8>,
        mmr_proof: MmrProof,
        chunk_proof: Vec<H256>,
    },
    /// The challenged `chunk_index` is at or beyond the leaf's chunk count, so
    /// only the leaf's MMR proof goes on chain.
    ChunkOutOfRange { mmr_proof: MmrProof },
}

#[async_trait::async_trait]
impl<T: ChallengeChainClient> ChallengeChainClient for Arc<T> {
    async fn poll_challenges(&self) -> Result<Vec<DetectedChallenge>, ChallengeError> {
        self.as_ref().poll_challenges().await
    }

    async fn fetch_challenge(
        &self,
        deadline: u32,
        index: u16,
    ) -> Result<Option<DetectedChallenge>, ChallengeError> {
        self.as_ref().fetch_challenge(deadline, index).await
    }

    async fn submit_response(
        &self,
        challenge_id: (u32, u16),
        response: ChallengeResponse,
    ) -> Result<H256, ChallengeError> {
        self.as_ref().submit_response(challenge_id, response).await
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
            .map_err(|_| ChallengeError::Shutdown)
    }

    /// Pause automatic responses.
    pub async fn pause(&self) -> Result<(), ChallengeError> {
        self.command_tx
            .send(ResponderCommand::Pause)
            .await
            .map_err(|_| ChallengeError::Shutdown)
    }

    /// Resume automatic responses.
    pub async fn resume(&self) -> Result<(), ChallengeError> {
        self.command_tx
            .send(ResponderCommand::Resume)
            .await
            .map_err(|_| ChallengeError::Shutdown)
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
    ///
    /// `events_rx` is a subscription to the chain-state coordinator's block
    /// event fan-out; the responder reacts to `ChallengeCreated` events and
    /// reconciles with a full scan on `Resubscribed` / lag / the safety-net
    /// interval.
    pub async fn start(
        self,
        events_rx: BlockEventRx,
        callback: Option<Arc<dyn Fn(ChallengeResponseResult) + Send + Sync>>,
    ) -> Result<ChallengeResponderHandle, ChallengeError> {
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
        mut events_rx: BlockEventRx,
        running: Arc<AtomicBool>,
        callback: Option<Arc<dyn Fn(ChallengeResponseResult) + Send + Sync>>,
    ) {
        let mut paused = false;
        // A closed broadcast channel (follower gone) yields `Closed` on every
        // poll; disarm the events select arm then, or the loop busy-spins.
        let mut events_open = true;
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
                // While paused, stop consuming so events stay queued instead of
                // being dropped. Replaying them on resume is safe: each one is
                // point-read against live chain state, so anything already
                // resolved is a no-op; an expired, unresolved one costs one
                // rejected response (`ChallengeExpired`). A pause longer than
                // the channel's capacity surfaces as `Lagged` below, which
                // reconciles with a full scan.
                event = events_rx.recv(), if events_open && !paused => {
                    if matches!(event, Err(broadcast::error::RecvError::Closed)) {
                        events_open = false;
                        continue;
                    }
                    // Unlike `paused`, this is permanent config: drain and drop,
                    // since no later state change makes these actionable.
                    if !self.config.auto_respond {
                        continue;
                    }
                    match event {
                        Ok(BlockEvent::ChallengeCreated { deadline, index, provider, .. }) => {
                            // Only challenges against our own account are actionable.
                            if self.config.provider_account != provider {
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

        // Step 1: Generate MMR proof (includes the leaf with data_root).
        // The challenged leaf_index is relative to the cited commitment's
        // start_seq, and the cited root may predate later commits or prunes,
        // so the proof is rebuilt for that exact commitment state.
        let mmr_proof = match self.proof_source.get_mmr_proof_for_commitment(
            challenge.bucket_id,
            &challenge.commitment,
            challenge.target.leaf_index,
        ) {
            Ok(proof) => proof,
            Err(e) => return proof_failure(challenge, "Failed to generate MMR proof", &e),
        };

        // Step 2: a chunk index at or past the leaf's chunk count has no chunk
        // to prove, so the leaf's MMR proof alone answers it. Otherwise fetch
        // the chunk and its Merkle proof under the leaf's data_root.
        let Some(chunk_count) = mmr_proof.leaf.chunk_count() else {
            let error = format!("unknown leaf version {}", mmr_proof.leaf.leaf_version);
            tracing::error!("Failed to answer challenge: {}", error);
            return ChallengeResponseResult::ProofGenerationFailed {
                challenge_id,
                error,
            };
        };
        let response = if challenge.target.chunk_index >= chunk_count {
            ChallengeResponse::ChunkOutOfRange { mmr_proof }
        } else {
            let (chunk_data, chunk_proof) = match self
                .proof_source
                .get_chunk_at_index(mmr_proof.leaf.data_root, challenge.target.chunk_index)
            {
                Ok(data) => data,
                Err(e) => return proof_failure(challenge, "Failed to get chunk data", &e),
            };
            ChallengeResponse::Proof {
                chunk_data,
                mmr_proof,
                chunk_proof,
            }
        };

        // Step 3: submit and report the outcome
        match self
            .chain_client
            .submit_response(challenge_id, response)
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

/// Log a proof-gathering failure and map it to the result to report, at a
/// level that matches what the failure means: [`ChallengeError::risks_slashing`]
/// failures are an emergency (the stake is on the line at `deadline`
/// regardless of which step failed), everything else is a routine failure
/// the safety-net scan will retry.
fn proof_failure(
    challenge: &DetectedChallenge,
    what: &str,
    e: &ChallengeError,
) -> ChallengeResponseResult {
    let challenge_id = (challenge.deadline, challenge.index);
    macro_rules! log {
        ($level:ident) => {
            tracing::$level!(
                bucket_id = challenge.bucket_id,
                leaf_index = challenge.target.leaf_index,
                chunk_index = challenge.target.chunk_index,
                deadline = challenge.deadline,
                "{what}: {e}"
            )
        };
    }
    if e.risks_slashing() {
        log!(error);
        ChallengeResponseResult::DataNotFound {
            challenge_id,
            bucket_id: challenge.bucket_id,
            leaf_index: challenge.target.leaf_index,
        }
    } else {
        log!(warn);
        ChallengeResponseResult::ProofGenerationFailed {
            challenge_id,
            error: e.to_string(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;
    use storage_primitives::{MmrLeaf, DEFAULT_CHUNK_SIZE, LEAF_VERSION};

    fn data_root() -> H256 {
        H256::repeat_byte(0xda)
    }

    fn challenge(index: u16) -> DetectedChallenge {
        DetectedChallenge {
            bucket_id: 5,
            deadline: 100,
            index,
            commitment: Commitment {
                mmr_root: H256::repeat_byte(0x11),
                start_seq: 0,
                leaf_count: 8,
            },
            target: ChunkLocation {
                leaf_index: 7,
                chunk_index: 3,
            },
        }
    }

    /// What a proof lookup does when the responder asks for it.
    #[derive(Clone, Copy)]
    enum Lookup {
        /// The data is there.
        Found,
        /// The data is provably absent - the slashing case.
        Missing,
        /// The backend itself failed; the data may well still be there.
        BackendFailed,
    }

    struct MockProofSource {
        mmr: Lookup,
        chunk: Lookup,
    }

    impl ChallengeProofSource for MockProofSource {
        fn get_mmr_proof_for_commitment(
            &self,
            bucket_id: BucketId,
            _commitment: &Commitment,
            leaf_index: u64,
        ) -> Result<MmrProof, ChallengeError> {
            let target = ProofTarget::MmrLeaf {
                bucket_id,
                leaf_index,
            };
            match self.mmr {
                // Four chunks, so the challenged chunk index 3 is in range.
                Lookup::Found => Ok(MmrProof {
                    leaf: MmrLeaf {
                        leaf_version: LEAF_VERSION,
                        data_root: data_root(),
                        data_size: 4 * u64::from(DEFAULT_CHUNK_SIZE),
                        total_size: 4 * u64::from(DEFAULT_CHUNK_SIZE),
                    },
                    items: vec![H256::repeat_byte(0x22)],
                }),
                Lookup::Missing => Err(ChallengeError::ProofDataMissing { target }),
                Lookup::BackendFailed => Err(ChallengeError::StorageUnavailable {
                    target,
                    detail: "rocksdb read failed".to_string(),
                }),
            }
        }

        fn get_chunk_at_index(
            &self,
            data_root: H256,
            chunk_index: u64,
        ) -> Result<(Vec<u8>, Vec<H256>), ChallengeError> {
            let target = ProofTarget::Chunk {
                data_root,
                chunk_index,
            };
            match self.chunk {
                Lookup::Found => Ok((b"chunk bytes".to_vec(), vec![H256::repeat_byte(0x33)])),
                Lookup::Missing => Err(ChallengeError::ProofDataMissing { target }),
                Lookup::BackendFailed => Err(ChallengeError::StorageUnavailable {
                    target,
                    detail: "chunk file unreadable".to_string(),
                }),
            }
        }
    }

    /// Records every submitted response; the chain accepts all of them.
    #[derive(Default)]
    struct MockChainClient {
        submitted: Mutex<Vec<(u32, u16)>>,
    }

    #[async_trait::async_trait]
    impl ChallengeChainClient for MockChainClient {
        async fn poll_challenges(&self) -> Result<Vec<DetectedChallenge>, ChallengeError> {
            Ok(Vec::new())
        }

        async fn fetch_challenge(
            &self,
            _deadline: u32,
            _index: u16,
        ) -> Result<Option<DetectedChallenge>, ChallengeError> {
            Ok(None)
        }

        async fn submit_response(
            &self,
            challenge_id: (u32, u16),
            _response: ChallengeResponse,
        ) -> Result<H256, ChallengeError> {
            self.submitted.lock().expect("lock").push(challenge_id);
            Ok(H256::repeat_byte(0xbb))
        }
    }

    /// Runs `respond_to_challenge` against `mmr` and `chunk` lookups, and
    /// returns the result with the responses the chain received.
    async fn respond(
        mmr: Lookup,
        chunk: Lookup,
        c: &DetectedChallenge,
    ) -> (ChallengeResponseResult, Vec<(u32, u16)>) {
        let chain = Arc::new(MockChainClient::default());
        let responder = ChallengeResponder::new(
            ChallengeResponderConfig::new(AccountId32::new([1u8; 32])),
            Arc::new(MockProofSource { mmr, chunk }),
            Box::new(chain.clone()),
        );
        let result = responder.respond_to_challenge(c).await;
        let submitted = chain.submitted.lock().expect("lock").clone();
        (result, submitted)
    }

    #[tokio::test]
    async fn missing_mmr_leaf_reports_data_not_found_and_submits_nothing() {
        let c = challenge(1);

        let (result, submitted) = respond(Lookup::Missing, Lookup::Found, &c).await;

        assert!(
            matches!(
                result,
                ChallengeResponseResult::DataNotFound { challenge_id, bucket_id, leaf_index }
                    if challenge_id == (100, 1) && bucket_id == 5 && leaf_index == c.target.leaf_index
            ),
            "unexpected result: {result:?}"
        );
        assert!(submitted.is_empty());
    }

    #[tokio::test]
    async fn missing_chunk_reports_data_not_found_rather_than_a_retryable_failure() {
        // The MMR leaf is intact but the chunk under it is gone: still the
        // slashing case, and reporting it as retryable would have the operator
        // wait for a scan that can never succeed.
        let c = challenge(2);

        let (result, submitted) = respond(Lookup::Found, Lookup::Missing, &c).await;

        assert!(
            matches!(
                result,
                ChallengeResponseResult::DataNotFound { challenge_id, bucket_id, leaf_index }
                    if challenge_id == (100, 2) && bucket_id == 5 && leaf_index == c.target.leaf_index
            ),
            "unexpected result: {result:?}"
        );
        assert!(submitted.is_empty());
    }

    #[tokio::test]
    async fn storage_backend_failure_reports_proof_generation_failed_not_data_loss() {
        let c = challenge(3);

        let (result, submitted) = respond(Lookup::BackendFailed, Lookup::Found, &c).await;

        assert!(
            matches!(
                result,
                ChallengeResponseResult::ProofGenerationFailed { challenge_id, ref error }
                    if challenge_id == (100, 3) && error.contains("storage backend unavailable")
            ),
            "unexpected result: {result:?}"
        );
        assert!(submitted.is_empty());
    }
}
