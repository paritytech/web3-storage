// SPDX-License-Identifier: Apache-2.0

//! Replica Sync Coordinator - Autonomous replica synchronization service.
//!
//! This module provides a background service that:
//! 1. Reacts to agreement/checkpoint events fanned out by the chain-state
//!    coordinator (with a bootstrap scan on every (re)subscribe and a slow
//!    safety-net interval as backstop)
//! 2. Detects when new data is available to sync
//! 3. Performs top-down MMR traversal to fetch missing data from primaries
//! 4. Submits `confirm_replica_sync` transactions to receive payment
//! 5. Handles historical roots matching for late syncs

use crate::source::SourceSet;
use crate::sync::ReplicaSync;
use crate::sync_roots::{SignedSyncRoots, SyncRootsSigner};
use crate::target::{position_of, select_target, SyncTarget};
use crate::Error;
use provider_events::{BlockEvent, BlockEventRx};
use provider_storage::StorageBackend;
use provider_types::ChainClientError;
use sp_core::H256;
use sp_runtime::AccountId32;
use std::collections::{HashMap, HashSet};
use std::str::FromStr;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, PoisonError};
use std::time::Duration;
use storage_primitives::{BucketId, Commitment};
use tokio::sync::{broadcast, mpsc, oneshot};

/// Configuration for the replica sync coordinator.
#[derive(Clone, Debug)]
pub struct ReplicaSyncCoordinatorConfig {
    /// Safety-net interval between duty reconciliation passes. Duties are
    /// normally discovered event-driven; zero disables the safety net.
    pub poll_interval: Duration,
    /// Timeout for a sync operation (default: 5 minutes).
    pub sync_timeout: Duration,
    /// Maximum concurrent bucket syncs (default: 3).
    pub max_concurrent_syncs: usize,
    /// Whether to automatically submit confirm_replica_sync.
    pub auto_confirm: bool,
}

impl Default for ReplicaSyncCoordinatorConfig {
    fn default() -> Self {
        Self {
            poll_interval: Duration::from_secs(600),
            sync_timeout: Duration::from_secs(300),
            max_concurrent_syncs: 3,
            auto_confirm: true,
        }
    }
}

/// Information about a replica sync duty.
#[derive(Clone, Debug)]
pub struct SyncDuty {
    /// Bucket needing sync.
    pub bucket_id: BucketId,
    /// The commitment to sync to and the position `confirm_replica_sync`
    /// finds it at.
    pub target: SyncTarget,
    /// Bytes the agreement allows the replica to store for the bucket.
    pub max_bytes: u64,
    /// Provider endpoints to sync from, in try order: primaries first, then
    /// the bucket's other replicas (see the `replica_sync` module docs for
    /// why replicas are a source at all).
    pub source_endpoints: Vec<String>,
    /// Available sync balance for this agreement.
    pub sync_balance: u128,
    /// Price per sync operation.
    pub sync_price: u128,
    /// Minimum blocks between syncs.
    pub min_sync_interval: u64,
    /// Last sync info (root, block) if any.
    pub last_sync: Option<(H256, u64)>,
}

/// Result of a replica sync operation.
#[derive(Clone, Debug)]
pub enum SyncResult {
    /// Successfully synced and confirmed on-chain.
    Success {
        bucket_id: BucketId,
        mmr_root: H256,
        position_matched: u8,
        payment: u128,
    },
    /// Sync balance insufficient for payment.
    InsufficientBalance {
        bucket_id: BucketId,
        required: u128,
        available: u128,
    },
    /// Sync interval has not elapsed since last sync.
    SyncIntervalNotElapsed {
        bucket_id: BucketId,
        blocks_remaining: u64,
    },
    /// Every sync source (primaries, then other replicas) failed or refused.
    SourcesUnavailable {
        bucket_id: BucketId,
        tried: Vec<String>,
    },
    /// Local state doesn't match expected root after sync.
    VerificationFailed { bucket_id: BucketId, reason: String },
    /// The local storage engine rejected the sync, for example because the
    /// bucket would exceed the agreement's `max_bytes`.
    StorageFailed { bucket_id: BucketId, error: String },
    /// Failed to submit confirm_replica_sync transaction.
    SubmissionFailed { bucket_id: BucketId, error: String },
    /// Already synced to this root.
    AlreadySynced { bucket_id: BucketId, mmr_root: H256 },
    /// No data to sync yet.
    NoDataToSync { bucket_id: BucketId },
    /// The data sync did not finish within `sync_timeout`. Nodes stored so far
    /// remain, and the next sync of the bucket skips them.
    TimedOut { bucket_id: BucketId },
}

