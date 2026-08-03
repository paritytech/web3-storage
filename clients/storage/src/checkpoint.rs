// SPDX-License-Identifier: Apache-2.0

//! Checkpoint Manager for automated multi-provider checkpoint coordination.
//!
//! This module provides a reusable checkpoint management system that can be
//! used by any Layer 1 interface (File System, Database, etc.) to handle
//! the complexity of multi-provider signature collection and checkpoint submission.
//!
//! # Example
//!
//! ```ignore
//! use storage_client::checkpoint::{CheckpointManager, CheckpointConfig};
//!
//! # async fn example() -> Result<(), Box<dyn std::error::Error>> {
//! // Create manager with default config
//! let manager = CheckpointManager::new(
//!     "ws://localhost:2222",
//!     CheckpointConfig::default(),
//! ).await?;
//!
//! // Add provider endpoints
//! let manager = manager.with_provider("http://localhost:3333");
//!
//! // Submit checkpoint for a bucket
//! let bucket_id = 1u64;
//! let result = manager.submit_checkpoint(bucket_id).await;
//! # Ok(())
//! # }
//! ```

use crate::challenger::{ChallengeId, ChallengerClient};
use crate::checkpoint_persistence::{
    CheckpointPersistence, PersistedCheckpointState, PersistenceConfig, StateBuilder,
};
use crate::convert;
use crate::substrate::{extrinsics, SubstrateClient};
use crate::{ClientError, CommitmentResponse};
use sp_core::H256;
use sp_runtime::AccountId32;
use std::collections::{HashMap, VecDeque};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};
use storage_primitives::{BucketId, ChunkLocation, Commitment};
use storage_subxt::api;
use tokio::sync::{mpsc, RwLock};

// ============================================================================
// Configuration
// ============================================================================

/// Configuration for the Checkpoint Manager.
#[derive(Clone, Debug)]
pub struct CheckpointConfig {
    /// Maximum time to wait for provider responses.
    pub provider_timeout: Duration,
    /// Number of retries for failed provider queries.
    pub max_retries: u32,
    /// Base delay between retries (exponential backoff).
    pub retry_delay: Duration,
    /// Minimum percentage of providers that must agree (0-100).
    pub consensus_threshold_percent: u8,
    /// How long to cache provider info before refreshing.
    pub provider_cache_ttl: Duration,
}

impl Default for CheckpointConfig {
    fn default() -> Self {
        Self {
            provider_timeout: Duration::from_secs(30),
            max_retries: 3,
            retry_delay: Duration::from_secs(2),
            consensus_threshold_percent: 51,
            provider_cache_ttl: Duration::from_secs(300), // 5 minutes
        }
    }
}

// ============================================================================
// Provider Types
// ============================================================================

/// Information about a storage provider.
#[derive(Clone, Debug)]
pub struct ProviderInfo {
    /// Provider's account ID.
    pub account_id: AccountId32,
    /// HTTP endpoint for the provider.
    pub endpoint: String,
    /// Provider's public key for signature verification.
    pub public_key: Vec<u8>,
    /// Last time we successfully contacted this provider.
    pub last_seen: Option<Instant>,
    /// Current health status.
    pub status: ProviderStatus,
}

/// Health status of a provider.
#[derive(Clone, Debug, PartialEq)]
pub enum ProviderStatus {
    /// Provider is responding normally.
    Healthy,
    /// Provider is responding but with issues.
    Degraded { last_error: String },
    /// Provider is not responding.
    Unreachable { since: Instant },
    /// Status unknown (not yet checked).
    Unknown,
}

/// Provider health history for tracking reliability over time.
#[derive(Clone, Debug)]
pub struct ProviderHealthHistory {
    /// Provider account ID.
    pub account_id: AccountId32,
    /// Total number of requests made.
    pub total_requests: u64,
    /// Number of successful requests.
    pub successful_requests: u64,
    /// Number of failed requests.
    pub failed_requests: u64,
    /// Average response time in milliseconds.
    pub avg_response_time_ms: u64,
    /// Last N status changes for trend analysis.
    pub recent_statuses: VecDeque<(Instant, ProviderStatus)>,
    /// Last successful contact time.
    pub last_success: Option<Instant>,
    /// Last failure time.
    pub last_failure: Option<Instant>,
    /// Current consecutive failures.
    pub consecutive_failures: u32,
}

impl ProviderHealthHistory {
    /// Create a new health history.
    pub fn new(account_id: AccountId32) -> Self {
        Self {
            account_id,
            total_requests: 0,
            successful_requests: 0,
            failed_requests: 0,
            avg_response_time_ms: 0,
            recent_statuses: VecDeque::new(),
            last_success: None,
            last_failure: None,
            consecutive_failures: 0,
        }
    }

    /// Record a successful request.
    pub fn record_success(&mut self, response_time_ms: u64) {
        self.total_requests += 1;
        self.successful_requests += 1;
        self.consecutive_failures = 0;
        self.last_success = Some(Instant::now());

        // Update average response time
        let total = self.successful_requests;
        self.avg_response_time_ms =
            (self.avg_response_time_ms * (total - 1) + response_time_ms) / total;

        // Track status change
        self.add_status(ProviderStatus::Healthy);
    }

    /// Record a failed request.
    pub fn record_failure(&mut self, error: String) {
        self.total_requests += 1;
        self.failed_requests += 1;
        self.consecutive_failures += 1;
        self.last_failure = Some(Instant::now());

        // Track status change
        self.add_status(ProviderStatus::Degraded { last_error: error });
    }

    /// Add a status to the history (keep last 10).
    fn add_status(&mut self, status: ProviderStatus) {
        self.recent_statuses.push_back((Instant::now(), status));
        if self.recent_statuses.len() > 10 {
            self.recent_statuses.pop_front();
        }
    }

    /// Calculate success rate (0.0 to 1.0).
    pub fn success_rate(&self) -> f64 {
        if self.total_requests == 0 {
            return 1.0;
        }
        self.successful_requests as f64 / self.total_requests as f64
    }

    /// Check if provider is considered healthy (success rate > 80%, no recent failures).
    pub fn is_healthy(&self) -> bool {
        self.success_rate() > 0.8 && self.consecutive_failures < 3
    }

    /// Get current status based on history.
    pub fn current_status(&self) -> ProviderStatus {
        if self.consecutive_failures >= 5 {
            ProviderStatus::Unreachable {
                since: self.last_failure.unwrap_or_else(Instant::now),
            }
        } else if self.consecutive_failures > 0 || self.success_rate() < 0.8 {
            ProviderStatus::Degraded {
                last_error: format!(
                    "{} consecutive failures, {:.0}% success rate",
                    self.consecutive_failures,
                    self.success_rate() * 100.0
                ),
            }
        } else if self.total_requests == 0 {
            ProviderStatus::Unknown
        } else {
            ProviderStatus::Healthy
        }
    }
}

// ============================================================================
// Result Types
// ============================================================================

/// Result of collecting commitments from providers.
#[derive(Clone, Debug)]
pub struct CommitmentCollection {
    /// Bucket ID.
    pub bucket_id: BucketId,
    /// Majority MMR root (most providers agree on this).
    pub mmr_root: H256,
    /// Start sequence number.
    pub start_seq: u64,
    /// Number of leaves in the MMR.
    pub leaf_count: u64,
    /// The shared `CommitmentPayload` nonce every signature in `signatures`
    /// was made over. The pallet verifies all signatures against one encoded
    /// `(bucket_id, commitment, nonce)` payload.
    pub nonce: u64,
    /// Signatures from agreeing providers: (account_id, signature_bytes).
    /// Only providers whose response matched the modal
    /// `(start_seq, leaf_count, nonce)` payload are included.
    pub signatures: Vec<(AccountId32, Vec<u8>)>,
    /// Providers that agreed on the majority root.
    pub agreeing_providers: Vec<AccountId32>,
    /// Providers with different roots: (account_id, their_root).
    pub disagreeing_providers: Vec<(AccountId32, H256)>,
    /// Providers that couldn't be reached.
    pub unreachable_providers: Vec<AccountId32>,
}

/// Length of an sr25519 signature; the only scheme providers sign with.
const SR25519_SIGNATURE_LEN: usize = 64;

/// The exact `CommitmentPayload` a provider signature covers.
///
/// The pallet re-encodes one of these and verifies *every* submitted
/// signature against it, so signatures over differing payloads cannot be
/// batched into a single `checkpoint` call.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
struct PayloadKey {
    start_seq: u64,
    leaf_count: u64,
    nonce: u64,
}

impl PayloadKey {
    const fn new(start_seq: u64, leaf_count: u64, nonce: u64) -> Self {
        Self {
            start_seq,
            leaf_count,
            nonce,
        }
    }
}

/// Signatures grouped by the payload they cover.
type SigsByPayload = HashMap<PayloadKey, Vec<(AccountId32, Vec<u8>)>>;

/// Pick the payload sub-group carrying the most signatures.
///
/// Since the nonce is client-chosen and validated on ingestion, groups here
/// differ only in `(start_seq, leaf_count)` - i.e. providers disagreeing about
/// the committed range. Ties break toward the larger `leaf_count` (never
/// reward a provider for claiming a shorter, less challengeable range), then
/// `start_seq`, so the choice never depends on `HashMap` iteration order.
fn modal_payload_group(groups: SigsByPayload) -> (PayloadKey, Vec<(AccountId32, Vec<u8>)>) {
    groups
        .into_iter()
        .max_by_key(|(key, sigs)| (sigs.len(), key.leaf_count, key.start_seq))
        .unwrap_or_default()
}

/// Detected conflict between providers.
#[derive(Clone, Debug)]
pub struct ProviderConflict {
    /// Bucket where conflict was detected.
    pub bucket_id: BucketId,
    /// The majority MMR root (what most providers agree on).
    pub majority_root: H256,
    /// Number of providers agreeing on majority.
    pub majority_count: usize,
    /// Conflicting providers with their different roots.
    pub conflicts: Vec<ConflictingProvider>,
    /// When the conflict was detected.
    pub detected_at: Instant,
    /// Possible resolution strategy.
    pub resolution: ConflictResolution,
}

/// A provider that disagrees with the majority.
#[derive(Clone, Debug)]
pub struct ConflictingProvider {
    /// Provider account ID.
    pub account_id: AccountId32,
    /// Their MMR root (different from majority).
    pub mmr_root: H256,
    /// Their leaf count.
    pub leaf_count: u64,
    /// Whether they're behind (likely sync delay) or divergent (data corruption).
    pub conflict_type: ConflictType,
}

/// Type of conflict detected.
#[derive(Clone, Debug, PartialEq)]
pub enum ConflictType {
    /// Provider is behind (lower leaf count) - likely sync delay.
    SyncDelay { behind_by: u64 },
    /// Provider has same leaf count but different root - data divergence.
    DataDivergence,
    /// Provider is ahead of majority - unusual.
    Ahead { ahead_by: u64 },
}

/// Suggested resolution for a conflict.
#[derive(Clone, Debug, PartialEq)]
pub enum ConflictResolution {
    /// Wait for sync and retry.
    WaitForSync { estimated_blocks: u32 },
    /// Proceed with majority (above threshold).
    ProceedWithMajority,
    /// Manual intervention required.
    ManualIntervention { reason: String },
    /// Consider challenging the provider.
    ConsiderChallenge { provider: AccountId32 },
}

// ============================================================================
// Background Checkpoint Loop Types
// ============================================================================

/// Configuration for background batched checkpoints.
#[derive(Clone, Debug)]
pub struct BatchedCheckpointConfig {
    /// Interval between checkpoint submissions (in blocks or duration).
    pub interval: BatchedInterval,
    /// Whether to submit checkpoint even if no changes detected.
    pub submit_on_empty: bool,
    /// Maximum number of consecutive failures before pausing.
    pub max_consecutive_failures: u32,
    /// Delay after failure before retrying.
    pub failure_retry_delay: Duration,
}

impl Default for BatchedCheckpointConfig {
    fn default() -> Self {
        Self {
            interval: BatchedInterval::Blocks(100),
            submit_on_empty: false,
            max_consecutive_failures: 5,
            failure_retry_delay: Duration::from_secs(30),
        }
    }
}

/// Interval specification for batched checkpoints.
#[derive(Clone, Debug)]
pub enum BatchedInterval {
    /// Number of blocks between checkpoints.
    Blocks(u32),
    /// Time duration between checkpoints.
    Duration(Duration),
}

/// Message for controlling the background checkpoint loop.
#[derive(Debug)]
pub enum CheckpointLoopCommand {
    /// Submit a checkpoint immediately.
    SubmitNow,
    /// Mark that changes have occurred for a bucket.
    MarkDirty(BucketId),
    /// Pause the checkpoint loop.
    Pause,
    /// Resume the checkpoint loop.
    Resume,
    /// Stop the checkpoint loop.
    Stop,
}

/// Status of a bucket in the checkpoint loop.
#[derive(Clone, Debug, Default)]
pub struct BucketCheckpointStatus {
    /// Whether the bucket has pending changes.
    pub dirty: bool,
    /// Last successful checkpoint time.
    pub last_checkpoint: Option<Instant>,
    /// Last checkpoint result.
    pub last_result: Option<CheckpointResult>,
    /// Number of consecutive failures.
    pub consecutive_failures: u32,
}

