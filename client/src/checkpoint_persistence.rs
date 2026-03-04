//! Checkpoint Persistence Module
//!
//! Provides persistence for checkpoint state across restarts, including:
//! - Provider health history
//! - Checkpoint metrics
//! - Bucket checkpoint status
//! - Conflict history for auto-challenge analysis
//!
//! # Example
//!
//! ```no_run
//! use storage_client::checkpoint_persistence::{CheckpointPersistence, PersistenceConfig};
//!
//! # async fn example() -> Result<(), Box<dyn std::error::Error>> {
//! let config = PersistenceConfig::new("/var/lib/storage/checkpoints.json");
//! let persistence = CheckpointPersistence::new(config);
//!
//! // Load existing state
//! let state = persistence.load().await?;
//!
//! // ... use state in CheckpointManager ...
//!
//! // Save state periodically
//! persistence.save(&state).await?;
//! # Ok(())
//! # }
//! ```

use crate::checkpoint::{
    BucketCheckpointStatus, CheckpointMetrics, CheckpointResult, ProviderHealthHistory,
};
use crate::ClientError;
use serde::{Deserialize, Serialize};
use sp_runtime::AccountId32;
use std::collections::{HashMap, VecDeque};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};
use storage_primitives::BucketId;
use tokio::fs;
use tokio::sync::RwLock;

// ============================================================================
// Configuration
// ============================================================================

/// Configuration for checkpoint persistence.
#[derive(Clone, Debug)]
pub struct PersistenceConfig {
    /// Path to the persistence file.
    pub file_path: PathBuf,
    /// Whether to auto-save on state changes.
    pub auto_save: bool,
    /// Interval for auto-save (if enabled).
    pub auto_save_interval: Duration,
    /// Whether to compress the persistence file.
    pub compress: bool,
    /// Maximum backup files to keep.
    pub max_backups: u32,
}

impl Default for PersistenceConfig {
    fn default() -> Self {
        Self {
            file_path: PathBuf::from("checkpoint_state.json"),
            auto_save: true,
            auto_save_interval: Duration::from_secs(60),
            compress: false,
            max_backups: 3,
        }
    }
}

impl PersistenceConfig {
    /// Create a new config with the given file path.
    pub fn new(file_path: impl Into<PathBuf>) -> Self {
        Self {
            file_path: file_path.into(),
            ..Default::default()
        }
    }

    /// Set auto-save interval.
    pub fn with_auto_save_interval(mut self, interval: Duration) -> Self {
        self.auto_save_interval = interval;
        self
    }

    /// Disable auto-save.
    pub fn without_auto_save(mut self) -> Self {
        self.auto_save = false;
        self
    }

    /// Set maximum backup files.
    pub fn with_max_backups(mut self, max: u32) -> Self {
        self.max_backups = max;
        self
    }
}

// ============================================================================
// Serializable State Types
// ============================================================================

/// Serializable version of the complete checkpoint state.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct PersistedCheckpointState {
    /// Version for forward compatibility.
    pub version: u32,
    /// When this state was saved.
    pub saved_at: u64,
    /// Provider health histories.
    pub health_histories: HashMap<String, PersistedHealthHistory>,
    /// Checkpoint metrics.
    pub metrics: PersistedMetrics,
    /// Bucket checkpoint statuses.
    pub bucket_statuses: HashMap<BucketId, PersistedBucketStatus>,
    /// Conflict history for auto-challenge.
    pub conflict_history: HashMap<String, Vec<PersistedConflict>>,
}

impl PersistedCheckpointState {
    /// Create a new empty state.
    pub fn new() -> Self {
        Self {
            version: 1,
            saved_at: current_timestamp(),
            ..Default::default()
        }
    }
}

/// Serializable health history for a provider.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct PersistedHealthHistory {
    /// Provider account ID (SS58 encoded).
    pub account_id: String,
    /// Total number of requests made.
    pub total_requests: u64,
    /// Number of successful requests.
    pub successful_requests: u64,
    /// Number of failed requests.
    pub failed_requests: u64,
    /// Average response time in milliseconds.
    pub avg_response_time_ms: u64,
    /// Current consecutive failures.
    pub consecutive_failures: u32,
    /// Last success timestamp (Unix epoch seconds).
    pub last_success_timestamp: Option<u64>,
    /// Last failure timestamp (Unix epoch seconds).
    pub last_failure_timestamp: Option<u64>,
}