/// The buckets with a sync in progress. Clones share one set.
#[derive(Clone, Debug, Default)]
pub struct SyncingBuckets(Arc<Mutex<HashSet<BucketId>>>);

impl SyncingBuckets {
    /// Whether a sync of `bucket_id` is in progress.
    pub fn is_syncing(&self, bucket_id: BucketId) -> bool {
        self.0
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .contains(&bucket_id)
    }

    /// Mark `bucket_id` as syncing until the returned guard is dropped. Hold
    /// one guard per bucket: dropping any guard clears the mark.
    pub fn begin(&self, bucket_id: BucketId) -> SyncGuard {
        self.0
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .insert(bucket_id);
        SyncGuard {
            buckets: self.clone(),
            bucket_id,
        }
    }
}

/// Keeps a bucket marked as syncing. The mark clears when the guard drops,
/// including when the sync task is aborted.
#[derive(Debug)]
pub struct SyncGuard {
    buckets: SyncingBuckets,
    bucket_id: BucketId,
}

impl Drop for SyncGuard {
    fn drop(&mut self) {
        self.buckets
            .0
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .remove(&self.bucket_id);
    }
}

/// Commands for controlling the coordinator.
#[derive(Debug)]
pub enum SyncCommand {
    /// Stop the coordinator.
    Stop,
    /// Pause automatic syncs. Running syncs stop.
    Pause,
    /// Resume automatic syncs.
    Resume,
    /// Force sync for a specific bucket. Ignores `min_sync_interval`,
    /// `max_concurrent_syncs` and Pause, but not the agreement's expiry or
    /// sync balance.
    ForceSync { bucket_id: BucketId },
    /// Get current status.
    Status {
        response_tx: oneshot::Sender<SyncCoordinatorStatus>,
    },
}

/// Overall coordinator status.
#[derive(Clone, Debug)]
pub struct SyncCoordinatorStatus {
    /// Whether coordinator is running.
    pub running: bool,
    /// Whether coordinator is paused.
    pub paused: bool,
    /// Number of active sync operations.
    pub active_syncs: usize,
    /// Buckets being tracked as replica.
    pub tracked_buckets: Vec<BucketId>,
}

/// Information about a replica agreement from chain.
#[derive(Clone, Debug)]
pub struct ReplicaAgreementInfo {
    pub bucket_id: BucketId,
    pub sync_balance: u128,
    pub sync_price: u128,
    pub min_sync_interval: u64,
    pub last_sync: Option<(H256, u64)>,
    /// Bytes the agreement allows the replica to store for the bucket.
    pub max_bytes: u64,
    /// Anchor block at which the agreement expires.
    pub expires_at: u64,
}

/// The commitments the chain holds for a bucket.
#[derive(Clone, Debug, Default)]
pub struct BucketCommitments {
    /// The latest checkpoint's commitment. All zero until the first checkpoint.
    pub snapshot: Commitment,
    /// The six historical commitments, each with the quotient
    /// `anchor_block / prime` of the checkpoint that wrote it. Entries no
    /// checkpoint has written are all zero.
    pub historical_roots: [(u32, Commitment); 6],
}

/// Trait abstracting chain interactions for the replica sync coordinator.
#[async_trait::async_trait]
pub trait ReplicaSyncChainClient: Send + Sync {
    /// Get the current block number.
    async fn get_current_block(&self) -> Result<u64, ChainClientError>;

    /// Time between two anchor blocks. The coordinator uses it to convert
    /// `sync_timeout` into anchor blocks.
    async fn get_anchor_block_time(&self) -> Result<Duration, ChainClientError>;

    /// Fetch replica agreements for this provider, including those for
    /// buckets it does not store yet.
    async fn fetch_replica_agreements(
        &self,
        provider_account: &str,
    ) -> Result<Vec<ReplicaAgreementInfo>, ChainClientError>;