/// Handle for controlling a running checkpoint loop.
pub struct CheckpointLoopHandle {
    /// Channel for sending commands to the loop.
    command_tx: mpsc::Sender<CheckpointLoopCommand>,
    /// Flag indicating if the loop is running.
    running: Arc<AtomicBool>,
    /// Handle to the background task.
    task_handle: Option<tokio::task::JoinHandle<()>>,
}

impl CheckpointLoopHandle {
    /// Create a new handle.
    fn new(
        command_tx: mpsc::Sender<CheckpointLoopCommand>,
        running: Arc<AtomicBool>,
        task_handle: tokio::task::JoinHandle<()>,
    ) -> Self {
        Self {
            command_tx,
            running,
            task_handle: Some(task_handle),
        }
    }

    /// Check if the loop is still running.
    pub fn is_running(&self) -> bool {
        self.running.load(Ordering::SeqCst)
    }

    /// Request immediate checkpoint submission.
    pub async fn submit_now(&self) -> Result<(), ClientError> {
        self.command_tx
            .send(CheckpointLoopCommand::SubmitNow)
            .await
            .map_err(|_| ClientError::Chain("Checkpoint loop not running".to_string()))
    }

    /// Mark a bucket as dirty (has pending changes).
    pub async fn mark_dirty(&self, bucket_id: BucketId) -> Result<(), ClientError> {
        self.command_tx
            .send(CheckpointLoopCommand::MarkDirty(bucket_id))
            .await
            .map_err(|_| ClientError::Chain("Checkpoint loop not running".to_string()))
    }

    /// Pause the checkpoint loop.
    pub async fn pause(&self) -> Result<(), ClientError> {
        self.command_tx
            .send(CheckpointLoopCommand::Pause)
            .await
            .map_err(|_| ClientError::Chain("Checkpoint loop not running".to_string()))
    }

    /// Resume the checkpoint loop.
    pub async fn resume(&self) -> Result<(), ClientError> {
        self.command_tx
            .send(CheckpointLoopCommand::Resume)
            .await
            .map_err(|_| ClientError::Chain("Checkpoint loop not running".to_string()))
    }

    /// Stop the checkpoint loop.
    pub async fn stop(&mut self) -> Result<(), ClientError> {
        let _ = self.command_tx.send(CheckpointLoopCommand::Stop).await;
        self.running.store(false, Ordering::SeqCst);

        if let Some(handle) = self.task_handle.take() {
            let _ = handle.await;
        }
        Ok(())
    }
}

/// Callback type for checkpoint completion events.
pub type CheckpointCallback = Arc<dyn Fn(BucketId, &CheckpointResult) + Send + Sync>;

// ============================================================================
// Phase 3: Metrics & Auto-Challenge Types
// ============================================================================

/// Metrics for checkpoint operations.
#[derive(Clone, Debug, Default)]
pub struct CheckpointMetrics {
    /// Total checkpoints attempted.
    pub total_attempts: u64,
    /// Successful checkpoints submitted.
    pub successful_submissions: u64,
    /// Checkpoints failed due to insufficient consensus.
    pub insufficient_consensus_count: u64,
    /// Checkpoints failed due to unreachable providers.
    pub unreachable_failures: u64,
    /// Checkpoints failed due to transaction errors.
    pub transaction_failures: u64,
    /// Total conflicts detected.
    pub conflicts_detected: u64,
    /// Conflicts where auto-challenge was recommended.
    pub auto_challenge_recommended: u64,
    /// Total providers queried.
    pub providers_queried: u64,
    /// Successful provider queries.
    pub providers_responded: u64,
    /// Average checkpoint submission time (ms).
    pub avg_submission_time_ms: u64,
    /// Last checkpoint timestamp.
    pub last_checkpoint_time: Option<Instant>,
    /// Rolling average of consensus rate (0.0 - 1.0).
    pub avg_consensus_rate: f64,
}

impl CheckpointMetrics {
    /// Record a checkpoint attempt result.
    pub fn record_attempt(&mut self, result: &CheckpointResult, duration_ms: u64) {
        self.total_attempts += 1;
        self.last_checkpoint_time = Some(Instant::now());

        match result {
            CheckpointResult::Submitted { signers, .. } => {
                self.successful_submissions += 1;
                // Update rolling average submission time (after incrementing count)
                let n = self.successful_submissions;
                self.avg_submission_time_ms =
                    (self.avg_submission_time_ms * (n - 1) + duration_ms) / n;
                self.providers_responded += signers.len() as u64;
            }
            CheckpointResult::InsufficientConsensus { agreeing, .. } => {
                self.insufficient_consensus_count += 1;
                self.providers_responded += *agreeing as u64;
            }
            CheckpointResult::ProvidersUnreachable { providers } => {
                self.unreachable_failures += 1;
                // These providers didn't respond
                self.providers_queried += providers.len() as u64;
            }
            CheckpointResult::NoProviders => {
                // No providers configured
            }
            CheckpointResult::TransactionFailed { .. } => {
                self.transaction_failures += 1;
            }
        }
    }

    /// Record a conflict detection.
    pub fn record_conflict(&mut self, conflict: &ProviderConflict) {
        self.conflicts_detected += 1;

        // Check if auto-challenge is recommended
        for c in &conflict.conflicts {
            if matches!(c.conflict_type, ConflictType::DataDivergence) {
                self.auto_challenge_recommended += 1;
                break;
            }
        }
    }

    /// Calculate success rate.
    pub fn success_rate(&self) -> f64 {
        if self.total_attempts == 0 {
            1.0
        } else {
            self.successful_submissions as f64 / self.total_attempts as f64
        }
    }

    /// Calculate provider response rate.
    pub fn provider_response_rate(&self) -> f64 {
        if self.providers_queried == 0 {
            1.0
        } else {
            self.providers_responded as f64 / self.providers_queried as f64
        }
    }
}

/// Configuration for automatic challenge submission.
#[derive(Clone, Debug)]
pub struct AutoChallengeConfig {
    /// Whether auto-challenge is enabled.
    pub enabled: bool,
    /// Minimum conflict occurrences before challenging.
    pub min_conflict_count: u32,
    /// Time to wait for sync before considering challenge.
    pub sync_wait_duration: Duration,
    /// Whether to challenge on data divergence (same leaf count, different root).
    pub challenge_on_divergence: bool,
}

impl Default for AutoChallengeConfig {
    fn default() -> Self {
        Self {
            enabled: false, // Disabled by default for safety
            min_conflict_count: 3,
            sync_wait_duration: Duration::from_secs(60),
            challenge_on_divergence: true,
        }
    }
}

/// Challenge recommendation from conflict analysis.
#[derive(Clone, Debug)]
pub struct ChallengeRecommendation {
    /// Provider to potentially challenge.
    pub provider: AccountId32,
    /// Reason for the recommendation.
    pub reason: ChallengeReason,
    /// Confidence level (0.0 - 1.0).
    pub confidence: f64,
    /// Number of times this conflict was observed.
    pub occurrence_count: u32,
    /// Evidence for the challenge.
    pub evidence: ChallengeEvidence,
}

/// Reason for challenge recommendation.
#[derive(Clone, Debug)]
pub enum ChallengeReason {
    /// Same leaf count but different MMR root.
    DataDivergence {
        majority_root: H256,
        provider_root: H256,
        leaf_count: u64,
    },
    /// Provider persistently behind after sync wait.
    PersistentlySyncing { behind_by: u64, duration: Duration },
    /// Provider claiming to be ahead of majority.
    ClaimingAhead {
        claimed_leaf_count: u64,
        majority_leaf_count: u64,
    },
}

/// Evidence to support a challenge.
#[derive(Clone, Debug)]
pub struct ChallengeEvidence {
    /// Bucket ID where conflict occurred.
    pub bucket_id: BucketId,
    /// Majority commitment from agreeing providers.
    pub majority_commitment: (H256, u64, u64), // (mmr_root, start_seq, leaf_count)
    /// Signatures from majority providers.
    pub majority_signatures: Vec<(AccountId32, Vec<u8>)>,
    /// Provider's commitment that conflicts.
    pub provider_commitment: Option<(H256, u64, u64)>,
    /// Timestamps of conflict observations.
    pub observation_times: Vec<Instant>,
}

/// Result of executing auto-challenges.
#[derive(Clone, Debug)]
pub struct AutoChallengeResult {
    /// Number of providers analyzed.
    pub providers_analyzed: usize,
    /// Challenges successfully submitted.
    pub challenges_submitted: Vec<SubmittedChallenge>,
    /// Challenges that failed to submit.
    pub challenges_failed: Vec<FailedChallenge>,
    /// Providers skipped (below confidence threshold).
    pub providers_skipped: usize,
}

/// A successfully submitted challenge.
#[derive(Clone, Debug)]
pub struct SubmittedChallenge {
    /// The provider challenged.
    pub provider: AccountId32,
    /// Challenge ID from the chain.
    pub challenge_id: ChallengeId,
    /// Reason for the challenge.
    pub reason: ChallengeReason,
    /// Confidence level of the recommendation.
    pub confidence: f64,
}

/// A challenge that failed to submit.
#[derive(Clone, Debug)]
pub struct FailedChallenge {
    /// The provider we tried to challenge.
    pub provider: AccountId32,
    /// Reason for the challenge attempt.
    pub reason: ChallengeReason,
    /// Error message.
    pub error: String,
}

/// Result of attempting to submit a checkpoint.
#[derive(Clone, Debug)]
pub enum CheckpointResult {
    /// Checkpoint submitted successfully.
    Submitted {
        /// Block hash where the transaction was included.
        block_hash: H256,
        /// Providers whose signatures were included.
        signers: Vec<AccountId32>,
    },
    /// Not enough providers agreed (below threshold).
    InsufficientConsensus {
        /// Number of agreeing providers.
        agreeing: usize,
        /// Number required to meet threshold.
        required: usize,
        /// Providers with different data.
        disagreements: Vec<(AccountId32, H256)>,
    },
    /// All providers were unreachable.
    ProvidersUnreachable {
        /// List of unreachable providers.
        providers: Vec<AccountId32>,
    },
    /// No providers found for this bucket.
    NoProviders,
    /// Transaction submission failed.
    TransactionFailed {
        /// Error message.
        error: String,
    },
}

// ============================================================================
// Checkpoint Manager
// ============================================================================

/// Manages checkpoint collection and submission for buckets.
///
/// The CheckpointManager handles:
/// - Discovering providers from on-chain state or configured endpoints
/// - Collecting commitments from multiple providers in parallel
/// - Verifying consensus (majority agreement)
/// - Submitting checkpoint transactions on-chain
/// - Tracking provider health over time
#[allow(clippy::type_complexity)]
pub struct CheckpointManager {
    /// Configuration.
    config: CheckpointConfig,
    /// Substrate client for chain interactions.
    chain_client: SubstrateClient,
    /// HTTP client for provider communication.
    http_client: reqwest::Client,
    /// Manually configured provider endpoints.
    provider_endpoints: Vec<String>,
    /// Cached provider info per bucket.
    provider_cache: Arc<RwLock<HashMap<BucketId, CachedProviders>>>,
    /// Provider health history tracking.
    health_history: Arc<RwLock<HashMap<AccountId32, ProviderHealthHistory>>>,
    /// Metrics tracking.
    metrics: Arc<RwLock<CheckpointMetrics>>,
    /// Conflict history for auto-challenge analysis.
    conflict_history: Arc<RwLock<HashMap<(BucketId, AccountId32), Vec<ProviderConflict>>>>,
    /// Auto-challenge configuration.
    auto_challenge_config: AutoChallengeConfig,
}

/// Cached provider information with TTL.
struct CachedProviders {
    providers: Vec<ProviderInfo>,
    cached_at: Instant,
}

impl CheckpointManager {
    /// Create a new CheckpointManager.
    ///
    /// # Arguments
    /// * `chain_endpoint` - WebSocket endpoint for the parachain
    /// * `config` - Configuration options
    pub async fn new(chain_endpoint: &str, config: CheckpointConfig) -> Result<Self, ClientError> {
        let chain_client = SubstrateClient::connect(chain_endpoint).await?;

        Ok(Self {
            config,
            chain_client,
            http_client: reqwest::Client::new(),
            provider_endpoints: Vec::new(),
            provider_cache: Arc::new(RwLock::new(HashMap::new())),
            health_history: Arc::new(RwLock::new(HashMap::new())),
            metrics: Arc::new(RwLock::new(CheckpointMetrics::default())),
            conflict_history: Arc::new(RwLock::new(HashMap::new())),
            auto_challenge_config: AutoChallengeConfig::default(),
        })
    }

    /// Create with an existing SubstrateClient (for sharing connections).
    pub fn with_chain_client(chain_client: SubstrateClient, config: CheckpointConfig) -> Self {
        Self {
            config,
            chain_client,
            http_client: reqwest::Client::new(),
            provider_endpoints: Vec::new(),
            provider_cache: Arc::new(RwLock::new(HashMap::new())),
            health_history: Arc::new(RwLock::new(HashMap::new())),
            metrics: Arc::new(RwLock::new(CheckpointMetrics::default())),
            conflict_history: Arc::new(RwLock::new(HashMap::new())),
            auto_challenge_config: AutoChallengeConfig::default(),
        }
    }

    /// Configure auto-challenge settings.
    pub fn with_auto_challenge(mut self, config: AutoChallengeConfig) -> Self {
        self.auto_challenge_config = config;
        self
    }

    /// Add a provider endpoint for commitment collection.
    pub fn with_provider(mut self, endpoint: &str) -> Self {
        self.provider_endpoints.push(endpoint.to_string());
        self
    }