impl From<&ProviderHealthHistory> for PersistedHealthHistory {
    fn from(history: &ProviderHealthHistory) -> Self {
        Self {
            account_id: account_id_to_string(&history.account_id),
            total_requests: history.total_requests,
            successful_requests: history.successful_requests,
            failed_requests: history.failed_requests,
            avg_response_time_ms: history.avg_response_time_ms,
            consecutive_failures: history.consecutive_failures,
            last_success_timestamp: history.last_success.map(|_| current_timestamp()),
            last_failure_timestamp: history.last_failure.map(|_| current_timestamp()),
        }
    }
}

impl PersistedHealthHistory {
    /// Convert back to ProviderHealthHistory.
    pub fn to_health_history(&self) -> Result<ProviderHealthHistory, ClientError> {
        let account_id = string_to_account_id(&self.account_id)?;

        Ok(ProviderHealthHistory {
            account_id,
            total_requests: self.total_requests,
            successful_requests: self.successful_requests,
            failed_requests: self.failed_requests,
            avg_response_time_ms: self.avg_response_time_ms,
            recent_statuses: VecDeque::new(), // Not persisted - rebuilt at runtime
            last_success: self.last_success_timestamp.map(|_| Instant::now()),
            last_failure: self.last_failure_timestamp.map(|_| Instant::now()),
            consecutive_failures: self.consecutive_failures,
        })
    }
}

/// Serializable checkpoint metrics.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct PersistedMetrics {
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
    /// Rolling average of consensus rate (0.0 - 1.0).
    pub avg_consensus_rate: f64,
}

impl From<&CheckpointMetrics> for PersistedMetrics {
    fn from(metrics: &CheckpointMetrics) -> Self {
        Self {
            total_attempts: metrics.total_attempts,
            successful_submissions: metrics.successful_submissions,
            insufficient_consensus_count: metrics.insufficient_consensus_count,
            unreachable_failures: metrics.unreachable_failures,
            transaction_failures: metrics.transaction_failures,
            conflicts_detected: metrics.conflicts_detected,
            auto_challenge_recommended: metrics.auto_challenge_recommended,
            providers_queried: metrics.providers_queried,
            providers_responded: metrics.providers_responded,
            avg_submission_time_ms: metrics.avg_submission_time_ms,
            avg_consensus_rate: metrics.avg_consensus_rate,
        }
    }
}

impl PersistedMetrics {
    /// Convert back to CheckpointMetrics.
    pub fn to_checkpoint_metrics(&self) -> CheckpointMetrics {
        CheckpointMetrics {
            total_attempts: self.total_attempts,
            successful_submissions: self.successful_submissions,
            insufficient_consensus_count: self.insufficient_consensus_count,
            unreachable_failures: self.unreachable_failures,
            transaction_failures: self.transaction_failures,
            conflicts_detected: self.conflicts_detected,
            auto_challenge_recommended: self.auto_challenge_recommended,
            providers_queried: self.providers_queried,
            providers_responded: self.providers_responded,
            avg_submission_time_ms: self.avg_submission_time_ms,
            last_checkpoint_time: None, // Not persisted - runtime only
            avg_consensus_rate: self.avg_consensus_rate,
        }
    }
}

/// Serializable bucket checkpoint status.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct PersistedBucketStatus {
    /// Whether the bucket has pending changes.
    pub dirty: bool,
    /// Last successful checkpoint timestamp.
    pub last_checkpoint_timestamp: Option<u64>,
    /// Last checkpoint result type.
    pub last_result: Option<String>,
    /// Number of consecutive failures.
    pub consecutive_failures: u32,
}

impl From<&BucketCheckpointStatus> for PersistedBucketStatus {
    fn from(status: &BucketCheckpointStatus) -> Self {
        Self {
            dirty: status.dirty,
            last_checkpoint_timestamp: status.last_checkpoint.map(|_| current_timestamp()),
            last_result: status.last_result.as_ref().map(result_to_string),
            consecutive_failures: status.consecutive_failures,
        }
    }
}

impl PersistedBucketStatus {
    /// Convert back to BucketCheckpointStatus.
    pub fn to_bucket_status(&self) -> BucketCheckpointStatus {
        BucketCheckpointStatus {
            dirty: self.dirty,
            last_checkpoint: self.last_checkpoint_timestamp.map(|_| Instant::now()),
            last_result: None, // Not fully restored - would need more info
            consecutive_failures: self.consecutive_failures,
        }
    }
}

/// Serializable conflict record.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct PersistedConflict {
    /// Bucket ID.
    pub bucket_id: BucketId,
    /// The majority MMR root (hex encoded).
    pub majority_root: String,
    /// Number of providers agreeing on majority.
    pub majority_count: usize,
    /// When the conflict was detected (Unix timestamp).
    pub detected_at: u64,
    /// Conflict type.
    pub conflict_type: String,
    /// Provider's MMR root if different.
    pub provider_root: Option<String>,
}

