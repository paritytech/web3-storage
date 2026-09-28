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

use provider_chain::{BlockEvent, BlockEventRx};
use sp_core::H256;
use sp_runtime::AccountId32;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;
use storage_primitives::{BucketId, MerkleProof, MmrProof};
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
/// [`ChallengeError::is_retryable`] and [`ChallengeError::risks_slashing`]
/// let a caller act on that meaning without matching every variant.
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
    /// The chain could not be reached, or the connection dropped mid-call.
    /// Nothing is known about whether the call took effect. Retryable.
    #[error("chain unreachable: {detail}")]
    ChainUnavailable {
        /// The transport failure, as text.
        detail: String,
    },
    /// The chain was reached and rejected the call. Resubmitting the same
    /// call will fail identically.
    #[error("chain rejected the call: {detail}")]
    ChainRejected {
        /// The chain's rejection, as text.
        detail: String,
    },
    /// The responder is shutting down; its control channel is closed.
    #[error("challenge responder is shutting down")]
    Shutdown,
}

impl ChallengeError {
    /// Whether repeating the operation could plausibly succeed.
    pub fn is_retryable(&self) -> bool {
        matches!(
            self,
            Self::StorageUnavailable { .. } | Self::ChainUnavailable { .. }
        )
    }

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
    /// responded / reaped) or targets another provider. Backs the
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
                // resolved is a no-op. A pause longer than the channel's
                // capacity surfaces as `Lagged` below, which reconciles with a
                // full scan.
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