    /// Add multiple provider endpoints.
    pub fn with_providers(mut self, endpoints: Vec<String>) -> Self {
        self.provider_endpoints.extend(endpoints);
        self
    }

    /// Set the signer for submitting transactions.
    pub fn with_signer(mut self, signer: crate::Signer) -> Self {
        self.chain_client = self.chain_client.with_signer(signer);
        self
    }

    // ========================================================================
    // Provider Discovery
    // ========================================================================

    /// Get providers for a bucket.
    ///
    /// First checks cache, then manual endpoints, then discovers from chain state.
    pub async fn get_providers(
        &self,
        bucket_id: BucketId,
    ) -> Result<Vec<ProviderInfo>, ClientError> {
        // Check cache first
        {
            let cache = self.provider_cache.read().await;
            if let Some(cached) = cache.get(&bucket_id) {
                if cached.cached_at.elapsed() < self.config.provider_cache_ttl {
                    return Ok(cached.providers.clone());
                }
            }
        }

        // If we have manually configured endpoints, use those. Their accounts
        // must come from the nodes themselves: the checkpoint extrinsic pairs
        // each signature with an account and the pallet rejects any signer that
        // is not in the bucket's snapshot.
        if !self.provider_endpoints.is_empty() {
            let ids = futures::future::join_all(
                self.provider_endpoints
                    .iter()
                    .map(|endpoint| crate::ProviderClient::fetch_provider_id(endpoint)),
            )
            .await;

            let mut providers = Vec::new();
            for (endpoint, id) in self.provider_endpoints.iter().zip(ids) {
                match id {
                    Ok(account_id) => providers.push(ProviderInfo {
                        account_id,
                        endpoint: endpoint.clone(),
                        public_key: Vec::new(),
                        last_seen: None,
                        status: ProviderStatus::Unknown,
                    }),
                    Err(e) => tracing::warn!(
                        "Skipping provider {endpoint}: could not read its account from /info: {e}"
                    ),
                }
            }

            if providers.is_empty() {
                return Err(ClientError::Chain(
                    "No configured provider endpoint reported a usable account id".to_string(),
                ));
            }

            // Update cache
            self.update_provider_cache(bucket_id, providers.clone())
                .await;
            return Ok(providers);
        }

        // Otherwise discover from chain
        let providers = self.discover_providers_from_chain(bucket_id).await?;

        // Update cache
        self.update_provider_cache(bucket_id, providers.clone())
            .await;

        Ok(providers)
    }

    /// Discover providers for a bucket from on-chain state.
    ///
    /// This queries:
    /// 1. `StorageProvider.Buckets(bucket_id)` - to get the list of primary providers
    /// 2. `StorageProvider.Providers(account_id)` - for each provider to get endpoint info
    pub async fn discover_providers_from_chain(
        &self,
        bucket_id: BucketId,
    ) -> Result<Vec<ProviderInfo>, ClientError> {
        let at = self.chain_client.at_current_block().await?;

        let bucket = at
            .storage()
            .try_fetch(api::storage().storage_provider().buckets(), (bucket_id,))
            .await
            .map_err(|e| ClientError::Chain(format!("Failed to fetch bucket: {e}")))?
            .ok_or_else(|| ClientError::Chain(format!("Bucket {bucket_id} not found")))?
            .decode()
            .map_err(|e| ClientError::Chain(format!("Failed to decode bucket: {e}")))?;

        let provider_accounts: Vec<AccountId32> = convert::unbounded(bucket.primary_providers)
            .iter()
            .map(convert::to_sp_account)
            .collect();

        if provider_accounts.is_empty() {
            return Err(ClientError::Chain(format!(
                "No primary providers found for bucket {bucket_id}"
            )));
        }

        // Read every provider at the same block, validating the address once.
        let entry = at
            .storage()
            .entry(api::storage().storage_provider().providers())
            .map_err(|e| ClientError::Chain(format!("Failed to prepare provider query: {e}")))?;

        let mut providers = Vec::new();
        for account_id in provider_accounts {
            // A provider that is unreadable or unreachable still belongs in the
            // list, flagged Unknown, so callers see the full membership.
            let mut info = ProviderInfo {
                account_id: account_id.clone(),
                endpoint: String::new(),
                public_key: Vec::new(),
                last_seen: None,
                status: ProviderStatus::Unknown,
            };

            if let Ok(Some(value)) = entry
                .try_fetch((convert::to_subxt_account(&account_id),))
                .await
            {
                if let Ok(registration) = value.decode() {
                    match Self::parse_multiaddr_to_http(&convert::unbounded(registration.multiaddr))
                    {
                        Ok(endpoint) => {
                            info.endpoint = endpoint;
                            info.public_key = convert::unbounded(registration.public_key);
                        }
                        Err(e) => {
                            tracing::warn!("Provider {account_id} has an unusable multiaddr: {e}")
                        }
                    }
                }
            }

            providers.push(info);
        }

        Ok(providers)
    }

    /// Parse a libp2p multiaddr to an HTTP(S) base URL.
    ///
    /// Handles the two forms the provider node registers on chain:
    /// - `/ip4/<host>/tcp/<port>` (also `ip6`/`dns4`/`dns6`/`dns`)
    ///   → `http://<host>:<port>`
    /// - `/dns4/<host>/tcp/443/tls/http/http-path/<path>`
    ///   → `https://<host>/<path>`
    ///
    /// A `tls` (or `https`/`wss`) segment selects `https`; the default port for
    /// the scheme (`443` https, `80` http) is elided; an `http-path/<segment>`
    /// appends a percent-decoded path; `ip6` hosts are bracketed.
    ///
    /// Deliberately narrow: it covers the addresses this system writes, not the
    /// full multiaddr grammar (a bare `tcp/443` stays http, a trailing
    /// `/p2p/...` is ignored), so it is not a drop-in match for the TS side.
    fn parse_multiaddr_to_http(multiaddr_bytes: &[u8]) -> Result<String, ClientError> {
        /// Minimal percent-decoder for `http-path` segments (`%XX`).
        fn percent_decode(s: &str) -> String {
            let bytes = s.as_bytes();
            let mut out = Vec::with_capacity(bytes.len());
            let mut i = 0;
            while i < bytes.len() {
                if bytes[i] == b'%' && i + 2 < bytes.len() {
                    if let Ok(b) = u8::from_str_radix(&s[i + 1..i + 3], 16) {
                        out.push(b);
                        i += 3;
                        continue;
                    }
                }
                out.push(bytes[i]);
                i += 1;
            }
            String::from_utf8_lossy(&out).into_owned()
        }

        let multiaddr_str = String::from_utf8(multiaddr_bytes.to_vec())
            .map_err(|e| ClientError::Chain(format!("Invalid multiaddr encoding: {e}")))?;

        let parts: Vec<&str> = multiaddr_str.split('/').filter(|s| !s.is_empty()).collect();

        // Minimum shape is `<proto>/<host>/tcp/<port>`.
        if parts.len() < 4 {
            return Err(ClientError::Chain(format!(
                "Invalid multiaddr format: {multiaddr_str}"
            )));
        }

        let host = match parts[0] {
            "ip4" | "dns4" | "dns6" | "dns" => parts[1].to_string(),
            "ip6" => format!("[{}]", parts[1]),
            other => {
                return Err(ClientError::Chain(format!(
                    "Unsupported multiaddr protocol: {other}"
                )));
            }
        };

        if parts[2] != "tcp" {
            return Err(ClientError::Chain(format!(
                "Unsupported multiaddr transport: {}",
                parts[2]
            )));
        }
        let port: u16 = parts[3]
            .parse()
            .map_err(|_| ClientError::Chain(format!("Invalid multiaddr port: {}", parts[3])))?;

        // Segments after `tcp/<port>` select the scheme and an optional path.
        let rest = &parts[4..];
        let tls = rest.iter().any(|p| matches!(*p, "tls" | "https" | "wss"));
        let scheme = if tls { "https" } else { "http" };

        let path = match rest.iter().position(|p| *p == "http-path") {
            Some(idx) => {
                let raw = rest.get(idx + 1).ok_or_else(|| {
                    ClientError::Chain("Missing value for http-path in multiaddr".to_string())
                })?;
                format!("/{}", percent_decode(raw).trim_start_matches('/'))
            }
            None => String::new(),
        };

        let default_port = if tls { 443 } else { 80 };
        let port_suffix = if port == default_port {
            String::new()
        } else {
            format!(":{port}")
        };
        Ok(format!("{scheme}://{host}{port_suffix}{path}"))
    }

    /// Update the provider cache.
    async fn update_provider_cache(&self, bucket_id: BucketId, providers: Vec<ProviderInfo>) {
        let mut cache = self.provider_cache.write().await;
        cache.insert(
            bucket_id,
            CachedProviders {
                providers,
                cached_at: Instant::now(),
            },
        );
    }

    // ========================================================================
    // Commitment Collection
    // ========================================================================

    /// Collect commitments from all providers for a bucket.
    ///
    /// The nonce is chosen here (the current anchor block) and sent to every
    /// provider, which signs over it and echoes it back — so all signatures
    /// cover one identical `CommitmentPayload`, exactly as the pallet
    /// verifies them. Responses that deviate (wrong nonce, unusable
    /// signature, a range that regresses below the on-chain snapshot) are
    /// dropped rather than allowed to spoil the batch.
    pub async fn collect_commitments(
        &self,
        bucket_id: BucketId,
    ) -> Result<CommitmentCollection, ClientError> {
        // Get providers
        let providers = self.get_providers(bucket_id).await?;

        if providers.is_empty() {
            return Err(ClientError::Chain(format!(
                "No providers found for bucket {bucket_id}"
            )));
        }

        // The anchor block is the pallet's clock: using it as the nonce keeps
        // us inside `MaxNonceAge` and is a value no provider can steer.
        let at = self
            .chain_client
            .api()
            .at_current_block()
            .await
            .map_err(|e| ClientError::Chain(format!("Failed to read chain state: {e}")))?;
        let nonce = u64::from(crate::substrate::fetch_current_anchor_block(&at).await?);
        let floor = Self::snapshot_floor(&at, bucket_id).await?;

        // Query all providers in parallel, all over the same nonce
        let futures: Vec<_> = providers
            .iter()
            .map(|p| self.query_provider_commitment(p, bucket_id, nonce))
            .collect();

        let results = futures::future::join_all(futures).await;

        // Categorize results by MMR root, keeping only usable responses
        let mut commitments_by_root: HashMap<
            H256,
            Vec<(AccountId32, CommitmentResponse, Vec<u8>)>,
        > = HashMap::new();
        let mut unreachable = Vec::new();

        for (provider, result) in providers.iter().zip(results) {
            let account = provider.account_id.clone();
            let Ok(commitment) = result else {
                unreachable.push(account);
                continue;
            };

            match Self::validated_signature(bucket_id, &account, &commitment, nonce, floor) {
                Some(sig_bytes) => {
                    let mmr_root = self.parse_h256(&commitment.mmr_root)?;
                    commitments_by_root
                        .entry(mmr_root)
                        .or_default()
                        .push((account, commitment, sig_bytes));
                }
                // An unusable response is no better than no response: the
                // provider contributes nothing this round.
                None => unreachable.push(account),
            }
        }

        // Find majority consensus
        let (majority_root, agreeing) = commitments_by_root
            .iter()
            .max_by_key(|(_, v)| v.len())
            .map(|(root, v)| (*root, (*v).clone()))
            .unwrap_or_else(|| (H256::zero(), Vec::new()));

        // Build disagreeing list
        let disagreeing: Vec<_> = commitments_by_root
            .iter()
            .filter(|(root, _)| **root != majority_root)
            .flat_map(|(root, providers)| {
                providers
                    .iter()
                    .map(|(id, _, _)| (id.clone(), *root))
                    .collect::<Vec<_>>()
            })
            .collect();

        // Same root still leaves room for disagreement about the committed
        // range, and the pallet accepts only one payload per call.
        let mut by_payload = SigsByPayload::new();
        for (id, c, sig) in &agreeing {
            by_payload
                .entry(PayloadKey::new(c.start_seq, c.leaf_count, c.nonce))
                .or_default()
                .push((id.clone(), sig.clone()));
        }
        let (payload, signatures) = modal_payload_group(by_payload);

        let dropped = agreeing.len().saturating_sub(signatures.len());
        if dropped > 0 {
            tracing::warn!(
                "Bucket {bucket_id}: dropping {dropped} same-root signature(s) covering a \
                 different (start_seq, leaf_count) than the majority range"
            );
        }

        Ok(CommitmentCollection {
            bucket_id,
            mmr_root: majority_root,
            start_seq: payload.start_seq,
            leaf_count: payload.leaf_count,
            nonce: payload.nonce,
            signatures,
            agreeing_providers: agreeing.iter().map(|(id, _, _)| id.clone()).collect(),
            disagreeing_providers: disagreeing,
            unreachable_providers: unreachable,
        })
    }

    /// The committed range already on chain, if any: a checkpoint must never
    /// move backwards from it.
    async fn snapshot_floor<C>(
        at: &subxt::client::ClientAtBlock<subxt::PolkadotConfig, C>,
        bucket_id: BucketId,
    ) -> Result<Option<(u64, u64)>, ClientError>
    where
        C: subxt::client::OnlineClientAtBlockT<subxt::PolkadotConfig>,
    {
        let bucket = at
            .storage()
            .try_fetch(api::storage().storage_provider().buckets(), (bucket_id,))
            .await
            .map_err(|e| ClientError::Chain(format!("Failed to fetch bucket: {e}")))?;

        let Some(bucket) = bucket else {
            return Ok(None);
        };
        let bucket = bucket
            .decode()
            .map_err(|e| ClientError::Chain(format!("Failed to decode bucket: {e}")))?;

        Ok(bucket
            .snapshot
            .map(|s| (s.commitment.start_seq, s.commitment.leaf_count)))
    }