// ============================================================================
// Persistence Manager
// ============================================================================

/// Manages checkpoint state persistence.
pub struct CheckpointPersistence {
    /// Configuration.
    config: PersistenceConfig,
    /// Cached state (for faster access).
    cached_state: RwLock<Option<PersistedCheckpointState>>,
    /// Dirty flag indicating unsaved changes.
    dirty: RwLock<bool>,
}

impl CheckpointPersistence {
    /// Create a new persistence manager.
    pub fn new(config: PersistenceConfig) -> Self {
        Self {
            config,
            cached_state: RwLock::new(None),
            dirty: RwLock::new(false),
        }
    }

    /// Create with default configuration.
    pub fn with_defaults() -> Self {
        Self::new(PersistenceConfig::default())
    }

    /// Get the file path.
    pub fn file_path(&self) -> &Path {
        &self.config.file_path
    }

    /// Check if there are unsaved changes.
    pub async fn is_dirty(&self) -> bool {
        *self.dirty.read().await
    }

    /// Load state from disk.
    ///
    /// Returns the loaded state, or a new empty state if the file doesn't exist.
    pub async fn load(&self) -> Result<PersistedCheckpointState, ClientError> {
        // Attempt to read file directly, handling NotFound without a prior exists() check
        let contents = match fs::read_to_string(&self.config.file_path).await {
            Ok(contents) => contents,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                let state = PersistedCheckpointState::new();
                *self.cached_state.write().await = Some(state.clone());
                return Ok(state);
            }
            Err(e) => {
                return Err(ClientError::Storage(format!(
                    "Failed to read persistence file: {e}"
                )));
            }
        };

        // Parse JSON
        let state: PersistedCheckpointState = serde_json::from_str(&contents)
            .map_err(|e| ClientError::Storage(format!("Failed to parse persistence file: {e}")))?;

        // Validate version
        if state.version > 1 {
            tracing::warn!(
                "Persistence file version {} is newer than supported version 1",
                state.version
            );
        }

        // Cache the state
        *self.cached_state.write().await = Some(state.clone());
        *self.dirty.write().await = false;

        tracing::info!(
            "Loaded checkpoint state from {} (saved at {})",
            self.config.file_path.display(),
            state.saved_at
        );

        Ok(state)
    }

    /// Save state to disk.
    pub async fn save(&self, state: &PersistedCheckpointState) -> Result<(), ClientError> {
        // Create backup if file exists
        if self.config.file_path.exists() && self.config.max_backups > 0 {
            self.rotate_backups().await?;
        }

        // Ensure parent directory exists (create_dir_all is idempotent)
        if let Some(parent) = self.config.file_path.parent() {
            fs::create_dir_all(parent).await.map_err(|e| {
                ClientError::Storage(format!("Failed to create persistence directory: {e}"))
            })?;
        }

        // Update timestamp
        let mut state = state.clone();
        state.saved_at = current_timestamp();

        // Serialize to JSON
        let contents = serde_json::to_string_pretty(&state)
            .map_err(|e| ClientError::Storage(format!("Failed to serialize state: {e}")))?;

        // Write atomically (write to temp file, then rename)
        let temp_path = self.config.file_path.with_extension("json.tmp");
        fs::write(&temp_path, &contents)
            .await
            .map_err(|e| ClientError::Storage(format!("Failed to write persistence file: {e}")))?;

        fs::rename(&temp_path, &self.config.file_path)
            .await
            .map_err(|e| ClientError::Storage(format!("Failed to rename persistence file: {e}")))?;

        // Update cache
        *self.cached_state.write().await = Some(state);
        *self.dirty.write().await = false;

        tracing::debug!(
            "Saved checkpoint state to {}",
            self.config.file_path.display()
        );

        Ok(())
    }

    /// Mark the cached state as dirty (needs saving).
    pub async fn mark_dirty(&self) {
        *self.dirty.write().await = true;
    }

    /// Get cached state (if loaded).
    pub async fn get_cached(&self) -> Option<PersistedCheckpointState> {
        self.cached_state.read().await.clone()
    }

    /// Update cached state without saving.
    pub async fn update_cached(&self, state: PersistedCheckpointState) {
        *self.cached_state.write().await = Some(state);
        *self.dirty.write().await = true;
    }

    /// Save if dirty.
    pub async fn save_if_dirty(&self) -> Result<bool, ClientError> {
        if *self.dirty.read().await {
            if let Some(state) = self.cached_state.read().await.clone() {
                self.save(&state).await?;
                return Ok(true);
            }
        }
        Ok(false)
    }

    /// Rotate backup files.
    async fn rotate_backups(&self) -> Result<(), ClientError> {
        // Remove oldest backup
        let oldest = self
            .config
            .file_path
            .with_extension(format!("json.{}", self.config.max_backups));
        if oldest.exists() {
            let _ = fs::remove_file(&oldest).await;
        }

        // Rotate existing backups
        for i in (1..self.config.max_backups).rev() {
            let from = self.config.file_path.with_extension(format!("json.{i}"));
            let to = self
                .config
                .file_path
                .with_extension(format!("json.{}", i + 1));
            if from.exists() {
                let _ = fs::rename(&from, &to).await;
            }
        }

        // Move current to .1
        let backup = self.config.file_path.with_extension("json.1");
        let _ = fs::rename(&self.config.file_path, &backup).await;

        Ok(())
    }

    /// Delete the persistence file and all backups.
    pub async fn clear(&self) -> Result<(), ClientError> {
        // Remove main file
        if self.config.file_path.exists() {
            fs::remove_file(&self.config.file_path).await.map_err(|e| {
                ClientError::Storage(format!("Failed to remove persistence file: {e}"))
            })?;
        }

        // Remove backups
        for i in 1..=self.config.max_backups {
            let backup = self.config.file_path.with_extension(format!("json.{i}"));
            if backup.exists() {
                let _ = fs::remove_file(&backup).await;
            }
        }

        // Clear cache
        *self.cached_state.write().await = None;
        *self.dirty.write().await = false;

        Ok(())
    }
}

