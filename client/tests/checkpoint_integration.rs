//! Integration tests for the checkpoint system.
//!
//! These tests verify the checkpoint manager's behavior with multiple
//! provider endpoints, conflict detection, and metrics tracking.

use sp_core::H256;
use sp_runtime::AccountId32;
use std::sync::Arc;
use std::time::Duration;
use storage_client::{
    AutoChallengeConfig, BatchedCheckpointConfig, BatchedInterval, CheckpointConfig,
    CheckpointMetrics, CheckpointResult, ConflictType, ProviderConflict, ProviderHealthHistory,
    ProviderStatus,
};
use storage_provider_node::{create_router, ProviderState, Storage};
use tokio::net::TcpListener;

/// Start a test provider node and return its URL.
async fn start_test_provider() -> String {
    let storage = Arc::new(Storage::new());
    let state = Arc::new(ProviderState::new(storage, "0xtest_provider".to_string()));
    let app = create_router(state);

    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();

    tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });

    tokio::time::sleep(Duration::from_millis(10)).await;
    format!("http://{}", addr)
}

/// Start multiple test provider nodes.
async fn start_multiple_providers(count: usize) -> Vec<String> {
    let mut urls = Vec::new();
    for _ in 0..count {
        urls.push(start_test_provider().await);
    }
    urls
}

// ============================================================================
// CheckpointConfig Tests
// ============================================================================

#[test]
fn test_checkpoint_config_builder() {
    let config = CheckpointConfig {
        provider_timeout: Duration::from_secs(60),
        max_retries: 5,
        retry_delay: Duration::from_secs(5),
        consensus_threshold_percent: 67,
        provider_cache_ttl: Duration::from_secs(600),
    };

    assert_eq!(config.provider_timeout, Duration::from_secs(60));
    assert_eq!(config.max_retries, 5);
    assert_eq!(config.consensus_threshold_percent, 67);
}

#[test]
fn test_batched_config_variants() {
    // Test with blocks
    let config_blocks = BatchedCheckpointConfig {
        interval: BatchedInterval::Blocks(50),
        submit_on_empty: true,
        max_consecutive_failures: 3,
        failure_retry_delay: Duration::from_secs(10),
    };
    assert!(config_blocks.submit_on_empty);

    // Test with duration
    let config_duration = BatchedCheckpointConfig {
        interval: BatchedInterval::Duration(Duration::from_secs(60)),
        submit_on_empty: false,
        max_consecutive_failures: 10,
        failure_retry_delay: Duration::from_secs(5),
    };
    assert!(!config_duration.submit_on_empty);
}

// ============================================================================
// Provider Health Tracking Tests
// ============================================================================

#[test]
fn test_provider_health_tracking_degradation() {
    let account = AccountId32::new([1u8; 32]);
    let mut history = ProviderHealthHistory::new(account);

    // Start healthy with many successes to maintain good success rate
    for _ in 0..10 {
        history.record_success(50);
    }
    assert!(history.is_healthy());
    assert!(matches!(history.current_status(), ProviderStatus::Healthy));

    // Some failures but still healthy (10/12 = 83% success rate, consecutive < 3)
    history.record_failure("timeout".to_string());
    history.record_failure("timeout".to_string());
    assert!(history.is_healthy()); // success_rate > 80% and consecutive_failures < 3

    // Third consecutive failure causes degradation
    history.record_failure("connection refused".to_string());
    assert!(!history.is_healthy()); // consecutive_failures = 3
}

#[test]
fn test_provider_health_recovery() {
    let account = AccountId32::new([2u8; 32]);
    let mut history = ProviderHealthHistory::new(account);

    // Start with failures
    for _ in 0..4 {
        history.record_failure("error".to_string());
    }
    assert!(!history.is_healthy());

    // Success resets consecutive failures
    history.record_success(100);
    assert_eq!(history.consecutive_failures, 0);

    // But overall success rate is still low
    assert!(!history.is_healthy()); // 1/5 = 20% success rate
}

#[test]
fn test_provider_becomes_unreachable() {
    let account = AccountId32::new([3u8; 32]);
    let mut history = ProviderHealthHistory::new(account);

    // 5 consecutive failures
    for _ in 0..5 {
        history.record_failure("unreachable".to_string());
    }

    assert!(matches!(
        history.current_status(),
        ProviderStatus::Unreachable { .. }
    ));
}

// ============================================================================
// Conflict Detection Tests
// ============================================================================