    /// Return a provider response's signature bytes, or `None` (with a
    /// warning) when the response cannot be part of a checkpoint.
    fn validated_signature(
        bucket_id: BucketId,
        account: &AccountId32,
        commitment: &CommitmentResponse,
        requested_nonce: u64,
        floor: Option<(u64, u64)>,
    ) -> Option<Vec<u8>> {
        if commitment.nonce != requested_nonce {
            tracing::warn!(
                "Bucket {bucket_id}: provider {account} echoed nonce {} instead of the requested \
                 {requested_nonce}; its signature cannot join this checkpoint",
                commitment.nonce
            );
            return None;
        }

        // A shrinking range would narrow what the provider can be challenged
        // over — never accept it, whatever the majority says.
        if let Some((floor_start_seq, floor_leaf_count)) = floor {
            if commitment.start_seq < floor_start_seq || commitment.leaf_count < floor_leaf_count {
                tracing::warn!(
                    "Bucket {bucket_id}: provider {account} reported range (start_seq={}, \
                     leaf_count={}) behind the on-chain snapshot (start_seq={floor_start_seq}, \
                     leaf_count={floor_leaf_count})",
                    commitment.start_seq,
                    commitment.leaf_count
                );
                return None;
            }
        }

        match Self::decode_signature(&commitment.provider_signature) {
            Ok(sig) if sig.len() == SR25519_SIGNATURE_LEN => Some(sig),
            Ok(sig) => {
                tracing::warn!(
                    "Bucket {bucket_id}: provider {account} returned a {}-byte signature, \
                     expected {SR25519_SIGNATURE_LEN}",
                    sig.len()
                );
                None
            }
            Err(e) => {
                tracing::warn!(
                    "Bucket {bucket_id}: provider {account} returned an undecodable signature: {e}"
                );
                None
            }
        }
    }

    /// Query a single provider for their commitment with health tracking.
    ///
    /// `nonce` is the caller-chosen replay-protection value the provider signs
    /// over; the endpoint requires it and echoes it back.
    async fn query_provider_commitment(
        &self,
        provider: &ProviderInfo,
        bucket_id: BucketId,
        nonce: u64,
    ) -> Result<CommitmentResponse, ClientError> {
        let mut retries = 0;
        let mut delay = self.config.retry_delay;
        let start = Instant::now();

        loop {
            let url = format!(
                "{}/commitment?bucket_id={}&nonce={}",
                provider.endpoint, bucket_id, nonce
            );

            let result = tokio::time::timeout(self.config.provider_timeout, async {
                self.http_client.get(&url).send().await
            })
            .await;

            match result {
                Ok(Ok(response)) => {
                    if response.status().is_success() {
                        let response_time_ms = start.elapsed().as_millis() as u64;

                        match response.json::<CommitmentResponse>().await {
                            Ok(commitment) => {
                                // Record success
                                self.record_provider_success(
                                    &provider.account_id,
                                    response_time_ms,
                                )
                                .await;
                                return Ok(commitment);
                            }
                            Err(e) => {
                                let error = format!("JSON parse error: {e}");
                                self.record_provider_failure(&provider.account_id, error.clone())
                                    .await;
                                return Err(ClientError::Serialization(error));
                            }
                        }
                    } else {
                        let error = format!("Provider returned status {}", response.status());
                        if retries < self.config.max_retries {
                            retries += 1;
                            tokio::time::sleep(delay).await;
                            delay *= 2;
                            continue;
                        }
                        self.record_provider_failure(&provider.account_id, error.clone())
                            .await;
                        return Err(ClientError::Api(error));
                    }
                }
                Ok(Err(e)) => {
                    if retries < self.config.max_retries {
                        retries += 1;
                        tokio::time::sleep(delay).await;
                        delay *= 2;
                        continue;
                    }
                    let error = format!("Request failed: {e}");
                    self.record_provider_failure(&provider.account_id, error.clone())
                        .await;
                    return Err(ClientError::Api(error));
                }
                Err(_) => {
                    if retries < self.config.max_retries {
                        retries += 1;
                        tokio::time::sleep(delay).await;
                        delay *= 2;
                        continue;
                    }
                    let error = "Request timeout".to_string();
                    self.record_provider_failure(&provider.account_id, error.clone())
                        .await;
                    return Err(ClientError::Api(error));
                }
            }
        }
    }

    // ========================================================================
    // Checkpoint Submission
    // ========================================================================

    /// Submit a checkpoint for a bucket.
    ///
    /// This collects commitments, verifies consensus, and submits on-chain.
    pub async fn submit_checkpoint(&self, bucket_id: BucketId) -> CheckpointResult {
        // Collect commitments
        let collection = match self.collect_commitments(bucket_id).await {
            Ok(c) => c,
            Err(e) => {
                return CheckpointResult::TransactionFailed {
                    error: e.to_string(),
                }
            }
        };

        // Check if all providers unreachable
        if collection.agreeing_providers.is_empty() {
            if collection.unreachable_providers.is_empty() {
                return CheckpointResult::NoProviders;
            }
            return CheckpointResult::ProvidersUnreachable {
                providers: collection.unreachable_providers,
            };
        }

        // Calculate total and required providers
        let total_providers = collection.agreeing_providers.len()
            + collection.disagreeing_providers.len()
            + collection.unreachable_providers.len();

        let required = (total_providers as f64 * self.config.consensus_threshold_percent as f64
            / 100.0)
            .ceil() as usize;

        // Check consensus threshold against what will actually be submitted:
        // agreeing providers whose signature survived payload grouping. The
        // pallet counts signatures, not opinions.
        if collection.signatures.len() < required {
            return CheckpointResult::InsufficientConsensus {
                agreeing: collection.signatures.len(),
                required,
                disagreements: collection.disagreeing_providers,
            };
        }

        let signers: Vec<AccountId32> = collection
            .signatures
            .iter()
            .map(|(id, _)| id.clone())
            .collect();

        // Submit on-chain
        match self.submit_commitment_onchain(&collection).await {
            Ok(block_hash) => CheckpointResult::Submitted {
                block_hash,
                signers,
            },
            Err(e) => CheckpointResult::TransactionFailed {
                error: e.to_string(),
            },
        }
    }

    /// Submit the commitment transaction on-chain via the `checkpoint`
    /// extrinsic.
    ///
    /// Providers excluded from this batch cannot simply be appended later:
    /// the pallet's `extend_checkpoint` verifies late signatures against the
    /// stored snapshot's own nonce, so they must re-sign that payload first
    /// (and this SDK does not expose the extrinsic yet).
    async fn submit_commitment_onchain(
        &self,
        collection: &CommitmentCollection,
    ) -> Result<H256, ClientError> {
        let api = self.chain_client.api();
        let signer = self.chain_client.signer()?;

        let tx = extrinsics::checkpoint(
            collection.bucket_id,
            Commitment {
                mmr_root: collection.mmr_root,
                start_seq: collection.start_seq,
                leaf_count: collection.leaf_count,
            },
            collection.nonce,
            collection.signatures.clone(),
        )?;

        // Submit and wait for finalization
        let tx_progress = api
            .at_current_block()
            .await
            .map_err(|e| ClientError::Chain(format!("Failed to submit: {e}")))?
            .transactions()
            .sign_and_submit_then_watch_default(&tx, signer)
            .await
            .map_err(|e| ClientError::Chain(format!("Failed to submit: {e}")))?;

        let _events = tx_progress
            .wait_for_finalized_success()
            .await
            .map_err(|e| ClientError::Chain(format!("Transaction failed: {e}")))?;

        // Return a success hash (we can't easily get block hash from events in this subxt version)
        Ok(collection.mmr_root)
    }

    // ========================================================================
    // Helpers
    // ========================================================================

    /// Parse a hex string to H256.
    fn parse_h256(&self, hex_str: &str) -> Result<H256, ClientError> {
        let s = hex_str.strip_prefix("0x").unwrap_or(hex_str);
        let bytes = hex::decode(s).map_err(|e| ClientError::Serialization(e.to_string()))?;
        if bytes.len() != 32 {
            return Err(ClientError::Serialization(
                "Invalid H256 length".to_string(),
            ));
        }
        let mut arr = [0u8; 32];
        arr.copy_from_slice(&bytes);
        Ok(H256::from(arr))
    }

    /// Decode a hex signature string to bytes.
    fn decode_signature(sig_str: &str) -> Result<Vec<u8>, ClientError> {
        let s = sig_str.strip_prefix("0x").unwrap_or(sig_str);
        hex::decode(s).map_err(|e| ClientError::Serialization(e.to_string()))
    }

    /// Invalidate the provider cache for a bucket.
    pub async fn invalidate_cache(&self, bucket_id: BucketId) {
        let mut cache = self.provider_cache.write().await;
        cache.remove(&bucket_id);
    }

    /// Get the current provider health status.
    pub async fn get_provider_status(
        &self,
        bucket_id: BucketId,
    ) -> Result<Vec<(AccountId32, ProviderStatus)>, ClientError> {
        let providers = self.get_providers(bucket_id).await?;

        let mut status = Vec::new();
        for provider in providers {
            // Do a quick health check
            let health_result = self.check_provider_health(&provider).await;
            let provider_status = match health_result {
                Ok(_) => ProviderStatus::Healthy,
                Err(e) => ProviderStatus::Degraded {
                    last_error: e.to_string(),
                },
            };
            status.push((provider.account_id, provider_status));
        }

        Ok(status)
    }

    /// Check if a provider is healthy.
    async fn check_provider_health(&self, provider: &ProviderInfo) -> Result<(), ClientError> {
        let url = format!("{}/health", provider.endpoint);

        let start = Instant::now();
        let result = tokio::time::timeout(Duration::from_secs(5), async {
            self.http_client.get(&url).send().await
        })
        .await;

        let response_time_ms = start.elapsed().as_millis() as u64;

        match result {
            Ok(Ok(response)) if response.status().is_success() => {
                // Record success
                self.record_provider_success(&provider.account_id, response_time_ms)
                    .await;
                Ok(())
            }
            Ok(Ok(response)) => {
                let error = format!("Health check returned {}", response.status());
                self.record_provider_failure(&provider.account_id, error.clone())
                    .await;
                Err(ClientError::Api(error))
            }
            Ok(Err(e)) => {
                let error = format!("Health check failed: {e}");
                self.record_provider_failure(&provider.account_id, error.clone())
                    .await;
                Err(ClientError::Api(error))
            }
            Err(_) => {
                let error = "Health check timeout".to_string();
                self.record_provider_failure(&provider.account_id, error.clone())
                    .await;
                Err(ClientError::Api(error))
            }
        }
    }

    // ========================================================================
    // Conflict Detection
    // ========================================================================

    /// Analyze a commitment collection for conflicts.
    ///
    /// Returns Some(ProviderConflict) if any providers disagree with the majority.
    pub fn analyze_conflicts(&self, collection: &CommitmentCollection) -> Option<ProviderConflict> {
        if collection.disagreeing_providers.is_empty() {
            return None;
        }

        let _majority_leaf_count = collection.leaf_count;
        let mut conflicts = Vec::new();

        for (account_id, their_root) in &collection.disagreeing_providers {
            // Determine conflict type
            // Note: We don't have their leaf_count directly, so we make assumptions
            // In a full implementation, we'd query each provider for full commitment
            let conflict_type = if their_root == &collection.mmr_root {
                // Same root - shouldn't be in disagreeing list
                continue;
            } else {
                // Different root - assume data divergence for now
                // A full implementation would compare leaf counts
                ConflictType::DataDivergence
            };

            conflicts.push(ConflictingProvider {
                account_id: account_id.clone(),
                mmr_root: *their_root,
                leaf_count: 0, // Unknown without additional query
                conflict_type,
            });
        }

        if conflicts.is_empty() {
            return None;
        }

        // Determine resolution strategy
        let total_providers = collection.agreeing_providers.len()
            + collection.disagreeing_providers.len()
            + collection.unreachable_providers.len();

        let majority_percentage =
            collection.agreeing_providers.len() as f64 / total_providers as f64 * 100.0;

        let resolution = if majority_percentage >= self.config.consensus_threshold_percent as f64 {
            ConflictResolution::ProceedWithMajority
        } else if conflicts
            .iter()
            .all(|c| matches!(c.conflict_type, ConflictType::SyncDelay { .. }))
        {
            ConflictResolution::WaitForSync {
                estimated_blocks: 10, // Estimate
            }
        } else if conflicts.len() == 1 {
            ConflictResolution::ConsiderChallenge {
                provider: conflicts[0].account_id.clone(),
            }
        } else {
            ConflictResolution::ManualIntervention {
                reason: format!(
                    "{} providers disagree, only {:.0}% consensus",
                    conflicts.len(),
                    majority_percentage
                ),
            }
        };

        Some(ProviderConflict {
            bucket_id: collection.bucket_id,
            majority_root: collection.mmr_root,
            majority_count: collection.agreeing_providers.len(),
            conflicts,
            detected_at: Instant::now(),
            resolution,
        })
    }