    /// Fetch the bucket's snapshot and historical commitments from chain. A
    /// bucket without a snapshot returns the default (all zero) commitments.
    async fn fetch_bucket_commitments(
        &self,
        bucket_id: BucketId,
    ) -> Result<BucketCommitments, ChainClientError>;

    /// Fetch primary provider HTTP endpoints for a bucket.
    async fn fetch_primary_endpoints(
        &self,
        bucket_id: BucketId,
    ) -> Result<Vec<String>, ChainClientError>;

    /// Fetch the HTTP endpoints of the bucket's *other* replica providers
    /// (excluding this node).
    async fn fetch_replica_endpoints(
        &self,
        bucket_id: BucketId,
    ) -> Result<Vec<String>, ChainClientError>;

    /// Submit a confirm_replica_sync extrinsic carrying the replica's
    /// signed roots attestation — the pallet verifies it against the
    /// registered `public_key`.
    ///
    /// Returns the matched position and the payment from the `ReplicaSynced`
    /// event, or `None` when the confirmation was already on chain from an
    /// earlier attempt and the event was not observed.
    async fn submit_sync_confirmation(
        &self,
        bucket_id: BucketId,
        attestation: SignedSyncRoots,
    ) -> Result<Option<(u8, u128)>, ChainClientError>;
}

#[async_trait::async_trait]
impl<T: ReplicaSyncChainClient> ReplicaSyncChainClient for Arc<T> {
    async fn get_current_block(&self) -> Result<u64, ChainClientError> {
        self.as_ref().get_current_block().await
    }

    async fn get_anchor_block_time(&self) -> Result<Duration, ChainClientError> {
        self.as_ref().get_anchor_block_time().await
    }

    async fn fetch_replica_agreements(
        &self,
        provider_account: &str,
    ) -> Result<Vec<ReplicaAgreementInfo>, ChainClientError> {
        self.as_ref()
            .fetch_replica_agreements(provider_account)
            .await
    }

    async fn fetch_bucket_commitments(
        &self,
        bucket_id: BucketId,
    ) -> Result<BucketCommitments, ChainClientError> {
        self.as_ref().fetch_bucket_commitments(bucket_id).await
    }

    async fn fetch_primary_endpoints(
        &self,
        bucket_id: BucketId,
    ) -> Result<Vec<String>, ChainClientError> {
        self.as_ref().fetch_primary_endpoints(bucket_id).await
    }

    async fn fetch_replica_endpoints(
        &self,
        bucket_id: BucketId,
    ) -> Result<Vec<String>, ChainClientError> {
        self.as_ref().fetch_replica_endpoints(bucket_id).await
    }

    async fn submit_sync_confirmation(
        &self,
        bucket_id: BucketId,
        attestation: SignedSyncRoots,
    ) -> Result<Option<(u8, u128)>, ChainClientError> {
        self.as_ref()
            .submit_sync_confirmation(bucket_id, attestation)
            .await
    }
}

/// Handle for controlling the replica sync coordinator.
pub struct ReplicaSyncCoordinatorHandle {
    command_tx: mpsc::Sender<SyncCommand>,
    running: Arc<AtomicBool>,
}

impl ReplicaSyncCoordinatorHandle {
    /// Check if the coordinator is running.
    pub fn is_running(&self) -> bool {
        self.running.load(Ordering::SeqCst)
    }

    /// Stop the coordinator.
    pub async fn stop(&self) -> Result<(), Error> {
        self.command_tx
            .send(SyncCommand::Stop)
            .await
            .map_err(|_| Error::ChannelClosed)
    }

    /// Pause automatic syncs. Running syncs stop.
    pub async fn pause(&self) -> Result<(), Error> {
        self.command_tx
            .send(SyncCommand::Pause)
            .await
            .map_err(|_| Error::ChannelClosed)
    }

    /// Resume automatic syncs.
    pub async fn resume(&self) -> Result<(), Error> {
        self.command_tx
            .send(SyncCommand::Resume)
            .await
            .map_err(|_| Error::ChannelClosed)
    }

    /// Force a sync for a specific bucket.
    pub async fn force_sync(&self, bucket_id: BucketId) -> Result<(), Error> {
        self.command_tx
            .send(SyncCommand::ForceSync { bucket_id })
            .await
            .map_err(|_| Error::ChannelClosed)
    }