// ============================================================================
// State Builder
// ============================================================================

/// Builder for creating PersistedCheckpointState from runtime state.
pub struct StateBuilder {
    state: PersistedCheckpointState,
}

impl StateBuilder {
    /// Create a new state builder.
    pub fn new() -> Self {
        Self {
            state: PersistedCheckpointState::new(),
        }
    }

    /// Add health histories.
    pub fn with_health_histories(
        mut self,
        histories: &HashMap<AccountId32, ProviderHealthHistory>,
    ) -> Self {
        for (account_id, history) in histories {
            self.state.health_histories.insert(
                account_id_to_string(account_id),
                PersistedHealthHistory::from(history),
            );
        }
        self
    }

    /// Add metrics.
    pub fn with_metrics(mut self, metrics: &CheckpointMetrics) -> Self {
        self.state.metrics = PersistedMetrics::from(metrics);
        self
    }

    /// Add bucket statuses.
    pub fn with_bucket_statuses(
        mut self,
        statuses: &HashMap<BucketId, BucketCheckpointStatus>,
    ) -> Self {
        for (bucket_id, status) in statuses {
            self.state
                .bucket_statuses
                .insert(*bucket_id, PersistedBucketStatus::from(status));
        }
        self
    }

    /// Build the state.
    pub fn build(self) -> PersistedCheckpointState {
        self.state
    }
}

impl Default for StateBuilder {
    fn default() -> Self {
        Self::new()
    }
}

// ============================================================================
// Helper Functions
// ============================================================================

/// Get current Unix timestamp in seconds.
fn current_timestamp() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}

/// Convert AccountId32 to hex string.
fn account_id_to_string(account_id: &AccountId32) -> String {
    let bytes: &[u8; 32] = account_id.as_ref();
    format!("0x{}", hex::encode(bytes))
}

/// Convert hex string to AccountId32.
fn string_to_account_id(s: &str) -> Result<AccountId32, ClientError> {
    let s = s.strip_prefix("0x").unwrap_or(s);
    let bytes =
        hex::decode(s).map_err(|e| ClientError::Storage(format!("Invalid account ID hex: {e}")))?;

    if bytes.len() != 32 {
        return Err(ClientError::Storage(format!(
            "Invalid account ID length: {} (expected 32)",
            bytes.len()
        )));
    }

    let mut array = [0u8; 32];
    array.copy_from_slice(&bytes);
    Ok(AccountId32::from(array))
}

/// Convert CheckpointResult to a string for persistence.
fn result_to_string(result: &CheckpointResult) -> String {
    match result {
        CheckpointResult::Submitted { block_hash, .. } => {
            format!("Submitted(0x{})", hex::encode(block_hash.as_bytes()))
        }
        CheckpointResult::InsufficientConsensus {
            agreeing, required, ..
        } => {
            format!("InsufficientConsensus({agreeing}/{required})")
        }
        CheckpointResult::ProvidersUnreachable { providers } => {
            format!("ProvidersUnreachable({})", providers.len())
        }
        CheckpointResult::NoProviders => "NoProviders".to_string(),
        CheckpointResult::TransactionFailed { error } => {
            format!("TransactionFailed({error})")
        }
    }
}