    /// Collect commitments and return any detected conflicts.
    pub async fn collect_commitments_with_conflicts(
        &self,
        bucket_id: BucketId,
    ) -> Result<(CommitmentCollection, Option<ProviderConflict>), ClientError> {
        let collection = self.collect_commitments(bucket_id).await?;
        let conflict = self.analyze_conflicts(&collection);
        Ok((collection, conflict))
    }

    // ========================================================================
    // Health Tracking
    // ========================================================================

    /// Record a successful provider interaction.
    async fn record_provider_success(&self, account_id: &AccountId32, response_time_ms: u64) {
        let mut history = self.health_history.write().await;
        let entry = history
            .entry(account_id.clone())
            .or_insert_with(|| ProviderHealthHistory::new(account_id.clone()));
        entry.record_success(response_time_ms);
    }

    /// Record a failed provider interaction.
    async fn record_provider_failure(&self, account_id: &AccountId32, error: String) {
        let mut history = self.health_history.write().await;
        let entry = history
            .entry(account_id.clone())
            .or_insert_with(|| ProviderHealthHistory::new(account_id.clone()));
        entry.record_failure(error);
    }

    /// Get health history for a specific provider.
    pub async fn get_health_history(
        &self,
        account_id: &AccountId32,
    ) -> Option<ProviderHealthHistory> {
        let history = self.health_history.read().await;
        history.get(account_id).cloned()
    }

    /// Get health history for all known providers.
    pub async fn get_all_health_history(&self) -> Vec<ProviderHealthHistory> {
        let history = self.health_history.read().await;
        history.values().cloned().collect()
    }

    /// Get providers sorted by health (healthiest first).
    pub async fn get_providers_by_health(
        &self,
        bucket_id: BucketId,
    ) -> Result<Vec<ProviderInfo>, ClientError> {
        let providers = self.get_providers(bucket_id).await?;
        let history = self.health_history.read().await;

        let mut scored_providers: Vec<_> = providers
            .into_iter()
            .map(|p| {
                let score = history
                    .get(&p.account_id)
                    .map(|h| h.success_rate())
                    .unwrap_or(0.5); // Unknown providers get neutral score
                (p, score)
            })
            .collect();

        // Sort by score descending (healthiest first)
        scored_providers.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));

        Ok(scored_providers.into_iter().map(|(p, _)| p).collect())
    }

    /// Check if a bucket has enough healthy providers to meet consensus.
    pub async fn has_enough_healthy_providers(
        &self,
        bucket_id: BucketId,
    ) -> Result<bool, ClientError> {
        let providers = self.get_providers(bucket_id).await?;
        let history = self.health_history.read().await;

        let healthy_count = providers
            .iter()
            .filter(|p| {
                history
                    .get(&p.account_id)
                    .map(|h| h.is_healthy())
                    .unwrap_or(true) // Unknown is considered potentially healthy
            })
            .count();

        let total = providers.len();
        let required =
            (total as f64 * self.config.consensus_threshold_percent as f64 / 100.0).ceil() as usize;

        Ok(healthy_count >= required)
    }

    // ========================================================================
    // Metrics & Monitoring (Phase 3)
    // ========================================================================

    /// Get a snapshot of current checkpoint metrics.
    pub async fn get_metrics(&self) -> CheckpointMetrics {
        self.metrics.read().await.clone()
    }

    /// Reset all metrics to zero.
    pub async fn reset_metrics(&self) {
        *self.metrics.write().await = CheckpointMetrics::default();
    }

    /// Record a checkpoint result in metrics.
    ///
    /// Called automatically by submit_checkpoint, but can also be called
    /// manually if integrating with custom checkpoint logic.
    pub async fn record_checkpoint_metrics(&self, result: &CheckpointResult, duration_ms: u64) {
        self.metrics
            .write()
            .await
            .record_attempt(result, duration_ms);
    }

    /// Record a conflict in metrics and history.
    ///
    /// Called when a conflict is detected. Stores the conflict for
    /// auto-challenge analysis and updates metrics.
    pub async fn record_conflict(&self, bucket_id: BucketId, conflict: &ProviderConflict) {
        // Update metrics
        self.metrics.write().await.record_conflict(conflict);

        // Store in conflict history for auto-challenge analysis
        let mut history = self.conflict_history.write().await;
        for conflicting in &conflict.conflicts {
            let key = (bucket_id, conflicting.account_id.clone());
            history
                .entry(key)
                .or_insert_with(Vec::new)
                .push(conflict.clone());
        }
    }

    // ========================================================================
    // Auto-Challenge Analysis (Phase 3)
    // ========================================================================

    /// Analyze conflict history and generate challenge recommendations.
    ///
    /// Returns a list of providers that should potentially be challenged
    /// based on repeated conflicts and the configured auto-challenge settings.
    pub async fn analyze_challenge_candidates(
        &self,
        bucket_id: BucketId,
    ) -> Vec<ChallengeRecommendation> {
        let history = self.conflict_history.read().await;
        let mut recommendations = Vec::new();

        // Get all conflicts for this bucket
        let bucket_conflicts: Vec<_> = history
            .iter()
            .filter(|((bid, _), _)| *bid == bucket_id)
            .collect();

        for ((_, provider), conflicts) in bucket_conflicts {
            // Check if enough conflicts to consider challenge
            if conflicts.len() < self.auto_challenge_config.min_conflict_count as usize {
                continue;
            }

            // Analyze conflict types
            let divergence_count = conflicts
                .iter()
                .flat_map(|c| &c.conflicts)
                .filter(|c| c.account_id == *provider)
                .filter(|c| matches!(c.conflict_type, ConflictType::DataDivergence))
                .count();

            let sync_delay_count = conflicts
                .iter()
                .flat_map(|c| &c.conflicts)
                .filter(|c| c.account_id == *provider)
                .filter(|c| matches!(c.conflict_type, ConflictType::SyncDelay { .. }))
                .count();

            // Generate recommendation based on conflict patterns
            if self.auto_challenge_config.challenge_on_divergence && divergence_count > 0 {
                // Data divergence - high confidence recommendation
                if let Some(latest_conflict) = conflicts.last() {
                    if let Some(provider_conflict) = latest_conflict
                        .conflicts
                        .iter()
                        .find(|c| c.account_id == *provider)
                    {
                        recommendations.push(ChallengeRecommendation {
                            provider: provider.clone(),
                            reason: ChallengeReason::DataDivergence {
                                majority_root: latest_conflict.majority_root,
                                provider_root: provider_conflict.mmr_root,
                                leaf_count: provider_conflict.leaf_count,
                            },
                            confidence: 0.9, // High confidence for data divergence
                            occurrence_count: divergence_count as u32,
                            evidence: ChallengeEvidence {
                                bucket_id,
                                majority_commitment: (
                                    latest_conflict.majority_root,
                                    0, // start_seq not tracked in conflict
                                    provider_conflict.leaf_count,
                                ),
                                majority_signatures: Vec::new(), // Would need to be collected
                                provider_commitment: Some((
                                    provider_conflict.mmr_root,
                                    0,
                                    provider_conflict.leaf_count,
                                )),
                                observation_times: conflicts
                                    .iter()
                                    .map(|c| c.detected_at)
                                    .collect(),
                            },
                        });
                    }
                }
            } else if sync_delay_count as u32 >= self.auto_challenge_config.min_conflict_count * 2 {
                // Persistently syncing - lower confidence
                if let Some(latest_conflict) = conflicts.last() {
                    if let Some(provider_conflict) = latest_conflict
                        .conflicts
                        .iter()
                        .find(|c| c.account_id == *provider)
                    {
                        if let ConflictType::SyncDelay { behind_by } =
                            provider_conflict.conflict_type
                        {
                            let first_seen = conflicts
                                .first()
                                .map(|c| c.detected_at)
                                .unwrap_or_else(Instant::now);
                            let duration = first_seen.elapsed();

                            if duration >= self.auto_challenge_config.sync_wait_duration {
                                recommendations.push(ChallengeRecommendation {
                                    provider: provider.clone(),
                                    reason: ChallengeReason::PersistentlySyncing {
                                        behind_by,
                                        duration,
                                    },
                                    confidence: 0.5, // Lower confidence for sync issues
                                    occurrence_count: sync_delay_count as u32,
                                    evidence: ChallengeEvidence {
                                        bucket_id,
                                        majority_commitment: (
                                            latest_conflict.majority_root,
                                            0,
                                            provider_conflict.leaf_count + behind_by,
                                        ),
                                        majority_signatures: Vec::new(),
                                        provider_commitment: Some((
                                            provider_conflict.mmr_root,
                                            0,
                                            provider_conflict.leaf_count,
                                        )),
                                        observation_times: conflicts
                                            .iter()
                                            .map(|c| c.detected_at)
                                            .collect(),
                                    },
                                });
                            }
                        }
                    }
                }
            }
        }

        // Sort by confidence (highest first)
        recommendations.sort_by(|a, b| {
            b.confidence
                .partial_cmp(&a.confidence)
                .unwrap_or(std::cmp::Ordering::Equal)
        });

        recommendations
    }

    /// Check if auto-challenge is enabled.
    pub fn is_auto_challenge_enabled(&self) -> bool {
        self.auto_challenge_config.enabled
    }

    /// Get auto-challenge configuration.
    pub fn auto_challenge_config(&self) -> &AutoChallengeConfig {
        &self.auto_challenge_config
    }

    /// Clear conflict history for a bucket.
    pub async fn clear_conflict_history(&self, bucket_id: BucketId) {
        let mut history = self.conflict_history.write().await;
        history.retain(|(bid, _), _| *bid != bucket_id);
    }

    /// Get conflict count for a specific provider in a bucket.
    pub async fn get_conflict_count(&self, bucket_id: BucketId, provider: &AccountId32) -> usize {
        let history = self.conflict_history.read().await;
        history
            .get(&(bucket_id, provider.clone()))
            .map(|v| v.len())
            .unwrap_or(0)
    }

    // ========================================================================
    // Auto-Challenge Execution
    // ========================================================================

    /// Execute auto-challenges for a bucket based on conflict analysis.
    ///
    /// This method:
    /// 1. Analyzes conflict history to find providers to challenge
    /// 2. Filters by confidence threshold
    /// 3. Submits challenges on-chain using the provided ChallengerClient
    /// 4. Updates metrics and clears conflict history for challenged providers
    ///
    /// # Arguments
    ///
    /// * `bucket_id` - The bucket to check for conflicts
    /// * `challenger` - A connected ChallengerClient for submitting challenges
    /// * `min_confidence` - Minimum confidence threshold (0.0 - 1.0)
    ///
    /// # Returns
    ///
    /// `AutoChallengeResult` with details of submitted and failed challenges.
    ///
    /// # Example
    ///
    /// ```ignore
    /// // Setup challenger client
    /// let mut challenger = ChallengerClient::with_defaults(Signer::from_seed("//Alice")?)?;
    /// challenger.connect().await?;
    ///
    /// // Execute auto-challenges
    /// let result = manager.execute_auto_challenges(bucket_id, &challenger, 0.7).await?;
    /// println!("Submitted {} challenges", result.challenges_submitted.len());
    /// ```
    pub async fn execute_auto_challenges(
        &self,
        bucket_id: BucketId,
        challenger: &ChallengerClient,
        min_confidence: f64,
    ) -> Result<AutoChallengeResult, ClientError> {
        // Check if auto-challenge is enabled
        if !self.auto_challenge_config.enabled {
            return Err(ClientError::Config(
                "Auto-challenge is not enabled. Use with_auto_challenge() to enable.".to_string(),
            ));
        }

        // Get challenge recommendations
        let recommendations = self.analyze_challenge_candidates(bucket_id).await;

        let mut result = AutoChallengeResult {
            providers_analyzed: recommendations.len(),
            challenges_submitted: Vec::new(),
            challenges_failed: Vec::new(),
            providers_skipped: 0,
        };

        for recommendation in recommendations {
            // Skip if below confidence threshold
            if recommendation.confidence < min_confidence {
                result.providers_skipped += 1;
                continue;
            }

            // Determine leaf_index and chunk_index based on challenge reason
            let (leaf_index, chunk_index) = match &recommendation.reason {
                ChallengeReason::DataDivergence { leaf_count, .. } => {
                    // Challenge a random leaf within the divergent range
                    // For simplicity, pick the last leaf and first chunk
                    (leaf_count.saturating_sub(1), 0u64)
                }
                ChallengeReason::PersistentlySyncing { behind_by, .. } => {
                    // Challenge at the point where they fell behind
                    let majority_count = recommendation.evidence.majority_commitment.2;
                    (majority_count.saturating_sub(*behind_by), 0u64)
                }
                ChallengeReason::ClaimingAhead {
                    majority_leaf_count,
                    ..
                } => {
                    // Challenge at the majority's edge
                    (majority_leaf_count.saturating_sub(1), 0u64)
                }
            };

            // Convert AccountId32 to SS58 string
            let provider_ss58 = format!("{}", recommendation.provider);

            // Submit the challenge
            match challenger
                .challenge_checkpoint(
                    bucket_id,
                    provider_ss58,
                    ChunkLocation {
                        leaf_index,
                        chunk_index,
                    },
                )
                .await
            {
                Ok(challenge_id) => {
                    tracing::info!(
                        "Auto-challenge submitted for provider {:?} on bucket {} (confidence: {:.2})",
                        recommendation.provider,
                        bucket_id,
                        recommendation.confidence
                    );

                    result.challenges_submitted.push(SubmittedChallenge {
                        provider: recommendation.provider.clone(),
                        challenge_id,
                        reason: recommendation.reason.clone(),
                        confidence: recommendation.confidence,
                    });

                    // Clear conflict history for this provider since we've challenged them
                    let mut history = self.conflict_history.write().await;
                    history.remove(&(bucket_id, recommendation.provider.clone()));

                    // Update metrics
                    let mut metrics = self.metrics.write().await;
                    metrics.auto_challenge_recommended += 1;
                }
                Err(e) => {
                    tracing::warn!(
                        "Failed to submit auto-challenge for provider {:?}: {}",
                        recommendation.provider,
                        e
                    );

                    result.challenges_failed.push(FailedChallenge {
                        provider: recommendation.provider,
                        reason: recommendation.reason,
                        error: e.to_string(),
                    });
                }
            }
        }

        Ok(result)
    }

    /// Execute auto-challenges for all buckets with tracked conflicts.
    ///
    /// This is a convenience method that iterates over all buckets with
    /// recorded conflicts and executes auto-challenges for each.
    pub async fn execute_all_auto_challenges(
        &self,
        challenger: &ChallengerClient,
        min_confidence: f64,
    ) -> Result<Vec<(BucketId, AutoChallengeResult)>, ClientError> {
        // Get all bucket IDs with conflicts
        let bucket_ids: Vec<BucketId> = {
            let history = self.conflict_history.read().await;
            history
                .keys()
                .map(|(bid, _)| *bid)
                .collect::<std::collections::HashSet<_>>()
                .into_iter()
                .collect()
        };

        let mut results = Vec::new();
        for bucket_id in bucket_ids {
            match self
                .execute_auto_challenges(bucket_id, challenger, min_confidence)
                .await
            {
                Ok(result) => results.push((bucket_id, result)),
                Err(e) => {
                    tracing::warn!(
                        "Failed to execute auto-challenges for bucket {}: {}",
                        bucket_id,
                        e
                    );
                }
            }
        }

        Ok(results)
    }

    // ========================================================================
    // Background Checkpoint Loop
    // ========================================================================

    /// Start a background checkpoint loop for a bucket.
    ///
    /// The loop runs in a background task and periodically submits checkpoints
    /// according to the batched configuration. It respects dirty flags and
    /// handles failures gracefully.
    ///
    /// # Arguments
    ///
    /// * `bucket_id` - The bucket to checkpoint
    /// * `batched_config` - Configuration for the batched loop
    /// * `callback` - Optional callback invoked after each checkpoint attempt
    ///
    /// # Returns
    ///
    /// A `CheckpointLoopHandle` for controlling the background loop.
    ///
    /// # Example
    ///
    /// ```ignore
    /// let handle = manager.start_checkpoint_loop(
    ///     bucket_id,
    ///     BatchedCheckpointConfig::default(),
    ///     Some(Arc::new(|bucket_id, result| {
    ///         println!("Checkpoint for bucket {}: {:?}", bucket_id, result);
    ///     })),
    /// ).await?;
    ///
    /// // Mark changes
    /// handle.mark_dirty(bucket_id).await?;
    ///
    /// // Stop when done
    /// handle.stop().await?;
    /// ```
    pub async fn start_checkpoint_loop(
        self: Arc<Self>,
        bucket_id: BucketId,
        batched_config: BatchedCheckpointConfig,
        callback: Option<CheckpointCallback>,
    ) -> Result<CheckpointLoopHandle, ClientError> {
        let (command_tx, command_rx) = mpsc::channel::<CheckpointLoopCommand>(32);
        let running = Arc::new(AtomicBool::new(true));
        let running_clone = running.clone();
        let manager = self.clone();

        let task_handle = tokio::spawn(async move {
            manager
                .run_checkpoint_loop(
                    bucket_id,
                    batched_config,
                    command_rx,
                    running_clone,
                    callback,
                )
                .await;
        });

        Ok(CheckpointLoopHandle::new(command_tx, running, task_handle))
    }

    /// Internal loop implementation.
    async fn run_checkpoint_loop(
        &self,
        bucket_id: BucketId,
        config: BatchedCheckpointConfig,
        mut command_rx: mpsc::Receiver<CheckpointLoopCommand>,
        running: Arc<AtomicBool>,
        callback: Option<CheckpointCallback>,
    ) {
        let mut status = BucketCheckpointStatus::default();
        let mut paused = false;

        // Calculate interval duration
        let interval_duration = match config.interval {
            BatchedInterval::Duration(d) => d,
            BatchedInterval::Blocks(blocks) => {
                // Assume ~6 second block time
                Duration::from_secs(blocks as u64 * 6)
            }
        };

        let mut interval = tokio::time::interval(interval_duration);
        interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);

        loop {
            tokio::select! {
                // Check for commands
                cmd = command_rx.recv() => {
                    match cmd {
                        Some(CheckpointLoopCommand::SubmitNow) => {
                            // Submit immediately
                            self.execute_checkpoint(
                                bucket_id,
                                &config,
                                &mut status,
                                &callback,
                            ).await;
                        }
                        Some(CheckpointLoopCommand::MarkDirty(bid)) => {
                            if bid == bucket_id {
                                status.dirty = true;
                            }
                        }
                        Some(CheckpointLoopCommand::Pause) => {
                            paused = true;
                        }
                        Some(CheckpointLoopCommand::Resume) => {
                            paused = false;
                        }
                        Some(CheckpointLoopCommand::Stop) | None => {
                            running.store(false, Ordering::SeqCst);
                            break;
                        }
                    }
                }

                // Interval tick
                _ = interval.tick() => {
                    if paused {
                        continue;
                    }

                    // Check if we should checkpoint
                    let should_checkpoint = status.dirty || config.submit_on_empty;

                    if should_checkpoint {
                        // Check if we're in a failure backoff period
                        let in_backoff = status.consecutive_failures > 0 &&
                            status.last_checkpoint.map(|t| t.elapsed() < config.failure_retry_delay).unwrap_or(false);

                        if !in_backoff {
                            self.execute_checkpoint(
                                bucket_id,
                                &config,
                                &mut status,
                                &callback,
                            ).await;
                        }
                    }
                }
            }

            // Check if we should stop due to too many failures
            if status.consecutive_failures >= config.max_consecutive_failures {
                // Pause instead of stopping completely
                paused = true;
            }

            // Check running flag
            if !running.load(Ordering::SeqCst) {
                break;
            }
        }
    }

    /// Execute a single checkpoint submission.
    async fn execute_checkpoint(
        &self,
        bucket_id: BucketId,
        _config: &BatchedCheckpointConfig,
        status: &mut BucketCheckpointStatus,
        callback: &Option<CheckpointCallback>,
    ) {
        let result = self.submit_checkpoint(bucket_id).await;

        // Update status
        status.last_checkpoint = Some(Instant::now());
        status.last_result = Some(result.clone());

        match &result {
            CheckpointResult::Submitted { .. } => {
                status.dirty = false;
                status.consecutive_failures = 0;
            }
            CheckpointResult::InsufficientConsensus { .. } => {
                // Keep dirty flag, increment failures
                status.consecutive_failures += 1;
            }
            CheckpointResult::ProvidersUnreachable { .. } => {
                status.consecutive_failures += 1;
            }
            CheckpointResult::NoProviders => {
                status.consecutive_failures += 1;
            }
            CheckpointResult::TransactionFailed { .. } => {
                status.consecutive_failures += 1;
            }
        }

        // Invoke callback if provided
        if let Some(cb) = callback {
            cb(bucket_id, &result);
        }
    }

    // ========================================================================
    // State Persistence (Phase 3+)
    // ========================================================================

    /// Save the current state to a persistence file.
    ///
    /// This saves metrics, provider health history, and conflict history
    /// so they can be restored after a restart.
    ///
    /// # Example
    ///
    /// ```no_run
    /// use storage_client::{CheckpointManager, CheckpointConfig, PersistenceConfig};
    ///
    /// # async fn example() -> Result<(), Box<dyn std::error::Error>> {
    /// let manager = CheckpointManager::new("ws://localhost:2222", CheckpointConfig::default()).await?;
    ///
    /// // Save state to file
    /// manager.save_state("/var/lib/storage/checkpoint_state.json").await?;
    /// # Ok(())
    /// # }
    /// ```
    pub async fn save_state(&self, path: impl Into<std::path::PathBuf>) -> Result<(), ClientError> {
        let persistence = CheckpointPersistence::new(PersistenceConfig::new(path));
        let state = self.export_state().await;
        persistence.save(&state).await
    }

    /// Restore state from a persistence file.
    ///
    /// This restores metrics, provider health history, and conflict history
    /// from a previous session.
    ///
    /// # Example
    ///
    /// ```no_run
    /// use storage_client::{CheckpointManager, CheckpointConfig};
    ///
    /// # async fn example() -> Result<(), Box<dyn std::error::Error>> {
    /// let manager = CheckpointManager::new("ws://localhost:2222", CheckpointConfig::default()).await?;
    ///
    /// // Restore state from file
    /// manager.restore_state("/var/lib/storage/checkpoint_state.json").await?;
    /// # Ok(())
    /// # }
    /// ```
    pub async fn restore_state(
        &self,
        path: impl Into<std::path::PathBuf>,
    ) -> Result<(), ClientError> {
        let persistence = CheckpointPersistence::new(PersistenceConfig::new(path));
        let state = persistence.load().await?;
        self.import_state(&state).await
    }

    /// Export current state as a PersistedCheckpointState.
    ///
    /// Useful for custom persistence implementations or debugging.
    pub async fn export_state(&self) -> PersistedCheckpointState {
        let health_histories = self.health_history.read().await;
        let metrics = self.metrics.read().await;

        StateBuilder::new()
            .with_health_histories(&health_histories)
            .with_metrics(&metrics)
            .build()
    }

    /// Import state from a PersistedCheckpointState.
    ///
    /// Restores health histories and metrics from a persisted state.
    pub async fn import_state(&self, state: &PersistedCheckpointState) -> Result<(), ClientError> {
        // Restore health histories
        {
            let mut health_histories = self.health_history.write().await;
            for persisted in state.health_histories.values() {
                let history = persisted.to_health_history()?;
                health_histories.insert(history.account_id.clone(), history);
            }
        }

        // Restore metrics
        {
            let mut metrics = self.metrics.write().await;
            *metrics = state.metrics.to_checkpoint_metrics();
        }

        tracing::info!(
            "Restored checkpoint state: {} health histories, {} total attempts",
            state.health_histories.len(),
            state.metrics.total_attempts
        );

        Ok(())
    }

    /// Get a copy of all provider health histories.
    ///
    /// Useful for monitoring and diagnostics.
    pub async fn get_health_histories(&self) -> HashMap<AccountId32, ProviderHealthHistory> {
        self.health_history.read().await.clone()
    }

    /// Get health history for a specific provider.
    pub async fn get_provider_health(
        &self,
        provider: &AccountId32,
    ) -> Option<ProviderHealthHistory> {
        self.health_history.read().await.get(provider).cloned()
    }

    /// Clear all conflict history for all buckets.
    ///
    /// Useful after resolving conflicts or starting fresh.
    pub async fn clear_all_conflict_history(&self) {
        self.conflict_history.write().await.clear();
    }

    /// Get the total number of recorded conflicts for a bucket across all providers.
    pub async fn get_bucket_conflict_count(&self, bucket_id: BucketId) -> usize {
        let history = self.conflict_history.read().await;
        history
            .iter()
            .filter(|((bid, _), _)| *bid == bucket_id)
            .map(|(_, conflicts)| conflicts.len())
            .sum()
    }
}