    /// Get current coordinator status.
    pub async fn status(&self) -> Result<SyncCoordinatorStatus, Error> {
        let (response_tx, response_rx) = oneshot::channel();
        self.command_tx
            .send(SyncCommand::Status { response_tx })
            .await
            .map_err(|_| Error::ChannelClosed)?;

        response_rx.await.map_err(|_| Error::ChannelClosed)
    }
}

/// Replica sync coordinator service.
///
/// Clones share the chain client and storage. Each sync runs as a task on its
/// own clone.
#[derive(Clone)]
pub struct ReplicaSyncCoordinator {
    config: ReplicaSyncCoordinatorConfig,
    storage: Arc<dyn StorageBackend>,
    provider_id: String,
    chain_client: Arc<dyn ReplicaSyncChainClient>,
    replica_sync: Arc<ReplicaSync>,
    /// Signs sync-roots attestations. `None` in provider-id-only setups, where
    /// the coordinator syncs data but cannot confirm on-chain.
    signer: Option<Arc<dyn SyncRootsSigner>>,
    /// The buckets with a sync in progress.
    syncing: SyncingBuckets,
}

impl ReplicaSyncCoordinator {
    /// Create a new replica sync coordinator.
    ///
    /// `signer` attests sync roots. Without one the coordinator still syncs
    /// data but refuses to submit confirmations, since the pallet verifies
    /// the attestation against the registered key.
    pub fn new(
        config: ReplicaSyncCoordinatorConfig,
        storage: Arc<dyn StorageBackend>,
        provider_id: String,
        chain_client: Box<dyn ReplicaSyncChainClient>,
        signer: Option<Arc<dyn SyncRootsSigner>>,
    ) -> Self {
        let replica_sync = Arc::new(ReplicaSync::new(storage.clone()));

        Self {
            config,
            storage,
            provider_id,
            chain_client: Arc::from(chain_client),
            replica_sync,
            signer,
            syncing: SyncingBuckets::default(),
        }
    }

    /// Report the buckets this coordinator syncs in `syncing`, so that other
    /// parts of the node can read them.
    pub fn with_syncing_buckets(mut self, syncing: SyncingBuckets) -> Self {
        self.syncing = syncing;
        self
    }

    /// Start the replica sync coordinator background service.
    ///
    /// `events_rx` is a subscription to the chain-state coordinator's block
    /// event fan-out; duty passes run on relevant agreement/checkpoint
    /// events, on `Resubscribed` / lag, and on the safety-net interval.
    pub async fn start(
        self,
        events_rx: BlockEventRx,
        callback: Option<Arc<dyn Fn(SyncResult) + Send + Sync>>,
    ) -> Result<ReplicaSyncCoordinatorHandle, Error> {
        let (command_tx, command_rx) = mpsc::channel::<SyncCommand>(32);
        let running = Arc::new(AtomicBool::new(true));
        let running_clone = running.clone();

        tokio::spawn(async move {
            self.run_loop(command_rx, events_rx, running_clone, callback)
                .await;
        });

        Ok(ReplicaSyncCoordinatorHandle {
            command_tx,
            running,
        })
    }

    /// Whether a broadcast event can create or advance a sync duty for us.
    ///
    /// Replica duties appear when we get a replica agreement and progress
    /// when a bucket we hold locally is checkpointed. Everything else is
    /// noise for this coordinator.
    fn is_relevant_event(&self, event: &BlockEvent, our_account: &Option<AccountId32>) -> bool {
        match event {
            BlockEvent::ReplicaAgreementEstablished { provider, .. } => {
                our_account.as_ref().is_none_or(|me| me == provider)
            }
            BlockEvent::BucketCheckpointed { bucket_id } => {
                self.storage.get_bucket(*bucket_id).is_some()
            }
            _ => false,
        }
    }