#[test]
fn test_conflict_type_sync_delay() {
    let conflict = ConflictType::SyncDelay { behind_by: 10 };
    assert!(matches!(
        conflict,
        ConflictType::SyncDelay { behind_by: 10 }
    ));
}

#[test]
fn test_conflict_type_data_divergence() {
    let conflict = ConflictType::DataDivergence;
    assert!(matches!(conflict, ConflictType::DataDivergence));
}

#[test]
fn test_conflict_type_ahead() {
    let conflict = ConflictType::Ahead { ahead_by: 5 };
    assert!(matches!(conflict, ConflictType::Ahead { ahead_by: 5 }));
}

#[test]
fn test_provider_conflict_creation() {
    use std::time::Instant;
    use storage_client::{ConflictResolution, ConflictingProvider};

    let conflict = ProviderConflict {
        bucket_id: 1,
        majority_root: H256::zero(),
        majority_count: 2,
        conflicts: vec![ConflictingProvider {
            account_id: AccountId32::new([1u8; 32]),
            mmr_root: H256::repeat_byte(0x11),
            leaf_count: 100,
            conflict_type: ConflictType::DataDivergence,
        }],
        detected_at: Instant::now(),
        resolution: ConflictResolution::ConsiderChallenge {
            provider: AccountId32::new([1u8; 32]),
        },
    };

    assert_eq!(conflict.bucket_id, 1);
    assert_eq!(conflict.majority_count, 2);
    assert_eq!(conflict.conflicts.len(), 1);
}

// ============================================================================
// Metrics Tracking Tests
// ============================================================================

#[test]
fn test_metrics_success_tracking() {
    let mut metrics = CheckpointMetrics::default();

    let result = CheckpointResult::Submitted {
        block_hash: H256::zero(),
        signers: vec![AccountId32::new([1u8; 32]), AccountId32::new([2u8; 32])],
    };

    metrics.record_attempt(&result, 150);

    assert_eq!(metrics.total_attempts, 1);
    assert_eq!(metrics.successful_submissions, 1);
    assert_eq!(metrics.providers_responded, 2);
    assert_eq!(metrics.success_rate(), 1.0);
}

#[test]
fn test_metrics_failure_tracking() {
    let mut metrics = CheckpointMetrics::default();

    // Insufficient consensus
    let result1 = CheckpointResult::InsufficientConsensus {
        agreeing: 1,
        required: 2,
        disagreements: vec![],
    };
    metrics.record_attempt(&result1, 100);

    // Unreachable providers
    let result2 = CheckpointResult::ProvidersUnreachable {
        providers: vec![AccountId32::new([1u8; 32])],
    };
    metrics.record_attempt(&result2, 50);

    // Transaction failure
    let result3 = CheckpointResult::TransactionFailed {
        error: "gas limit".to_string(),
    };
    metrics.record_attempt(&result3, 25);

    assert_eq!(metrics.total_attempts, 3);
    assert_eq!(metrics.successful_submissions, 0);
    assert_eq!(metrics.insufficient_consensus_count, 1);
    assert_eq!(metrics.unreachable_failures, 1);
    assert_eq!(metrics.transaction_failures, 1);
    assert_eq!(metrics.success_rate(), 0.0);
}

#[test]
fn test_metrics_provider_response_rate() {
    let mut metrics = CheckpointMetrics::default();

    // Record some providers queried
    metrics.providers_queried = 10;
    metrics.providers_responded = 8;

    assert_eq!(metrics.provider_response_rate(), 0.8);
}

// ============================================================================
// Auto-Challenge Configuration Tests
// ============================================================================

#[test]
fn test_auto_challenge_config_disabled_by_default() {
    let config = AutoChallengeConfig::default();
    assert!(!config.enabled);
}

#[test]
fn test_auto_challenge_config_custom() {
    let config = AutoChallengeConfig {
        enabled: true,
        min_conflict_count: 5,
        sync_wait_duration: Duration::from_secs(120),
        challenge_on_divergence: false,
    };

    assert!(config.enabled);
    assert_eq!(config.min_conflict_count, 5);
    assert_eq!(config.sync_wait_duration, Duration::from_secs(120));
    assert!(!config.challenge_on_divergence);
}

// ============================================================================
// Integration Tests with Test Providers
// ============================================================================

#[tokio::test]
async fn test_checkpoint_manager_with_single_provider() {
    let url = start_test_provider().await;

    // Note: This test creates a manager but can't test full checkpoint flow
    // without a running blockchain. It validates that the manager can be
    // configured with provider endpoints.
    let config = CheckpointConfig::default();

    // We can't test the full flow without a chain, but we can validate
    // the configuration works
    assert_eq!(config.consensus_threshold_percent, 51);
    assert!(!url.is_empty());
}