// ============================================================================
// Convenience Functions
// ============================================================================

/// Quick helper to submit a checkpoint for a bucket.
///
/// Creates a temporary CheckpointManager and submits the checkpoint.
pub async fn submit_checkpoint_simple(
    chain_endpoint: &str,
    bucket_id: BucketId,
    provider_endpoints: Vec<String>,
    signer: crate::Signer,
) -> CheckpointResult {
    let manager = match CheckpointManager::new(chain_endpoint, CheckpointConfig::default()).await {
        Ok(m) => m,
        Err(e) => {
            return CheckpointResult::TransactionFailed {
                error: e.to_string(),
            }
        }
    };

    let manager = manager
        .with_providers(provider_endpoints)
        .with_signer(signer);

    manager.submit_checkpoint(bucket_id).await
}

#[cfg(test)]
mod tests {
    use super::*;

    // ========================================================================
    // CheckpointConfig Tests
    // ========================================================================

    #[test]
    fn test_default_config() {
        let config = CheckpointConfig::default();
        assert_eq!(config.provider_timeout, Duration::from_secs(30));
        assert_eq!(config.max_retries, 3);
        assert_eq!(config.consensus_threshold_percent, 51);
        assert_eq!(config.retry_delay, Duration::from_secs(2));
        assert_eq!(config.provider_cache_ttl, Duration::from_secs(300));
    }

    // ========================================================================
    // BatchedCheckpointConfig Tests
    // ========================================================================

    #[test]
    fn test_batched_config_default() {
        let config = BatchedCheckpointConfig::default();
        assert!(!config.submit_on_empty);
        assert_eq!(config.max_consecutive_failures, 5);
        assert_eq!(config.failure_retry_delay, Duration::from_secs(30));

        match config.interval {
            BatchedInterval::Blocks(blocks) => assert_eq!(blocks, 100),
            _ => panic!("Expected Blocks interval"),
        }
    }