    /// Main coordinator loop.
    async fn run_loop(
        self,
        mut command_rx: mpsc::Receiver<SyncCommand>,
        mut events_rx: BlockEventRx,
        running: Arc<AtomicBool>,
        callback: Option<Arc<dyn Fn(SyncResult) + Send + Sync>>,
    ) {
        let mut paused = false;
        // The sync tasks that were started and have not been cleaned up.
        let mut active_syncs: HashMap<BucketId, tokio::task::JoinHandle<()>> = HashMap::new();
        // A closed broadcast channel (follower gone) yields `Closed` on every
        // poll; disarm the events select arm then, or the loop busy-spins.
        let mut events_open = true;
        let our_account = AccountId32::from_str(&self.provider_id).ok();
        // The safety-net interval's first tick fires immediately, doubling as
        // the startup bootstrap pass (duties accrued while the node was
        // down). With the safety net disabled, the bootstrap pass comes from
        // the follower's `Resubscribed` event on its first connect instead.
        let safety_net = !self.config.poll_interval.is_zero();
        let mut interval = tokio::time::interval(if safety_net {
            self.config.poll_interval
        } else {
            Duration::from_secs(3600)
        });

        tracing::info!("Replica sync coordinator started");

        loop {
            tokio::select! {
                // Prefer control commands over event/scan work, so a
                // Pause/Stop queued right after start() is honored first.
                biased;

                cmd = command_rx.recv() => {
                    match cmd {
                        Some(SyncCommand::Stop) | None => {
                            tracing::info!("Replica sync coordinator stopping");
                            running.store(false, Ordering::SeqCst);
                            for sync in active_syncs.values() {
                                sync.abort();
                            }
                            break;
                        }
                        Some(SyncCommand::Pause) => {
                            tracing::info!("Replica sync coordinator paused");
                            paused = true;
                            // Stored nodes remain, so a sync after Resume
                            // continues where the stopped one was.
                            for sync in active_syncs.values() {
                                sync.abort();
                            }
                        }
                        Some(SyncCommand::Resume) => {
                            tracing::info!("Replica sync coordinator resumed");
                            paused = false;
                        }
                        Some(SyncCommand::ForceSync { bucket_id }) => {
                            tracing::info!("Force sync requested for bucket {bucket_id}");
                            if let Ok(Some(duty)) = self.get_sync_duty(bucket_id).await {
                                self.start_sync(&mut active_syncs, duty, &callback);
                            }
                        }
                        Some(SyncCommand::Status { response_tx }) => {
                            let status = SyncCoordinatorStatus {
                                running: running.load(Ordering::SeqCst),
                                paused,
                                active_syncs: active_syncs
                                    .values()
                                    .filter(|sync| !sync.is_finished())
                                    .count(),
                                tracked_buckets: self.storage.bucket_ids(),
                            };
                            let _ = response_tx.send(status);
                        }
                    }
                }
                // While paused, stop consuming so events stay queued instead of
                // being dropped. Replaying them on resume is safe: a duty pass
                // reconciles against live chain state, so a stale event costs at
                // most one redundant pass. A pause longer than the channel's
                // capacity surfaces as `Lagged` below, which does the same.
                event = events_rx.recv(), if events_open && !paused => {
                    if matches!(event, Err(broadcast::error::RecvError::Closed)) {
                        events_open = false;
                        continue;
                    }
                    // Unlike `paused`, this is permanent config: drain and drop,
                    // since no later state change makes these actionable.
                    if !self.config.auto_confirm {
                        continue;
                    }
                    match event {
                        Ok(BlockEvent::Resubscribed { .. })
                        | Err(broadcast::error::RecvError::Lagged(_)) => {
                            self.run_duty_pass(&mut active_syncs, &callback).await;
                        }
                        Ok(event) if self.is_relevant_event(&event, &our_account) => {
                            self.run_duty_pass(&mut active_syncs, &callback).await;
                        }
                        Ok(_) | Err(broadcast::error::RecvError::Closed) => {}
                    }
                }
                _ = interval.tick() => {
                    if paused || !self.config.auto_confirm || !safety_net {
                        continue;
                    }
                    self.run_duty_pass(&mut active_syncs, &callback).await;
                }
            }
        }
    }