        // Step 1: Generate MMR proof (includes the leaf with data_root)
        let mmr_proof = match self
            .proof_source
            .get_mmr_proof(challenge.bucket_id, challenge.leaf_index)
        {
            Ok(proof) => proof,
            Err(e) => {
                log_proof_failure(challenge, "Failed to generate MMR proof", &e);
                if e.risks_slashing() {
                    return ChallengeResponseResult::DataNotFound {
                        challenge_id,
                        bucket_id: challenge.bucket_id,
                        leaf_index: challenge.leaf_index,
                    };
                }
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
                log_proof_failure(challenge, "Failed to get chunk data", &e);
                if e.risks_slashing() {
                    return ChallengeResponseResult::DataNotFound {
                        challenge_id,
                        bucket_id: challenge.bucket_id,
                        leaf_index: challenge.leaf_index,
                    };
                }
                return ChallengeResponseResult::ProofGenerationFailed {
                    challenge_id,
                    error: e.to_string(),
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

/// Log a proof-gathering failure with the fields an operator needs to act on
/// it, at a level that matches what the failure means: [`ChallengeError::risks_slashing`]
/// failures are an emergency (the stake is on the line at `deadline`
/// regardless of which step failed), everything else is a routine failure
/// the safety-net scan will retry.
fn log_proof_failure(challenge: &DetectedChallenge, what: &str, e: &ChallengeError) {
    if e.risks_slashing() {
        tracing::error!(
            bucket_id = challenge.bucket_id,
            leaf_index = challenge.leaf_index,
            chunk_index = challenge.chunk_index,
            deadline = challenge.deadline,
            "{what}: {e}"
        );
    } else {
        tracing::warn!(
            bucket_id = challenge.bucket_id,
            leaf_index = challenge.leaf_index,
            chunk_index = challenge.chunk_index,
            deadline = challenge.deadline,
            "{what}: {e}"
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;
    use std::sync::atomic::AtomicUsize;
    use std::sync::Mutex;
    use storage_primitives::MmrLeaf;

    const CHUNK: &[u8] = b"chunk bytes";

    fn me() -> AccountId32 {
        AccountId32::new([1u8; 32])
    }

    fn someone_else() -> AccountId32 {
        AccountId32::new([2u8; 32])
    }

    fn data_root() -> H256 {
        H256::repeat_byte(0xda)
    }

    fn response_block() -> H256 {
        H256::repeat_byte(0xbb)
    }

    fn challenge(deadline: u32, index: u16, bucket_id: BucketId) -> DetectedChallenge {
        DetectedChallenge {
            bucket_id,
            deadline,
            index,
            mmr_root: H256::repeat_byte(0x11),
            start_seq: 0,
            leaf_index: 7,
            chunk_index: 3,
            challenger: "challenger".to_string(),
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
        mmr_calls: Mutex<Vec<(BucketId, u64)>>,
        chunk_calls: Mutex<Vec<(H256, u64)>>,
    }

    impl MockProofSource {
        fn new(mmr: Lookup, chunk: Lookup) -> Arc<Self> {
            Arc::new(Self {
                mmr,
                chunk,
                mmr_calls: Mutex::new(Vec::new()),
                chunk_calls: Mutex::new(Vec::new()),
            })
        }

        fn serving_everything() -> Arc<Self> {
            Self::new(Lookup::Found, Lookup::Found)
        }

        fn chunk_calls(&self) -> Vec<(H256, u64)> {
            self.chunk_calls.lock().expect("lock").clone()
        }
    }

    impl ChallengeProofSource for MockProofSource {
        fn get_mmr_proof(
            &self,
            bucket_id: BucketId,
            leaf_index: u64,
        ) -> Result<MmrProof, ChallengeError> {
            self.mmr_calls
                .lock()
                .expect("lock")
                .push((bucket_id, leaf_index));
            let target = ProofTarget::MmrLeaf {
                bucket_id,
                leaf_index,
            };
            match self.mmr {
                Lookup::Found => Ok(MmrProof {
                    peaks: vec![H256::repeat_byte(0x22)],
                    leaf: MmrLeaf {
                        data_root: data_root(),
                        data_size: 1024,
                        total_size: 1024,
                    },
                    leaf_proof: MerkleProof {
                        siblings: vec![],
                        path: vec![],
                    },
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
        ) -> Result<(Vec<u8>, MerkleProof), ChallengeError> {
            self.chunk_calls
                .lock()
                .expect("lock")
                .push((data_root, chunk_index));
            let target = ProofTarget::Chunk {
                data_root,
                chunk_index,
            };
            match self.chunk {
                Lookup::Found => Ok((
                    CHUNK.to_vec(),
                    MerkleProof {
                        siblings: vec![H256::repeat_byte(0x33)],
                        path: vec![true],
                    },
                )),
                Lookup::Missing => Err(ChallengeError::ProofDataMissing { target }),
                Lookup::BackendFailed => Err(ChallengeError::StorageUnavailable {
                    target,
                    detail: "chunk file unreadable".to_string(),
                }),
            }
        }
    }

    /// What one `submit_response` call carried, as the chain saw it.
    #[derive(Clone, Debug, PartialEq, Eq)]
    struct Submission {
        challenge_id: (u32, u16),
        chunk_data: Vec<u8>,
        /// The root the submitted MMR leaf commits to.
        data_root: H256,
    }

    struct MockChainClient {
        /// Returned by every full scan.
        scan: Vec<DetectedChallenge>,
        /// Point-reads that resolve; anything else reads as gone.
        fetchable: HashMap<(u32, u16), DetectedChallenge>,
        accepts_response: bool,
        poll_calls: AtomicUsize,
        fetch_calls: Mutex<Vec<(u32, u16)>>,
        submissions: Mutex<Vec<Submission>>,
    }

    impl MockChainClient {
        fn new() -> Self {
            Self {
                scan: Vec::new(),
                fetchable: HashMap::new(),
                accepts_response: true,
                poll_calls: AtomicUsize::new(0),
                fetch_calls: Mutex::new(Vec::new()),
                submissions: Mutex::new(Vec::new()),
            }
        }

        fn scanning(mut self, challenges: Vec<DetectedChallenge>) -> Self {
            self.scan = challenges;
            self
        }

        fn serving(mut self, challenges: Vec<DetectedChallenge>) -> Self {
            self.fetchable = challenges
                .into_iter()
                .map(|c| ((c.deadline, c.index), c))
                .collect();
            self
        }

        fn rejecting_responses(mut self) -> Self {
            self.accepts_response = false;
            self
        }

        fn poll_calls(&self) -> usize {
            self.poll_calls.load(Ordering::SeqCst)
        }

        fn fetch_calls(&self) -> Vec<(u32, u16)> {
            self.fetch_calls.lock().expect("lock").clone()
        }

        fn submissions(&self) -> Vec<Submission> {
            self.submissions.lock().expect("lock").clone()
        }
    }

    #[async_trait::async_trait]
    impl ChallengeChainClient for MockChainClient {
        async fn poll_challenges(&self) -> Result<Vec<DetectedChallenge>, ChallengeError> {
            self.poll_calls.fetch_add(1, Ordering::SeqCst);
            Ok(self.scan.clone())
        }

        async fn fetch_challenge(
            &self,
            deadline: u32,
            index: u16,
        ) -> Result<Option<DetectedChallenge>, ChallengeError> {
            self.fetch_calls
                .lock()
                .expect("lock")
                .push((deadline, index));
            Ok(self.fetchable.get(&(deadline, index)).cloned())
        }

        async fn submit_response(
            &self,
            challenge_id: (u32, u16),
            chunk_data: Vec<u8>,
            mmr_proof: MmrProof,
            _chunk_proof: MerkleProof,
        ) -> Result<H256, ChallengeError> {
            self.submissions.lock().expect("lock").push(Submission {
                challenge_id,
                chunk_data,
                data_root: mmr_proof.leaf.data_root,
            });
            if self.accepts_response {
                Ok(response_block())
            } else {
                Err(ChallengeError::ChainRejected {
                    detail: "proof did not verify".to_string(),
                })
            }
        }
    }

    fn config() -> ChallengeResponderConfig {
        ChallengeResponderConfig {
            // Safety net off by default: tests that want scans either drive
            // them through events or set an interval explicitly.
            poll_interval: Duration::ZERO,
            ..ChallengeResponderConfig::new(me())
        }
    }

    fn responder(
        config: ChallengeResponderConfig,
        proofs: Arc<MockProofSource>,
        chain: Arc<MockChainClient>,
    ) -> ChallengeResponder {
        ChallengeResponder::new(config, proofs, Box::new(chain))
    }

    /// A started responder plus the ends of every channel a test drives it
    /// through.
    struct Harness {
        handle: ChallengeResponderHandle,
        events_tx: broadcast::Sender<BlockEvent>,
        results: mpsc::UnboundedReceiver<ChallengeResponseResult>,
    }

    impl Harness {
        async fn start(
            config: ChallengeResponderConfig,
            proofs: Arc<MockProofSource>,
            chain: Arc<MockChainClient>,
            event_capacity: usize,
        ) -> Self {
            let (events_tx, events_rx) = broadcast::channel(event_capacity);
            let (results_tx, results) = mpsc::unbounded_channel();
            let handle = responder(config, proofs, chain)
                .start(
                    events_rx,
                    Some(Arc::new(move |result| {
                        let _ = results_tx.send(result);
                    })),
                )
                .await
                .expect("responder starts");
            Self {
                handle,
                events_tx,
                results,
            }
        }

        fn challenge_created(&self, c: &DetectedChallenge, provider: AccountId32) {
            self.events_tx
                .send(BlockEvent::ChallengeCreated {
                    deadline: c.deadline,
                    index: c.index,
                    bucket_id: c.bucket_id,
                    provider,
                })
                .expect("responder is subscribed");
        }

        async fn next_result(&mut self) -> ChallengeResponseResult {
            tokio::time::timeout(Duration::from_secs(1), self.results.recv())
                .await
                .expect("responder produced a result")
                .expect("callback channel stays open")
        }

        /// Drop the responder's only event sender, closing its event channel.
        fn close_event_channel(&mut self) {
            self.events_tx = broadcast::channel(1).0;
        }

        /// Let the responder run itself out and assert it responded to
        /// nothing. Time is paused in these tests, so it only advances once
        /// every task is idle - an elapsed timeout therefore also proves the
        /// loop parked rather than busy-spun.
        async fn assert_idle_with_no_response(&mut self) {
            let unexpected =
                tokio::time::timeout(Duration::from_millis(50), self.results.recv()).await;
            assert!(
                unexpected.is_err(),
                "expected no challenge response, got {:?}",
                unexpected.ok()
            );
        }
    }

    // ── respond_to_challenge: what each failure means for the operator ──

    #[tokio::test]
    async fn response_proves_the_chunk_under_the_data_root_named_by_the_mmr_leaf() {
        let c = challenge(100, 0, 5);
        let proofs = MockProofSource::serving_everything();
        let chain = Arc::new(MockChainClient::new());
        let responder = responder(config(), proofs.clone(), chain.clone());

        let result = responder.respond_to_challenge(&c).await;

        assert!(
            matches!(
                result,
                ChallengeResponseResult::Success { challenge_id, block_hash }
                    if challenge_id == (100, 0) && block_hash == response_block()
            ),
            "unexpected result: {result:?}"
        );
        // The chunk lookup must use the data root the MMR leaf commits to -
        // proving a chunk under any other root answers a different question
        // than the one the chain asked.
        assert_eq!(proofs.chunk_calls(), vec![(data_root(), c.chunk_index)]);
        assert_eq!(
            chain.submissions(),
            vec![Submission {
                challenge_id: (100, 0),
                chunk_data: CHUNK.to_vec(),
                data_root: data_root(),
            }]
        );
    }

    #[tokio::test]
    async fn missing_mmr_leaf_reports_data_not_found_and_submits_nothing() {
        let c = challenge(100, 1, 5);
        let proofs = MockProofSource::new(Lookup::Missing, Lookup::Found);
        let chain = Arc::new(MockChainClient::new());
        let responder = responder(config(), proofs.clone(), chain.clone());

        let result = responder.respond_to_challenge(&c).await;

        assert!(
            matches!(
                result,
                ChallengeResponseResult::DataNotFound { challenge_id, bucket_id, leaf_index }
                    if challenge_id == (100, 1) && bucket_id == 5 && leaf_index == c.leaf_index
            ),
            "unexpected result: {result:?}"
        );
        assert!(proofs.chunk_calls().is_empty());
        assert!(chain.submissions().is_empty());
    }

    #[tokio::test]
    async fn missing_chunk_reports_data_not_found_rather_than_a_retryable_failure() {
        // The MMR leaf is intact but the chunk under it is gone: still the
        // slashing case, and reporting it as retryable would have the operator
        // wait for a scan that can never succeed.
        let c = challenge(100, 2, 5);
        let proofs = MockProofSource::new(Lookup::Found, Lookup::Missing);
        let chain = Arc::new(MockChainClient::new());
        let responder = responder(config(), proofs, chain.clone());

        let result = responder.respond_to_challenge(&c).await;

        assert!(
            matches!(
                result,
                ChallengeResponseResult::DataNotFound { challenge_id, bucket_id, leaf_index }
                    if challenge_id == (100, 2) && bucket_id == 5 && leaf_index == c.leaf_index
            ),
            "unexpected result: {result:?}"
        );
        assert!(chain.submissions().is_empty());
    }

    #[tokio::test]
    async fn storage_backend_failure_reports_proof_generation_failed_not_data_loss() {
        let c = challenge(100, 3, 5);
        let proofs = MockProofSource::new(Lookup::BackendFailed, Lookup::Found);
        let chain = Arc::new(MockChainClient::new());
        let responder = responder(config(), proofs, chain.clone());

        let result = responder.respond_to_challenge(&c).await;

        assert!(
            matches!(
                result,
                ChallengeResponseResult::ProofGenerationFailed { challenge_id, ref error }
                    if challenge_id == (100, 3) && error.contains("storage backend unavailable")
            ),
            "unexpected result: {result:?}"
        );
        assert!(chain.submissions().is_empty());
    }

    #[tokio::test]
    async fn chain_rejection_reports_submission_failed() {
        let c = challenge(100, 4, 5);
        let chain = Arc::new(MockChainClient::new().rejecting_responses());
        let responder = responder(
            config(),
            MockProofSource::serving_everything(),
            chain.clone(),
        );

        let result = responder.respond_to_challenge(&c).await;

        assert!(
            matches!(
                result,
                ChallengeResponseResult::SubmissionFailed { challenge_id, ref error }
                    if challenge_id == (100, 4) && error.contains("chain rejected")
            ),
            "unexpected result: {result:?}"
        );
    }

    // ── run loop: which challenges reach respond_to_challenge, and when ──

    #[tokio::test]
    async fn challenge_event_for_another_provider_is_ignored() {
        let theirs = challenge(200, 0, 5);
        let ours = challenge(201, 0, 6);
        let chain = Arc::new(MockChainClient::new().serving(vec![theirs.clone(), ours.clone()]));
        let mut h = Harness::start(
            config(),
            MockProofSource::serving_everything(),
            chain.clone(),
            16,
        )
        .await;

        h.challenge_created(&theirs, someone_else());
        h.challenge_created(&ours, me());

        // Events are handled in order, so the first response arriving for our
        // own challenge means the other provider's was skipped, not merely slow.
        let result = h.next_result().await;
        assert!(
            matches!(result, ChallengeResponseResult::Success { challenge_id, .. } if challenge_id == (201, 0)),
            "unexpected result: {result:?}"
        );
        assert_eq!(chain.fetch_calls(), vec![(201, 0)]);
    }

    #[tokio::test]
    async fn challenge_that_is_already_gone_produces_no_response() {
        let gone = challenge(300, 0, 5);
        let live = challenge(301, 0, 6);
        // Only `live` is point-readable: `gone` was already answered or reaped.
        let chain = Arc::new(MockChainClient::new().serving(vec![live.clone()]));
        let mut h = Harness::start(
            config(),
            MockProofSource::serving_everything(),
            chain.clone(),
            16,
        )
        .await;

        h.challenge_created(&gone, me());
        h.challenge_created(&live, me());

        let result = h.next_result().await;
        assert!(
            matches!(result, ChallengeResponseResult::Success { challenge_id, .. } if challenge_id == (301, 0)),
            "unexpected result: {result:?}"
        );
        assert_eq!(chain.fetch_calls(), vec![(300, 0), (301, 0)]);
    }

    #[tokio::test(start_paused = true)]
    async fn auto_respond_disabled_drops_events_without_touching_the_chain() {
        let ours = challenge(400, 0, 5);
        let chain = Arc::new(MockChainClient::new().serving(vec![ours.clone()]));
        let config = ChallengeResponderConfig {
            auto_respond: false,
            ..config()
        };
        let mut h = Harness::start(
            config,
            MockProofSource::serving_everything(),
            chain.clone(),
            16,
        )
        .await;

        h.challenge_created(&ours, me());

        h.assert_idle_with_no_response().await;
        assert!(chain.fetch_calls().is_empty());
    }

    #[tokio::test]
    async fn resubscribing_reconciles_with_a_full_scan() {
        // A resubscription means events may have been missed while the
        // follower was down, so every open challenge has to be re-derived
        // from chain state rather than from the event stream.
        let first = challenge(500, 0, 5);
        let second = challenge(500, 1, 6);
        let chain = Arc::new(MockChainClient::new().scanning(vec![first, second]));
        let mut h = Harness::start(
            config(),
            MockProofSource::serving_everything(),
            chain.clone(),
            16,
        )
        .await;

        h.events_tx
            .send(BlockEvent::Resubscribed { at_block: 42 })
            .expect("responder is subscribed");

        for expected in [(500u32, 0u16), (500, 1)] {
            let result = h.next_result().await;
            assert!(
                matches!(result, ChallengeResponseResult::Success { challenge_id, .. } if challenge_id == expected),
                "unexpected result: {result:?}"
            );
        }
        assert_eq!(chain.poll_calls(), 1);
        assert!(chain.fetch_calls().is_empty());
    }

    #[tokio::test(start_paused = true)]
    async fn pausing_queues_events_and_resuming_answers_them() {
        // Dropping events while paused would leave the provider silently
        // liable for a challenge it never saw, so they must survive the pause.
        let ours = challenge(600, 0, 5);
        let chain = Arc::new(MockChainClient::new().serving(vec![ours.clone()]));
        let mut h = Harness::start(
            config(),
            MockProofSource::serving_everything(),
            chain.clone(),
            16,
        )
        .await;

        h.handle.pause().await.expect("pause is accepted");
        h.assert_idle_with_no_response().await;

        h.challenge_created(&ours, me());
        h.assert_idle_with_no_response().await;
        assert!(chain.fetch_calls().is_empty(), "event handled while paused");

        h.handle.resume().await.expect("resume is accepted");

        let result = h.next_result().await;
        assert!(
            matches!(result, ChallengeResponseResult::Success { challenge_id, .. } if challenge_id == (600, 0)),
            "unexpected result: {result:?}"
        );
    }

    #[tokio::test(start_paused = true)]
    async fn overflowing_the_event_channel_reconciles_with_a_full_scan() {
        // Events lost to a lagging receiver are indistinguishable from events
        // that never arrived, so the responder falls back to chain state.
        let missed_one = challenge(700, 0, 5);
        let missed_two = challenge(700, 1, 5);
        let found_by_scan = challenge(700, 2, 5);
        let chain = Arc::new(MockChainClient::new().scanning(vec![found_by_scan]));
        let mut h = Harness::start(
            config(),
            MockProofSource::serving_everything(),
            chain.clone(),
            // One slot, so a second queued event evicts the first.
            1,
        )
        .await;

        h.handle.pause().await.expect("pause is accepted");
        h.assert_idle_with_no_response().await;
        h.challenge_created(&missed_one, me());
        h.challenge_created(&missed_two, me());
        h.handle.resume().await.expect("resume is accepted");

        let result = h.next_result().await;
        assert!(
            matches!(result, ChallengeResponseResult::Success { challenge_id, .. } if challenge_id == (700, 2)),
            "unexpected result: {result:?}"
        );
        assert_eq!(chain.poll_calls(), 1);
    }

    #[tokio::test(start_paused = true)]
    async fn safety_net_scans_at_startup_and_then_every_interval() {
        // The startup scan is what catches challenges raised while the node
        // was down; the interval is what catches a silently broken event path.
        let interval = Duration::from_secs(300);
        let chain = Arc::new(MockChainClient::new().scanning(vec![challenge(800, 0, 5)]));
        let config = ChallengeResponderConfig {
            poll_interval: interval,
            ..config()
        };
        let mut h = Harness::start(
            config,
            MockProofSource::serving_everything(),
            chain.clone(),
            16,
        )
        .await;

        h.next_result().await;
        assert_eq!(chain.poll_calls(), 1, "no scan at startup");

        tokio::time::sleep(interval + Duration::from_secs(1)).await;

        h.next_result().await;
        assert_eq!(chain.poll_calls(), 2, "no scan on the safety-net interval");
    }

    #[tokio::test(start_paused = true)]
    async fn a_zero_interval_disables_the_safety_net_scan() {
        let chain = Arc::new(MockChainClient::new().scanning(vec![challenge(900, 0, 5)]));
        let mut h = Harness::start(
            config(),
            MockProofSource::serving_everything(),
            chain.clone(),
            16,
        )
        .await;

        tokio::time::sleep(Duration::from_secs(24 * 60 * 60)).await;

        h.assert_idle_with_no_response().await;
        assert_eq!(chain.poll_calls(), 0);
    }

    #[tokio::test]
    async fn a_closed_event_channel_does_not_starve_the_safety_net_scan() {
        // A closed broadcast channel reports `Closed` on every poll. If that
        // arm stays armed it is always ready, and `biased` select then starves
        // the safety-net scan while spinning at 100% CPU - i.e. the one
        // remaining way to notice a challenge stops running. Real time here
        // rather than paused time: under paused time a spinning loop never
        // lets the clock advance, so the regression would hang the test
        // instead of failing it.
        let interval = Duration::from_millis(20);
        let chain = Arc::new(MockChainClient::new().scanning(vec![challenge(1000, 0, 5)]));
        let config = ChallengeResponderConfig {
            poll_interval: interval,
            ..config()
        };
        let mut h = Harness::start(
            config,
            MockProofSource::serving_everything(),
            chain.clone(),
            16,
        )
        .await;

        h.next_result().await;
        h.close_event_channel();

        // Scans keep coming after the channel closed.
        h.next_result().await;
        h.next_result().await;

        assert!(h.handle.is_running());
        h.handle.stop().await.expect("stop is accepted");
        for _ in 0..100 {
            if !h.handle.is_running() {
                return;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        panic!("stop did not end the loop");
    }
}