    #[test]
    fn test_batched_interval_blocks() {
        let interval = BatchedInterval::Blocks(50);
        match interval {
            BatchedInterval::Blocks(b) => assert_eq!(b, 50),
            _ => panic!("Expected Blocks"),
        }
    }

    #[test]
    fn test_batched_interval_duration() {
        let interval = BatchedInterval::Duration(Duration::from_secs(120));
        match interval {
            BatchedInterval::Duration(d) => assert_eq!(d, Duration::from_secs(120)),
            _ => panic!("Expected Duration"),
        }
    }

    // ========================================================================
    // ProviderHealthHistory Tests
    // ========================================================================

    #[test]
    fn test_health_history_new() {
        let account = AccountId32::new([1u8; 32]);
        let history = ProviderHealthHistory::new(account.clone());

        assert_eq!(history.account_id, account);
        assert_eq!(history.total_requests, 0);
        assert_eq!(history.successful_requests, 0);
        assert_eq!(history.failed_requests, 0);
        assert_eq!(history.avg_response_time_ms, 0);
        assert!(history.recent_statuses.is_empty());
        assert!(history.last_success.is_none());
        assert!(history.last_failure.is_none());
        assert_eq!(history.consecutive_failures, 0);
    }

    #[test]
    fn test_health_history_record_success() {
        let account = AccountId32::new([1u8; 32]);
        let mut history = ProviderHealthHistory::new(account);

        history.record_success(100);
        assert_eq!(history.total_requests, 1);
        assert_eq!(history.successful_requests, 1);
        assert_eq!(history.failed_requests, 0);
        assert_eq!(history.avg_response_time_ms, 100);
        assert_eq!(history.consecutive_failures, 0);
        assert!(history.last_success.is_some());

        history.record_success(200);
        assert_eq!(history.total_requests, 2);
        assert_eq!(history.successful_requests, 2);
        assert_eq!(history.avg_response_time_ms, 150); // (100 + 200) / 2
    }

    #[test]
    fn test_health_history_record_failure() {
        let account = AccountId32::new([1u8; 32]);
        let mut history = ProviderHealthHistory::new(account);

        history.record_failure("Connection refused".to_string());
        assert_eq!(history.total_requests, 1);
        assert_eq!(history.successful_requests, 0);
        assert_eq!(history.failed_requests, 1);
        assert_eq!(history.consecutive_failures, 1);
        assert!(history.last_failure.is_some());

        history.record_failure("Timeout".to_string());
        assert_eq!(history.consecutive_failures, 2);
    }

    #[test]
    fn test_health_history_success_resets_consecutive_failures() {
        let account = AccountId32::new([1u8; 32]);
        let mut history = ProviderHealthHistory::new(account);

        history.record_failure("Error 1".to_string());
        history.record_failure("Error 2".to_string());
        assert_eq!(history.consecutive_failures, 2);

        history.record_success(50);
        assert_eq!(history.consecutive_failures, 0);
    }

    #[test]
    fn test_health_history_success_rate() {
        let account = AccountId32::new([1u8; 32]);
        let mut history = ProviderHealthHistory::new(account);

        // No requests = 100% success rate (optimistic default)
        assert_eq!(history.success_rate(), 1.0);

        // 1 success, 0 failures = 100%
        history.record_success(100);
        assert_eq!(history.success_rate(), 1.0);

        // 1 success, 1 failure = 50%
        history.record_failure("Error".to_string());
        assert_eq!(history.success_rate(), 0.5);

        // 2 successes, 2 failures = 50%
        history.record_success(100);
        history.record_failure("Error".to_string());
        assert_eq!(history.success_rate(), 0.5);

        // 3 successes, 2 failures = 60%
        history.record_success(100);
        assert_eq!(history.success_rate(), 0.6);
    }

    #[test]
    fn test_health_history_is_healthy() {
        let account = AccountId32::new([1u8; 32]);
        let mut history = ProviderHealthHistory::new(account);

        // New provider with no history is considered healthy
        // (success_rate returns 1.0 for 0 requests)
        assert!(history.is_healthy());

        // After some successes, still healthy
        history.record_success(100);
        history.record_success(100);
        history.record_success(100);
        history.record_success(100);
        assert!(history.is_healthy());

        // After 3 consecutive failures, not healthy
        history.record_failure("Error 1".to_string());
        history.record_failure("Error 2".to_string());
        history.record_failure("Error 3".to_string());
        assert!(!history.is_healthy());
    }

    #[test]
    fn test_health_history_current_status() {
        let account = AccountId32::new([1u8; 32]);
        let mut history = ProviderHealthHistory::new(account);

        // Unknown status when no requests
        assert_eq!(history.current_status(), ProviderStatus::Unknown);

        // Healthy after success
        history.record_success(100);
        assert_eq!(history.current_status(), ProviderStatus::Healthy);

        // Degraded after some failures
        history.record_failure("Error".to_string());
        match history.current_status() {
            ProviderStatus::Degraded { .. } => {}
            _ => panic!("Expected Degraded status"),
        }

        // Unreachable after 5 consecutive failures
        for _ in 0..5 {
            history.record_failure("Error".to_string());
        }
        match history.current_status() {
            ProviderStatus::Unreachable { .. } => {}
            _ => panic!("Expected Unreachable status"),
        }
    }

    #[test]
    fn test_health_history_recent_statuses_limit() {
        let account = AccountId32::new([1u8; 32]);
        let mut history = ProviderHealthHistory::new(account);

        // Add 15 entries
        for i in 0..15 {
            if i % 2 == 0 {
                history.record_success(100);
            } else {
                history.record_failure("Error".to_string());
            }
        }

        // Should only keep last 10
        assert_eq!(history.recent_statuses.len(), 10);
    }

    // ========================================================================
    // BucketCheckpointStatus Tests
    // ========================================================================

    #[test]
    fn test_bucket_status_default() {
        let status = BucketCheckpointStatus::default();
        assert!(!status.dirty);
        assert!(status.last_checkpoint.is_none());
        assert!(status.last_result.is_none());
        assert_eq!(status.consecutive_failures, 0);
    }

    // ========================================================================
    // ProviderStatus Tests
    // ========================================================================

    #[test]
    fn test_provider_status_equality() {
        assert_eq!(ProviderStatus::Healthy, ProviderStatus::Healthy);
        assert_eq!(ProviderStatus::Unknown, ProviderStatus::Unknown);

        let degraded1 = ProviderStatus::Degraded {
            last_error: "Error".to_string(),
        };
        let degraded2 = ProviderStatus::Degraded {
            last_error: "Error".to_string(),
        };
        assert_eq!(degraded1, degraded2);
    }

    // ========================================================================
    // ConflictType Tests
    // ========================================================================

    #[test]
    fn test_conflict_type_equality() {
        assert_eq!(ConflictType::DataDivergence, ConflictType::DataDivergence);
        assert_eq!(
            ConflictType::SyncDelay { behind_by: 5 },
            ConflictType::SyncDelay { behind_by: 5 }
        );
        assert_eq!(
            ConflictType::Ahead { ahead_by: 3 },
            ConflictType::Ahead { ahead_by: 3 }
        );
        assert_ne!(
            ConflictType::SyncDelay { behind_by: 5 },
            ConflictType::SyncDelay { behind_by: 10 }
        );
    }

    // ========================================================================
    // ConflictResolution Tests
    // ========================================================================

    #[test]
    fn test_conflict_resolution_equality() {
        assert_eq!(
            ConflictResolution::ProceedWithMajority,
            ConflictResolution::ProceedWithMajority
        );
        assert_eq!(
            ConflictResolution::WaitForSync {
                estimated_blocks: 10
            },
            ConflictResolution::WaitForSync {
                estimated_blocks: 10
            }
        );

        let account = AccountId32::new([1u8; 32]);
        assert_eq!(
            ConflictResolution::ConsiderChallenge {
                provider: account.clone()
            },
            ConflictResolution::ConsiderChallenge { provider: account }
        );
    }

    // ========================================================================
    // CommitmentCollection Tests
    // ========================================================================

    #[test]
    fn test_commitment_collection_clone() {
        let collection = CommitmentCollection {
            bucket_id: 1,
            mmr_root: H256::zero(),
            start_seq: 0,
            leaf_count: 10,
            nonce: 42,
            signatures: vec![],
            agreeing_providers: vec![],
            disagreeing_providers: vec![],
            unreachable_providers: vec![],
        };

        let cloned = collection.clone();
        assert_eq!(cloned.bucket_id, 1);
        assert_eq!(cloned.leaf_count, 10);
        assert_eq!(cloned.nonce, 42);
    }

    fn test_mmr_root() -> String {
        format!("0x{}", hex::encode([7u8; 32]))
    }

    fn valid_signature_hex() -> String {
        format!("0x{}", hex::encode([9u8; SR25519_SIGNATURE_LEN]))
    }

    #[test]
    fn modal_payload_group_picks_largest_group() {
        let a = AccountId32::new([1u8; 32]);
        let b = AccountId32::new([2u8; 32]);
        let c = AccountId32::new([3u8; 32]);

        let mut groups = SigsByPayload::new();
        groups.insert(
            PayloadKey::new(0, 10, 100),
            vec![(a, vec![1]), (b, vec![2])],
        );
        groups.insert(PayloadKey::new(0, 20, 100), vec![(c, vec![3])]);

        let (key, sigs) = modal_payload_group(groups);
        assert_eq!(key, PayloadKey::new(0, 10, 100));
        assert_eq!(sigs.len(), 2);
    }

    #[test]
    fn modal_payload_group_tie_breaks_on_larger_range() {
        let a = AccountId32::new([1u8; 32]);
        let b = AccountId32::new([2u8; 32]);

        let mut groups = SigsByPayload::new();
        groups.insert(PayloadKey::new(0, 10, 100), vec![(a, vec![1])]);
        groups.insert(PayloadKey::new(0, 20, 100), vec![(b, vec![2])]);

        let (key, _) = modal_payload_group(groups);
        assert_eq!(
            key.leaf_count, 20,
            "a tie must never install the shorter, less challengeable range"
        );
    }

    #[test]
    fn modal_payload_group_empty_is_zeroed() {
        let (key, sigs) = modal_payload_group(SigsByPayload::new());
        assert_eq!(key, PayloadKey::default());
        assert!(sigs.is_empty());
    }

    #[test]
    fn validated_signature_accepts_matching_response() {
        let account = AccountId32::new([1u8; 32]);
        let response =
            CommitmentResponse::new(1, test_mmr_root(), 5, 10, valid_signature_hex(), 42);

        let sig = CheckpointManager::validated_signature(1, &account, &response, 42, Some((5, 10)));
        assert_eq!(sig, Some(vec![9u8; SR25519_SIGNATURE_LEN]));
    }

    #[test]
    fn validated_signature_rejects_nonce_mismatch() {
        let account = AccountId32::new([1u8; 32]);
        // A provider echoing its own nonce cannot join a batch signed over ours.
        let response =
            CommitmentResponse::new(1, test_mmr_root(), 5, 10, valid_signature_hex(), 999);

        assert!(CheckpointManager::validated_signature(1, &account, &response, 42, None).is_none());
    }

    #[test]
    fn validated_signature_rejects_range_regression() {
        let account = AccountId32::new([1u8; 32]);

        let shrunk_leaves =
            CommitmentResponse::new(1, test_mmr_root(), 5, 9, valid_signature_hex(), 42);
        assert!(
            CheckpointManager::validated_signature(1, &account, &shrunk_leaves, 42, Some((5, 10)))
                .is_none(),
            "leaf_count below the on-chain snapshot must be refused"
        );

        let rewound_start =
            CommitmentResponse::new(1, test_mmr_root(), 4, 10, valid_signature_hex(), 42);
        assert!(CheckpointManager::validated_signature(
            1,
            &account,
            &rewound_start,
            42,
            Some((5, 10))
        )
        .is_none());
    }

    #[test]
    fn validated_signature_rejects_unusable_signatures() {
        let account = AccountId32::new([1u8; 32]);

        let undecodable =
            CommitmentResponse::new(1, test_mmr_root(), 5, 10, "not-hex".to_string(), 42);
        assert!(
            CheckpointManager::validated_signature(1, &account, &undecodable, 42, None).is_none()
        );

        let wrong_length = CommitmentResponse::new(
            1,
            test_mmr_root(),
            5,
            10,
            format!("0x{}", hex::encode([9u8; 63])),
            42,
        );
        assert!(
            CheckpointManager::validated_signature(1, &account, &wrong_length, 42, None).is_none(),
            "a short signature must not reach the extrinsic builder"
        );
    }

    // ========================================================================
    // ProviderInfo Tests
    // ========================================================================

    #[test]
    fn test_provider_info_clone() {
        let account = AccountId32::new([1u8; 32]);
        let info = ProviderInfo {
            account_id: account.clone(),
            endpoint: "http://localhost:3333".to_string(),
            public_key: vec![1, 2, 3],
            last_seen: None,
            status: ProviderStatus::Healthy,
        };

        let cloned = info.clone();
        assert_eq!(cloned.account_id, account);
        assert_eq!(cloned.endpoint, "http://localhost:3333");
        assert_eq!(cloned.public_key, vec![1, 2, 3]);
    }

    // ========================================================================
    // CheckpointResult Tests
    // ========================================================================