    /// One duty pass: reconcile agreements against local state and start
    /// syncs for every bucket that needs one (bounded by
    /// `max_concurrent_syncs`).
    ///
    /// Runs the full agreement fetch even when triggered by a single-bucket
    /// event: relevant events are rare (new replica agreement, checkpoint on
    /// a held bucket), so this stays off the hot path while keeping exactly
    /// one duty-discovery code path.
    async fn run_duty_pass(
        &self,
        active_syncs: &mut HashMap<BucketId, tokio::task::JoinHandle<()>>,
        callback: &Option<Arc<dyn Fn(SyncResult) + Send + Sync>>,
    ) {
        // Clean up completed syncs
        active_syncs.retain(|_, sync| !sync.is_finished());

        // Get active replica duties
        match self.get_active_replica_duties().await {
            Ok(duties) => {
                for duty in duties {
                    // Skip if already syncing this bucket
                    if active_syncs.contains_key(&duty.bucket_id) {
                        continue;
                    }

                    // Skip if at max concurrent syncs
                    if active_syncs.len() >= self.config.max_concurrent_syncs {
                        break;
                    }

                    self.start_sync(active_syncs, duty, callback);
                }
            }
            Err(e) => {
                tracing::warn!("Failed to get replica duties: {e}");
            }
        }
    }

    /// Run the sync of `duty` as a task, unless the bucket is already syncing.
    /// The callback gets the result when the task ends.
    fn start_sync(
        &self,
        active_syncs: &mut HashMap<BucketId, tokio::task::JoinHandle<()>>,
        duty: SyncDuty,
        callback: &Option<Arc<dyn Fn(SyncResult) + Send + Sync>>,
    ) {
        if active_syncs
            .get(&duty.bucket_id)
            .is_some_and(|sync| !sync.is_finished())
        {
            return;
        }

        tracing::info!(
            "Starting sync for bucket {} (target root: {:#x})",
            duty.bucket_id,
            duty.target.commitment.mmr_root
        );

        let worker = self.clone();
        let callback = callback.clone();
        let guard = self.syncing.begin(duty.bucket_id);
        let bucket_id = duty.bucket_id;
        let sync = tokio::spawn(async move {
            let _guard = guard;
            let result = worker.sync_and_confirm(&duty).await;
            if let Some(callback) = callback {
                callback(result);
            }
        });
        active_syncs.insert(bucket_id, sync);
    }

    /// Get replica duties for buckets where this provider is a replica.
    ///
    /// Creates the local bucket of every replica agreement that has not
    /// expired, even before the chain has a commitment to sync to. A checkpoint
    /// of a bucket the node stores starts the next duty pass, so the first
    /// checkpoint reaches a new replica. A new bucket gets the agreement's
    /// quota; a sync sets the quota of an existing one.
    pub async fn get_active_replica_duties(&self) -> Result<Vec<SyncDuty>, Error> {
        let anchor_block = self.chain_client.get_current_block().await?;
        let agreements = self
            .chain_client
            .fetch_replica_agreements(&self.provider_id)
            .await?;
        let timeout_blocks = self.timeout_blocks().await?;

        let mut duties = Vec::new();
        for agreement in agreements {
            let bucket_id = agreement.bucket_id;
            // A running sync keeps its bucket, so it needs no duty.
            if self.syncing.is_syncing(bucket_id) {
                continue;
            }
            // One failing agreement must not hide the duties of the others.
            match self
                .duty_for(agreement, anchor_block, timeout_blocks, true)
                .await
            {
                Ok(Some(duty)) => duties.push(duty),
                Ok(None) => {}
                Err(e) => tracing::warn!("Skipping bucket {bucket_id}: {e}"),
            }
        }
        Ok(duties)
    }

    /// The anchor blocks a sync may take: `sync_timeout` in anchor blocks.
    /// The confirmation transaction after the sync is not included, so an
    /// agreement that expires within a few blocks of this margin can still
    /// end with a rejected confirmation.
    async fn timeout_blocks(&self) -> Result<u64, Error> {
        let anchor_block_time = self.chain_client.get_anchor_block_time().await?;
        let blocks = self
            .config
            .sync_timeout
            .as_millis()
            .div_ceil(anchor_block_time.as_millis().max(1));
        Ok(u64::try_from(blocks).unwrap_or(u64::MAX))
    }