// ============================================================================
// Tests
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    #[test]
    fn test_persisted_state_serialization() {
        let state = PersistedCheckpointState {
            version: 1,
            saved_at: 1234567890,
            health_histories: HashMap::new(),
            metrics: PersistedMetrics::default(),
            bucket_statuses: HashMap::new(),
            conflict_history: HashMap::new(),
        };

        let json = serde_json::to_string(&state).unwrap();
        let restored: PersistedCheckpointState = serde_json::from_str(&json).unwrap();

        assert_eq!(restored.version, 1);
        assert_eq!(restored.saved_at, 1234567890);
    }

    #[test]
    fn test_health_history_conversion() {
        let account_id = AccountId32::new([1u8; 32]);
        let mut history = ProviderHealthHistory::new(account_id.clone());
        history.record_success(100);
        history.record_success(150);
        history.record_failure("timeout".to_string());

        let persisted = PersistedHealthHistory::from(&history);
        assert_eq!(persisted.total_requests, 3);
        assert_eq!(persisted.successful_requests, 2);
        assert_eq!(persisted.failed_requests, 1);

        let restored = persisted.to_health_history().unwrap();
        assert_eq!(restored.total_requests, 3);
        assert_eq!(restored.successful_requests, 2);
    }

    #[test]
    fn test_metrics_conversion() {
        let metrics = CheckpointMetrics {
            total_attempts: 100,
            successful_submissions: 95,
            conflicts_detected: 5,
            ..Default::default()
        };

        let persisted = PersistedMetrics::from(&metrics);
        assert_eq!(persisted.total_attempts, 100);
        assert_eq!(persisted.successful_submissions, 95);

        let restored = persisted.to_checkpoint_metrics();
        assert_eq!(restored.total_attempts, 100);
        assert_eq!(restored.successful_submissions, 95);
    }

    #[tokio::test]
    async fn test_persistence_save_load() {
        let dir = tempdir().unwrap();
        let file_path = dir.path().join("test_checkpoint.json");

        let config = PersistenceConfig::new(&file_path);
        let persistence = CheckpointPersistence::new(config);

        // Create test state
        let mut state = PersistedCheckpointState::new();
        state.metrics.total_attempts = 42;
        state.bucket_statuses.insert(
            1,
            PersistedBucketStatus {
                dirty: true,
                last_checkpoint_timestamp: Some(1234567890),
                last_result: Some("Submitted".to_string()),
                consecutive_failures: 0,
            },
        );

        // Save
        persistence.save(&state).await.unwrap();
        assert!(file_path.exists());

        // Load
        let loaded = persistence.load().await.unwrap();
        assert_eq!(loaded.metrics.total_attempts, 42);
        assert!(loaded.bucket_statuses.contains_key(&1));
    }

    #[tokio::test]
    async fn test_persistence_backup_rotation() {
        let dir = tempdir().unwrap();
        let file_path = dir.path().join("test_checkpoint.json");

        let config = PersistenceConfig::new(&file_path).with_max_backups(3);
        let persistence = CheckpointPersistence::new(config);

        // Save multiple times
        for i in 0..5 {
            let mut state = PersistedCheckpointState::new();
            state.metrics.total_attempts = i;
            persistence.save(&state).await.unwrap();
        }

        // Check backups exist
        assert!(file_path.exists());
        assert!(file_path.with_extension("json.1").exists());
        assert!(file_path.with_extension("json.2").exists());
        assert!(file_path.with_extension("json.3").exists());
        // Backup 4 should not exist (max_backups = 3)
        assert!(!file_path.with_extension("json.4").exists());
    }

    #[test]
    fn test_account_id_conversion() {
        let original = AccountId32::new([42u8; 32]);
        let string = account_id_to_string(&original);
        let restored = string_to_account_id(&string).unwrap();
        assert_eq!(original, restored);
    }

    #[test]
    fn test_state_builder() {
        let mut histories = HashMap::new();
        let account_id = AccountId32::new([1u8; 32]);
        histories.insert(account_id.clone(), ProviderHealthHistory::new(account_id));

        let metrics = CheckpointMetrics {
            total_attempts: 10,
            ..Default::default()
        };

        let state = StateBuilder::new()
            .with_health_histories(&histories)
            .with_metrics(&metrics)
            .build();

        assert_eq!(state.health_histories.len(), 1);
        assert_eq!(state.metrics.total_attempts, 10);
    }
}