    #[test]
    fn test_checkpoint_result_variants() {
        let submitted = CheckpointResult::Submitted {
            block_hash: H256::zero(),
            signers: vec![],
        };

        let insufficient = CheckpointResult::InsufficientConsensus {
            agreeing: 1,
            required: 2,
            disagreements: vec![],
        };

        let unreachable = CheckpointResult::ProvidersUnreachable { providers: vec![] };

        let no_providers = CheckpointResult::NoProviders;

        let failed = CheckpointResult::TransactionFailed {
            error: "Test error".to_string(),
        };

        // Test that all variants can be cloned
        let _ = submitted.clone();
        let _ = insufficient.clone();
        let _ = unreachable.clone();
        let _ = no_providers.clone();
        let _ = failed.clone();
    }

    // ========================================================================
    // Multiaddr Parsing Tests (via CheckpointManager helper)
    // ========================================================================

    fn parse_ma(s: &str) -> Result<String, ClientError> {
        CheckpointManager::parse_multiaddr_to_http(s.as_bytes())
    }

    #[test]
    fn multiaddr_ip4_is_http() {
        assert_eq!(
            parse_ma("/ip4/127.0.0.1/tcp/3333").unwrap(),
            "http://127.0.0.1:3333"
        );
    }

    #[test]
    fn multiaddr_ip6_host_is_bracketed() {
        assert_eq!(parse_ma("/ip6/::1/tcp/3333").unwrap(), "http://[::1]:3333");
    }

    #[test]
    fn multiaddr_default_http_port_elided() {
        assert_eq!(
            parse_ma("/ip4/127.0.0.1/tcp/80").unwrap(),
            "http://127.0.0.1"
        );
    }

    #[test]
    fn multiaddr_tls_http_is_https_with_default_port_elided() {
        assert_eq!(
            parse_ma("/dns4/host.example.com/tcp/443/tls/http").unwrap(),
            "https://host.example.com"
        );
    }

    #[test]
    fn multiaddr_tls_keeps_non_default_port() {
        assert_eq!(
            parse_ma("/dns4/host.example.com/tcp/8443/tls/http").unwrap(),
            "https://host.example.com:8443"
        );
    }

    #[test]
    fn multiaddr_http_path_is_appended_the_hosted_provider_form() {
        // The exact form the PreviewNet preset / `--public-multiaddr` advertise.
        assert_eq!(
            parse_ma(
                "/dns4/previewnet.substrate.dev/tcp/443/tls/http/http-path/web3-storage-provider"
            )
            .unwrap(),
            "https://previewnet.substrate.dev/web3-storage-provider"
        );
    }

    #[test]
    fn multiaddr_http_path_is_percent_decoded() {
        assert_eq!(
            parse_ma("/dns4/host.example.com/tcp/443/tls/http/http-path/a%2fb%2fc").unwrap(),
            "https://host.example.com/a/b/c"
        );
    }

    #[test]
    fn multiaddr_without_port_is_rejected() {
        assert!(parse_ma("/ip4/127.0.0.1").is_err());
    }

    #[test]
    fn multiaddr_unsupported_protocol_is_rejected() {
        assert!(parse_ma("/unix/var/run/sock/tcp/3333").is_err());
    }

    // ========================================================================
    // Phase 3: Metrics Tests
    // ========================================================================

    #[test]
    fn test_checkpoint_metrics_default() {
        let metrics = CheckpointMetrics::default();
        assert_eq!(metrics.total_attempts, 0);
        assert_eq!(metrics.successful_submissions, 0);
        assert_eq!(metrics.insufficient_consensus_count, 0);
        assert_eq!(metrics.unreachable_failures, 0);
        assert_eq!(metrics.transaction_failures, 0);
        assert_eq!(metrics.conflicts_detected, 0);
        assert_eq!(metrics.success_rate(), 1.0); // No attempts = 100%
    }

    #[test]
    fn test_checkpoint_metrics_record_success() {
        let mut metrics = CheckpointMetrics::default();

        let result = CheckpointResult::Submitted {
            block_hash: H256::zero(),
            signers: vec![AccountId32::new([1u8; 32]), AccountId32::new([2u8; 32])],
        };

        metrics.record_attempt(&result, 100);

        assert_eq!(metrics.total_attempts, 1);
        assert_eq!(metrics.successful_submissions, 1);
        assert_eq!(metrics.providers_responded, 2);
        assert_eq!(metrics.success_rate(), 1.0);
    }

    #[test]
    fn test_checkpoint_metrics_record_failures() {
        let mut metrics = CheckpointMetrics::default();

        // Record insufficient consensus
        let result1 = CheckpointResult::InsufficientConsensus {
            agreeing: 1,
            required: 2,
            disagreements: vec![],
        };
        metrics.record_attempt(&result1, 50);
        assert_eq!(metrics.insufficient_consensus_count, 1);

        // Record unreachable
        let result2 = CheckpointResult::ProvidersUnreachable {
            providers: vec![AccountId32::new([1u8; 32])],
        };
        metrics.record_attempt(&result2, 30);
        assert_eq!(metrics.unreachable_failures, 1);

        // Record transaction failure
        let result3 = CheckpointResult::TransactionFailed {
            error: "Test".to_string(),
        };
        metrics.record_attempt(&result3, 20);
        assert_eq!(metrics.transaction_failures, 1);

        // Check totals
        assert_eq!(metrics.total_attempts, 3);
        assert_eq!(metrics.successful_submissions, 0);
        assert_eq!(metrics.success_rate(), 0.0);
    }

    #[test]
    fn test_checkpoint_metrics_record_conflict() {
        let mut metrics = CheckpointMetrics::default();

        let conflict = ProviderConflict {
            bucket_id: 1,
            majority_root: H256::zero(),
            majority_count: 2,
            conflicts: vec![ConflictingProvider {
                account_id: AccountId32::new([1u8; 32]),
                mmr_root: H256::repeat_byte(0x11),
                leaf_count: 10,
                conflict_type: ConflictType::DataDivergence,
            }],
            detected_at: Instant::now(),
            resolution: ConflictResolution::ConsiderChallenge {
                provider: AccountId32::new([1u8; 32]),
            },
        };

        metrics.record_conflict(&conflict);

        assert_eq!(metrics.conflicts_detected, 1);
        assert_eq!(metrics.auto_challenge_recommended, 1);
    }

    // ========================================================================
    // Phase 3: Auto-Challenge Config Tests
    // ========================================================================

    #[test]
    fn test_auto_challenge_config_default() {
        let config = AutoChallengeConfig::default();
        assert!(!config.enabled); // Disabled by default
        assert_eq!(config.min_conflict_count, 3);
        assert_eq!(config.sync_wait_duration, Duration::from_secs(60));
        assert!(config.challenge_on_divergence);
    }

    // ========================================================================
    // Phase 3: Challenge Reason Tests
    // ========================================================================

    #[test]
    fn test_challenge_reason_data_divergence() {
        let reason = ChallengeReason::DataDivergence {
            majority_root: H256::zero(),
            provider_root: H256::repeat_byte(0x11),
            leaf_count: 100,
        };

        match reason {
            ChallengeReason::DataDivergence { leaf_count, .. } => {
                assert_eq!(leaf_count, 100);
            }
            _ => panic!("Expected DataDivergence"),
        }
    }

    #[test]
    fn test_challenge_reason_persistently_syncing() {
        let reason = ChallengeReason::PersistentlySyncing {
            behind_by: 50,
            duration: Duration::from_secs(120),
        };

        match reason {
            ChallengeReason::PersistentlySyncing {
                behind_by,
                duration,
            } => {
                assert_eq!(behind_by, 50);
                assert_eq!(duration, Duration::from_secs(120));
            }
            _ => panic!("Expected PersistentlySyncing"),
        }
    }

    #[test]
    fn test_challenge_evidence() {
        let evidence = ChallengeEvidence {
            bucket_id: 1,
            majority_commitment: (H256::zero(), 0, 100),
            majority_signatures: vec![],
            provider_commitment: Some((H256::repeat_byte(0x11), 0, 100)),
            observation_times: vec![Instant::now()],
        };

        assert_eq!(evidence.bucket_id, 1);
        assert!(evidence.provider_commitment.is_some());
        assert_eq!(evidence.observation_times.len(), 1);
    }

    // ========================================================================
    // Auto-Challenge Execution Tests
    // ========================================================================

    #[test]
    fn test_auto_challenge_result_empty() {
        let result = AutoChallengeResult {
            providers_analyzed: 5,
            challenges_submitted: vec![],
            challenges_failed: vec![],
            providers_skipped: 3,
        };

        assert_eq!(result.providers_analyzed, 5);
        assert!(result.challenges_submitted.is_empty());
        assert!(result.challenges_failed.is_empty());
        assert_eq!(result.providers_skipped, 3);
    }

    #[test]
    fn test_submitted_challenge() {
        let challenge = SubmittedChallenge {
            provider: AccountId32::new([1u8; 32]),
            challenge_id: ChallengeId {
                deadline: 1000,
                index: 1,
            },
            reason: ChallengeReason::DataDivergence {
                majority_root: H256::zero(),
                provider_root: H256::repeat_byte(0x11),
                leaf_count: 100,
            },
            confidence: 0.9,
        };

        assert_eq!(challenge.challenge_id.deadline, 1000);
        assert_eq!(challenge.challenge_id.index, 1);
        assert_eq!(challenge.confidence, 0.9);
    }

    #[test]
    fn test_failed_challenge() {
        let failed = FailedChallenge {
            provider: AccountId32::new([2u8; 32]),
            reason: ChallengeReason::PersistentlySyncing {
                behind_by: 10,
                duration: Duration::from_secs(60),
            },
            error: "Transaction failed".to_string(),
        };

        assert_eq!(failed.error, "Transaction failed");
        match failed.reason {
            ChallengeReason::PersistentlySyncing { behind_by, .. } => {
                assert_eq!(behind_by, 10);
            }
            _ => panic!("Expected PersistentlySyncing"),
        }
    }

    #[test]
    fn test_auto_challenge_result_with_submissions() {
        let result = AutoChallengeResult {
            providers_analyzed: 10,
            challenges_submitted: vec![
                SubmittedChallenge {
                    provider: AccountId32::new([1u8; 32]),
                    challenge_id: ChallengeId {
                        deadline: 1000,
                        index: 0,
                    },
                    reason: ChallengeReason::DataDivergence {
                        majority_root: H256::zero(),
                        provider_root: H256::repeat_byte(0x11),
                        leaf_count: 100,
                    },
                    confidence: 0.95,
                },
                SubmittedChallenge {
                    provider: AccountId32::new([2u8; 32]),
                    challenge_id: ChallengeId {
                        deadline: 1000,
                        index: 1,
                    },
                    reason: ChallengeReason::DataDivergence {
                        majority_root: H256::zero(),
                        provider_root: H256::repeat_byte(0x22),
                        leaf_count: 100,
                    },
                    confidence: 0.85,
                },
            ],
            challenges_failed: vec![FailedChallenge {
                provider: AccountId32::new([3u8; 32]),
                reason: ChallengeReason::PersistentlySyncing {
                    behind_by: 5,
                    duration: Duration::from_secs(120),
                },
                error: "Insufficient funds".to_string(),
            }],
            providers_skipped: 7,
        };

        assert_eq!(result.providers_analyzed, 10);
        assert_eq!(result.challenges_submitted.len(), 2);
        assert_eq!(result.challenges_failed.len(), 1);
        assert_eq!(result.providers_skipped, 7);

        // Verify first submitted challenge
        assert_eq!(result.challenges_submitted[0].confidence, 0.95);

        // Verify failed challenge error
        assert_eq!(result.challenges_failed[0].error, "Insufficient funds");
    }

    // ========================================================================
    // CheckpointMetrics Rolling Average Tests
    // ========================================================================

    #[test]
    fn test_metrics_rolling_average_first_submission() {
        let mut metrics = CheckpointMetrics::default();
        let result = CheckpointResult::Submitted {
            block_hash: H256::zero(),
            signers: vec![AccountId32::new([1u8; 32])],
        };

        metrics.record_attempt(&result, 200);

        assert_eq!(metrics.successful_submissions, 1);
        assert_eq!(metrics.avg_submission_time_ms, 200);
    }

    #[test]
    fn test_metrics_rolling_average_multiple_submissions() {
        let mut metrics = CheckpointMetrics::default();
        let result = CheckpointResult::Submitted {
            block_hash: H256::zero(),
            signers: vec![AccountId32::new([1u8; 32])],
        };

        metrics.record_attempt(&result, 100);
        assert_eq!(metrics.avg_submission_time_ms, 100);

        metrics.record_attempt(&result, 200);
        assert_eq!(metrics.avg_submission_time_ms, 150); // (100 + 200) / 2

        metrics.record_attempt(&result, 300);
        assert_eq!(metrics.avg_submission_time_ms, 200); // (150*2 + 300) / 3 = 200
    }

    #[test]
    fn test_metrics_rolling_average_ignores_failures() {
        let mut metrics = CheckpointMetrics::default();
        let success = CheckpointResult::Submitted {
            block_hash: H256::zero(),
            signers: vec![AccountId32::new([1u8; 32])],
        };
        let failure = CheckpointResult::TransactionFailed {
            error: "timeout".to_string(),
        };

        metrics.record_attempt(&success, 100);
        assert_eq!(metrics.avg_submission_time_ms, 100);

        // Failure should not change the submission time average
        metrics.record_attempt(&failure, 5000);
        assert_eq!(metrics.avg_submission_time_ms, 100);

        metrics.record_attempt(&success, 200);
        assert_eq!(metrics.avg_submission_time_ms, 150);
    }
}