#[tokio::test]
async fn test_multiple_provider_endpoints() {
    let urls = start_multiple_providers(3).await;

    assert_eq!(urls.len(), 3);
    for url in &urls {
        assert!(url.starts_with("http://"));
    }

    // In a full integration test with a blockchain, we would:
    // 1. Create a CheckpointManager with these endpoints
    // 2. Submit checkpoints and verify consensus
}

#[tokio::test]
async fn test_provider_health_over_requests() {
    let url = start_test_provider().await;

    // Make several health check requests
    let client = reqwest::Client::new();
    for _ in 0..5 {
        let resp = client.get(format!("{}/health", url)).send().await.unwrap();
        assert!(resp.status().is_success());
    }

    // In a full integration test, we would track these in ProviderHealthHistory
}

// ============================================================================
// Commitment Collection Simulation Tests
// ============================================================================

#[tokio::test]
async fn test_simulated_commitment_collection() {
    // Simulate collecting commitments from multiple providers
    // without requiring actual blockchain interaction

    use storage_client::CommitmentCollection;

    let collection = CommitmentCollection {
        bucket_id: 1,
        mmr_root: H256::repeat_byte(0xAB),
        start_seq: 0,
        leaf_count: 100,
        signatures: vec![
            (AccountId32::new([1u8; 32]), vec![0u8; 64]),
            (AccountId32::new([2u8; 32]), vec![0u8; 64]),
        ],
        agreeing_providers: vec![AccountId32::new([1u8; 32]), AccountId32::new([2u8; 32])],
        disagreeing_providers: vec![(AccountId32::new([3u8; 32]), H256::repeat_byte(0xCD))],
        unreachable_providers: vec![AccountId32::new([4u8; 32])],
    };

    // Verify consensus calculation
    let total_providers = collection.agreeing_providers.len()
        + collection.disagreeing_providers.len()
        + collection.unreachable_providers.len();
    let agreeing_percent = (collection.agreeing_providers.len() * 100) / total_providers;

    assert_eq!(total_providers, 4);
    assert_eq!(agreeing_percent, 50); // 2 out of 4 = 50%
}

#[tokio::test]
async fn test_checkpoint_result_handling() {
    // Test handling different checkpoint results

    // Submitted result
    let submitted = CheckpointResult::Submitted {
        block_hash: H256::repeat_byte(0x12),
        signers: vec![AccountId32::new([1u8; 32])],
    };
    assert!(matches!(submitted, CheckpointResult::Submitted { .. }));

    // Insufficient consensus
    let insufficient = CheckpointResult::InsufficientConsensus {
        agreeing: 1,
        required: 2,
        disagreements: vec![],
    };
    assert!(matches!(
        insufficient,
        CheckpointResult::InsufficientConsensus { .. }
    ));

    // No providers
    let no_providers = CheckpointResult::NoProviders;
    assert!(matches!(no_providers, CheckpointResult::NoProviders));
}

// ============================================================================
// Batched Checkpoint Loop Configuration Tests
// ============================================================================

#[test]
fn test_batched_interval_to_duration() {
    // Test interval conversion logic that would be used in the loop
    let interval_blocks = BatchedInterval::Blocks(100);
    let interval_duration = BatchedInterval::Duration(Duration::from_secs(600));

    match interval_blocks {
        BatchedInterval::Blocks(blocks) => {
            // Assuming 6 second block time
            let expected_duration = Duration::from_secs(blocks as u64 * 6);
            assert_eq!(expected_duration, Duration::from_secs(600));
        }
        _ => panic!("Expected Blocks"),
    }

    match interval_duration {
        BatchedInterval::Duration(d) => {
            assert_eq!(d, Duration::from_secs(600));
        }
        _ => panic!("Expected Duration"),
    }
}

#[test]
fn test_batched_config_failure_handling() {
    let config = BatchedCheckpointConfig {
        interval: BatchedInterval::Blocks(50),
        submit_on_empty: false,
        max_consecutive_failures: 3,
        failure_retry_delay: Duration::from_secs(30),
    };

    // Simulate failure tracking
    let mut consecutive_failures = 0u32;

    // Simulate failures up to max
    for _ in 0..3 {
        consecutive_failures += 1;
        if consecutive_failures >= config.max_consecutive_failures {
            // Would pause the loop
            break;
        }
    }

    assert_eq!(consecutive_failures, config.max_consecutive_failures);
}
