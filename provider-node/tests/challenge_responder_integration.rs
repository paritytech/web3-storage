//! Integration tests for the challenge responder (moved from unit tests for CI coverage).

use sp_core::H256;
use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;
use std::time::Duration;
use storage_primitives::BucketId;
use storage_provider_node::{
    ChallengeChainClient, ChallengeResponder, ChallengeResponderConfig, ChallengeResponseResult,
    DetectedChallenge, Error, ProviderState, Storage,
};
use tokio::sync::Mutex;

struct MockChallengeChainClient {
    challenges: Mutex<Vec<DetectedChallenge>>,
    submitted: Mutex<Vec<(u32, u16)>>,
    submit_result: Mutex<Option<Result<H256, Error>>>,
}

impl MockChallengeChainClient {
    fn new() -> Self {
        Self {
            challenges: Mutex::new(Vec::new()),
            submitted: Mutex::new(Vec::new()),
            submit_result: Mutex::new(None),
        }
    }

    fn with_challenges(self, challenges: Vec<DetectedChallenge>) -> Self {
        Self {
            challenges: Mutex::new(challenges),
            ..self
        }
    }

    #[allow(dead_code)]
    fn with_submit_result(self, result: Result<H256, Error>) -> Self {
        Self {
            submit_result: Mutex::new(Some(result)),
            ..self
        }
    }
}

impl ChallengeChainClient for MockChallengeChainClient {
    fn poll_challenges(
        &self,
    ) -> Pin<Box<dyn Future<Output = Result<Vec<DetectedChallenge>, Error>> + Send + '_>> {
        Box::pin(async { Ok(self.challenges.lock().await.clone()) })
    }

    fn submit_response(
        &self,
        challenge_id: (u32, u16),
        _chunk_data: Vec<u8>,
        _mmr_proof: storage_primitives::MmrProof,
        _chunk_proof: storage_primitives::MerkleProof,
    ) -> Pin<Box<dyn Future<Output = Result<H256, Error>> + Send + '_>> {
        Box::pin(async move {
            self.submitted.lock().await.push(challenge_id);
            let result = self.submit_result.lock().await.take();
            result.unwrap_or(Ok(H256::zero()))
        })
    }
}

/// Newtype wrapper to satisfy orphan rules when impl'ing the trait for shared mock access.
struct SharedMock(Arc<MockChallengeChainClient>);

impl ChallengeChainClient for SharedMock {
    fn poll_challenges(
        &self,
    ) -> Pin<Box<dyn Future<Output = Result<Vec<DetectedChallenge>, Error>> + Send + '_>> {
        self.0.poll_challenges()
    }

    fn submit_response(
        &self,
        challenge_id: (u32, u16),
        chunk_data: Vec<u8>,
        mmr_proof: storage_primitives::MmrProof,
        chunk_proof: storage_primitives::MerkleProof,
    ) -> Pin<Box<dyn Future<Output = Result<H256, Error>> + Send + '_>> {
        self.0
            .submit_response(challenge_id, chunk_data, mmr_proof, chunk_proof)
    }
}

fn test_state() -> Arc<ProviderState> {
    let storage = Arc::new(Storage::new());
    Arc::new(ProviderState::new(
        storage,
        "5GrwvaEF5zXb26Fz9rcQpDWS57CtERHpNehXCPcNoHGKutQY".to_string(),
    ))
}

fn make_challenge(bucket_id: BucketId, deadline: u32, index: u16) -> DetectedChallenge {
    DetectedChallenge {
        bucket_id,
        deadline,
        index,
        mmr_root: H256::zero(),
        start_seq: 0,
        leaf_index: 5,
        chunk_index: 0,
        challenger: "5GrwvaEF...".to_string(),
        created_at_block: 900,
    }
}

#[test]
fn test_challenge_responder_config_default() {
    let config = ChallengeResponderConfig::default();
    assert_eq!(config.poll_interval, Duration::from_secs(6));
    assert!(config.auto_respond);
}

#[test]
fn test_detected_challenge() {
    let challenge = make_challenge(1, 1000, 0);
    assert_eq!(challenge.bucket_id, 1);
    assert_eq!(challenge.deadline, 1000);
    assert_eq!(challenge.leaf_index, 5);
}

#[test]
fn test_challenge_response_result_variants() {
    let success = ChallengeResponseResult::Success {
        challenge_id: (1000, 0),
        block_hash: H256::zero(),
    };
    assert!(matches!(success, ChallengeResponseResult::Success { .. }));

    let proof_failed = ChallengeResponseResult::ProofGenerationFailed {
        challenge_id: (1000, 0),
        error: "test".to_string(),
    };
    assert!(matches!(
        proof_failed,
        ChallengeResponseResult::ProofGenerationFailed { .. }
    ));

    let not_found = ChallengeResponseResult::DataNotFound {
        challenge_id: (1000, 0),
        bucket_id: 1,
        leaf_index: 5,
    };
    assert!(matches!(
        not_found,
        ChallengeResponseResult::DataNotFound { .. }
    ));
}

#[tokio::test]
async fn test_no_challenges() {
    let mock = Arc::new(MockChallengeChainClient::new());
    let state = test_state();
    let config = ChallengeResponderConfig {
        poll_interval: Duration::from_millis(50),
        ..Default::default()
    };
    let responder = ChallengeResponder::new(config, state, Box::new(SharedMock(Arc::clone(&mock))));
    let handle = responder.start(None).await.unwrap();

    tokio::time::sleep(Duration::from_millis(200)).await;

    assert!(mock.submitted.lock().await.is_empty());
    handle.stop().await.unwrap();
}

#[tokio::test]
async fn test_paused_skips_poll() {
    let mock =
        Arc::new(MockChallengeChainClient::new().with_challenges(vec![make_challenge(1, 100, 0)]));
    let state = test_state();
    let config = ChallengeResponderConfig {
        poll_interval: Duration::from_millis(50),
        ..Default::default()
    };
    let responder = ChallengeResponder::new(config, state, Box::new(SharedMock(Arc::clone(&mock))));
    let handle = responder.start(None).await.unwrap();

    // Pause immediately
    handle.pause().await.unwrap();
    tokio::time::sleep(Duration::from_millis(200)).await;

    // No submissions because paused
    assert!(mock.submitted.lock().await.is_empty());

    handle.stop().await.unwrap();
}

#[tokio::test]
async fn test_stop_command() {
    let mock = MockChallengeChainClient::new();
    let state = test_state();
    let config = ChallengeResponderConfig {
        poll_interval: Duration::from_secs(60),
        ..Default::default()
    };
    let responder = ChallengeResponder::new(config, state, Box::new(mock));
    let handle = responder.start(None).await.unwrap();

    assert!(handle.is_running());
    handle.stop().await.unwrap();
    tokio::time::sleep(Duration::from_millis(50)).await;
    assert!(!handle.is_running());
}
