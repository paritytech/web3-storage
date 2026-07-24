// SPDX-License-Identifier: GPL-3.0-only

//! Checkpoint Coordinator - Provider-initiated checkpoint coordination.
//!
//! This module provides a background service that coordinates with other
//! providers to autonomously submit checkpoints without requiring the
//! client to be online.

use crate::chain_events::{BlockEvent, BlockEventRx};
use crate::{Error, ProviderState};
use codec::Encode;
use sp_core::{Pair, H256};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;
use storage_primitives::{BucketId, CheckpointProposal};
use tokio::sync::{broadcast, mpsc};

/// Configuration for the checkpoint coordinator.
#[derive(Clone, Debug)]
pub struct CheckpointCoordinatorConfig {
    /// Timeout for collecting signatures from peers.
    pub signature_timeout: Duration,
    /// Whether to automatically submit checkpoints when leader.
    pub auto_submit: bool,
}

impl Default for CheckpointCoordinatorConfig {
    fn default() -> Self {
        Self {
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
#[async_trait::async_trait]
pub trait CheckpointChainClient: Send + Sync {
    /// Get the current block number.
    async fn get_current_block(&self) -> Result<u64, Error>;

    /// Fetch checkpoint config (interval, grace_period) for a bucket.
    /// Returns `None` if no config exists on chain.
    async fn fetch_checkpoint_config(
        &self,
        bucket_id: BucketId,
    ) -> Result<Option<(u32, u32)>, Error>;

    /// Submit a checkpoint transaction with collected signatures.
    async fn submit_checkpoint(
        &self,
        duty: &CheckpointDuty,
        signatures: Vec<(String, String)>,
    ) -> Result<H256, Error>;
}

#[async_trait::async_trait]
impl<T: CheckpointChainClient> CheckpointChainClient for Arc<T> {
    async fn get_current_block(&self) -> Result<u64, Error> {
        self.as_ref().get_current_block().await
    }

    async fn fetch_checkpoint_config(
        &self,
        bucket_id: BucketId,
    ) -> Result<Option<(u32, u32)>, Error> {
        self.as_ref().fetch_checkpoint_config(bucket_id).await
    }

    async fn submit_checkpoint(
        &self,
        duty: &CheckpointDuty,
        signatures: Vec<(String, String)>,
    ) -> Result<H256, Error> {
        self.as_ref().submit_checkpoint(duty, signatures).await
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
    ///
    /// `events_rx` is a subscription to the chain-state coordinator's block
    /// event fan-out; duty checks run once per finalized block (checkpoint
    /// windows are a function of block height, so this is the natural clock).
    pub async fn start(
        self,
        events_rx: BlockEventRx,
        callback: Option<Arc<dyn Fn(CheckpointResult) + Send + Sync>>,
    ) -> Result<CheckpointCoordinatorHandle, Error> {
        let (command_tx, command_rx) = mpsc::channel::<CoordinatorCommand>(32);
        let running = Arc::new(AtomicBool::new(true));
        let running_clone = running.clone();

        let running_exit = running.clone();
        tokio::spawn(async move {
            self.run_loop(command_rx, events_rx, running_clone, callback)
                .await;
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
        mut events_rx: BlockEventRx,
        running: Arc<AtomicBool>,
        callback: Option<Arc<dyn Fn(CheckpointResult) + Send + Sync>>,
    ) {
        let mut paused = false;
        // A closed broadcast channel (follower gone) yields `Closed` on every
        // poll; disarm the events select arm then, or the loop busy-spins.
        // With the arm disarmed only commands remain, matching the pre-event
        // behavior of a coordinator without a chain connection.
        let mut events_open = true;

        tracing::info!("Checkpoint coordinator started");

        loop {
            tokio::select! {
                // Prefer control commands over the poll tick: the interval's
                // first tick fires immediately, so an unbiased select could
                // service a poll before a Pause/Stop queued right after start().
                biased;

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
                event = events_rx.recv(), if events_open => {
                    if matches!(event, Err(broadcast::error::RecvError::Closed)) {
                        events_open = false;
                        continue;
                    }
                    if paused || !self.config.auto_submit {
                        continue;
                    }
                    // Checkpoint windows advance with block height, so a new
                    // finalized block is the duty tick. On a lagged receiver
                    // or (re)subscribe, checking duties once is equally
                    // correct — window state is recomputed from chain state.
                    match event {
                        Ok(BlockEvent::NewBlock { .. })
                        | Ok(BlockEvent::Resubscribed { .. })
                        | Err(broadcast::error::RecvError::Lagged(_)) => {}
                        Ok(_) | Err(broadcast::error::RecvError::Closed) => continue,
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
    pub async fn get_checkpoint_duty(
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

        let anchor_block = self.chain_client.get_current_block().await?;

        let (interval, grace_period) = self
            .chain_client
            .fetch_checkpoint_config(bucket_id)
            .await?
            .unwrap_or((100u32, 20u32));

        let window = if interval > 0 {
            anchor_block / interval as u64
        } else {
            0
        };

        tracing::info!(
            "Checkpoint duty: bucket={} block={} interval={} window={} mmr_root=0x{} leaves={}",
            bucket_id,
            anchor_block,
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
    pub async fn coordinate_checkpoint(&self, duty: &CheckpointDuty) -> CheckpointResult {
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
            mmr_root: format!("0x{}", hex::encode(proposal.commitment.mmr_root.as_bytes())),
            start_seq: proposal.commitment.start_seq,
            leaf_count: proposal.commitment.leaf_count,
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