    /// The duty for `agreement`, or `None` when the replica cannot or need not
    /// sync now. `check_interval` false ignores `min_sync_interval`, for a
    /// forced sync.
    async fn duty_for(
        &self,
        agreement: ReplicaAgreementInfo,
        anchor_block: u64,
        timeout_blocks: u64,
        check_interval: bool,
    ) -> Result<Option<SyncDuty>, Error> {
        // `confirm_replica_sync` rejects a call at or after `expires_at`.
        if anchor_block.saturating_add(timeout_blocks) >= agreement.expires_at {
            tracing::debug!(
                "Bucket {} agreement expires at block {} before a sync could finish",
                agreement.bucket_id,
                agreement.expires_at
            );
            return Ok(None);
        }

        // Create the bucket even when no sync starts now, so a checkpoint of
        // it is relevant. Only create it: a sync task may be writing the
        // bucket, and the task sets the quota itself.
        self.storage
            .init_bucket(agreement.bucket_id, agreement.max_bytes)?;

        if agreement.sync_balance < agreement.sync_price {
            tracing::debug!(
                "Bucket {} has insufficient sync balance: {} < {}",
                agreement.bucket_id,
                agreement.sync_balance,
                agreement.sync_price
            );
            return Ok(None);
        }

        if check_interval {
            if let Some((_, last_block)) = agreement.last_sync {
                let elapsed = anchor_block.saturating_sub(last_block);
                if elapsed < agreement.min_sync_interval {
                    tracing::debug!(
                        "Bucket {} sync interval not elapsed: {} < {}",
                        agreement.bucket_id,
                        elapsed,
                        agreement.min_sync_interval
                    );
                    return Ok(None);
                }
            }
        }

        let commitments = self
            .chain_client
            .fetch_bucket_commitments(agreement.bucket_id)
            .await?;
        let last_sync_root = agreement.last_sync.map(|(root, _)| root);
        let Some(target) = select_target(&commitments, last_sync_root) else {
            return Ok(None);
        };

        let source_endpoints = self.fetch_source_endpoints(agreement.bucket_id).await?;

        Ok(Some(SyncDuty {
            bucket_id: agreement.bucket_id,
            target,
            max_bytes: agreement.max_bytes,
            source_endpoints,
            sync_balance: agreement.sync_balance,
            sync_price: agreement.sync_price,
            min_sync_interval: agreement.min_sync_interval,
            last_sync: agreement.last_sync,
        }))
    }

    /// Create the local bucket if it does not exist and set its quota.
    /// `init_bucket` keeps the quota of a bucket that exists, so the quota is
    /// set afterwards.
    fn prepare_bucket(&self, bucket_id: BucketId, max_bytes: u64) -> Result<(), Error> {
        self.storage.init_bucket(bucket_id, max_bytes)?;
        self.storage.set_max_bytes(bucket_id, max_bytes)?;
        Ok(())
    }

    /// Endpoints to sync from, in try order: primaries first (most current),
    /// then the bucket's other replicas. A failure to list replicas only
    /// shrinks the fallback set, so it degrades to primaries-only.
    async fn fetch_source_endpoints(&self, bucket_id: BucketId) -> Result<Vec<String>, Error> {
        let mut sources = self.chain_client.fetch_primary_endpoints(bucket_id).await?;
        match self.chain_client.fetch_replica_endpoints(bucket_id).await {
            Ok(replicas) => {
                for url in replicas {
                    if !sources.contains(&url) {
                        sources.push(url);
                    }
                }
            }
            Err(e) => {
                tracing::warn!("Failed to fetch replica endpoints for bucket {bucket_id}: {e}")
            }
        }
        Ok(sources)
    }

    /// Get sync duty for a specific bucket, or `None` if this provider has no
    /// replica agreement for it or cannot sync it now. Ignores
    /// `min_sync_interval`.
    async fn get_sync_duty(&self, bucket_id: BucketId) -> Result<Option<SyncDuty>, Error> {
        let anchor_block = self.chain_client.get_current_block().await?;
        let agreements = self
            .chain_client
            .fetch_replica_agreements(&self.provider_id)
            .await?;
        let Some(agreement) = agreements
            .into_iter()
            .find(|agreement| agreement.bucket_id == bucket_id)
        else {
            return Ok(None);
        };
        let timeout_blocks = self.timeout_blocks().await?;
        self.duty_for(agreement, anchor_block, timeout_blocks, false)
            .await
    }

    /// Perform sync and submit confirmation.
    pub async fn sync_and_confirm(&self, duty: &SyncDuty) -> SyncResult {
        let target_root = duty.target.commitment.mmr_root;

        // Check sync balance
        if duty.sync_balance < duty.sync_price {
            return SyncResult::InsufficientBalance {
                bucket_id: duty.bucket_id,
                required: duty.sync_price,
                available: duty.sync_balance,
            };
        }

        // No data to sync if target is zero
        if target_root == H256::zero() {
            return SyncResult::NoDataToSync {
                bucket_id: duty.bucket_id,
            };
        }

        // The agreement's quota limits what the replica stores.
        if let Err(e) = self.prepare_bucket(duty.bucket_id, duty.max_bytes) {
            return SyncResult::StorageFailed {
                bucket_id: duty.bucket_id,
                error: format!("cannot prepare the bucket: {e}"),
            };
        }

        let mut sources = SourceSet::new(duty.source_endpoints.clone());
        let synced = tokio::time::timeout(
            self.config.sync_timeout,
            self.replica_sync
                .sync_to_target(duty.bucket_id, &duty.target.commitment, &mut sources),
        )
        .await;
        match synced {
            Err(_) => {
                return SyncResult::TimedOut {
                    bucket_id: duty.bucket_id,
                };
            }
            Ok(Ok(())) => tracing::info!(
                "Synced bucket {} to root {:#x}",
                duty.bucket_id,
                target_root
            ),
            Ok(Err(Error::NoSourceLeft { .. })) => {
                return SyncResult::SourcesUnavailable {
                    bucket_id: duty.bucket_id,
                    tried: duty.source_endpoints.clone(),
                };
            }
            Ok(Err(Error::Backend(provider_storage::Error::CommitmentNotReproducible(reason)))) => {
                return SyncResult::VerificationFailed {
                    bucket_id: duty.bucket_id,
                    reason,
                };
            }
            Ok(Err(Error::Backend(e))) => {
                return SyncResult::StorageFailed {
                    bucket_id: duty.bucket_id,
                    error: e.to_string(),
                };
            }
            Ok(Err(e)) => {
                return SyncResult::VerificationFailed {
                    bucket_id: duty.bucket_id,
                    reason: e.to_string(),
                };
            }
        }

        // Submit on-chain confirmation if auto_confirm is enabled
        if self.config.auto_confirm {
            self.confirm_on_chain(duty).await
        } else {
            SyncResult::Success {
                bucket_id: duty.bucket_id,
                mmr_root: target_root,
                position_matched: duty.target.position,
                payment: 0,
            }
        }
    }

    /// Attest the synced roots and submit `confirm_replica_sync`. The pallet
    /// verifies the signature over the SCALE-encoded `roots` array against
    /// our registered public key, so a node without a signing key must not
    /// submit at all.
    pub async fn confirm_on_chain(&self, duty: &SyncDuty) -> SyncResult {
        let Some(signer) = self.signer.as_deref() else {
            return SyncResult::SubmissionFailed {
                bucket_id: duty.bucket_id,
                error: "no signing key configured; cannot attest sync roots".to_string(),
            };
        };
        let target_root = duty.target.commitment.mmr_root;

        // A checkpoint during the sync can move the root to another position
        // or overwrite it, so look it up again.
        let position = match self
            .chain_client
            .fetch_bucket_commitments(duty.bucket_id)
            .await
        {
            Ok(commitments) => position_of(&commitments, target_root),
            Err(e) => {
                return SyncResult::SubmissionFailed {
                    bucket_id: duty.bucket_id,
                    error: e.to_string(),
                };
            }
        };
        let Some(position) = position else {
            return SyncResult::SubmissionFailed {
                bucket_id: duty.bucket_id,
                error: format!("root {target_root:#x} is no longer on chain"),
            };
        };
        let attestation = match SignedSyncRoots::sign(signer, position, target_root) {
            Ok(attestation) => attestation,
            Err(e) => {
                return SyncResult::SubmissionFailed {
                    bucket_id: duty.bucket_id,
                    error: format!("cannot attest sync roots: {e}"),
                };
            }
        };
        match self
            .chain_client
            .submit_sync_confirmation(duty.bucket_id, attestation)
            .await
        {
            Ok(Some((position, payment))) => SyncResult::Success {
                bucket_id: duty.bucket_id,
                mmr_root: target_root,
                position_matched: position,
                payment,
            },
            Ok(None) => SyncResult::AlreadySynced {
                bucket_id: duty.bucket_id,
                mmr_root: target_root,
            },
            Err(e) => SyncResult::SubmissionFailed {
                bucket_id: duty.bucket_id,
                error: e.to_string(),
            },
        }
    }
}
