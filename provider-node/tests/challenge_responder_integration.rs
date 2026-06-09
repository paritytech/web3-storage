//! Integration tests for the challenge responder (moved from unit tests for CI coverage).

use sp_core::H256;
use std::sync::Arc;
use std::time::Duration;
use storage_primitives::{blake2_256, BucketId};
use storage_provider_node::{
    build_padded_merkle_tree, ChallengeChainClient, ChallengeResponder, ChallengeResponderConfig,
    ChallengeResponseResult, DetectedChallenge, Error, ProviderState, Storage,
};
use tokio::sync::Mutex;

struct MockChallengeChainClient {
    challenges: Mutex<Vec<DetectedChallenge>>,
    submitted: Mutex<Vec<(u32, u16)>>,
    /// If set, `submit_response` always returns this error message.
    submit_error: Mutex<Option<String>>,
}

impl MockChallengeChainClient {
    fn new() -> Self {
        Self {
            challenges: Mutex::new(Vec::new()),
            submitted: Mutex::new(Vec::new()),
            submit_error: Mutex::new(None),
        }
    }

    fn with_challenges(self, challenges: Vec<DetectedChallenge>) -> Self {
        Self {
            challenges: Mutex::new(challenges),
            ..self
        }
    }

    fn with_submit_error(self, err: String) -> Self {
        Self {
            submit_error: Mutex::new(Some(err)),
            ..self
        }
    }
}

#[async_trait::async_trait]
impl ChallengeChainClient for MockChallengeChainClient {
    async fn poll_challenges(&self) -> Result<Vec<DetectedChallenge>, Error> {
        Ok(self.challenges.lock().await.clone())
    }

    async fn submit_response(
        &self,
        challenge_id: (u32, u16),
        _chunk_data: Vec<u8>,
        _mmr_proof: storage_primitives::MmrProof,
        _chunk_proof: storage_primitives::MerkleProof,
    ) -> Result<H256, Error> {
        self.submitted.lock().await.push(challenge_id);
        if let Some(err) = self.submit_error.lock().await.as_ref() {
            return Err(Error::Internal(err.clone()));
        }
        Ok(H256::zero())
    }
}

/// Newtype wrapper to satisfy orphan rules when impl'ing the trait for shared mock access.
struct SharedMock(Arc<MockChallengeChainClient>);

#[async_trait::async_trait]
impl ChallengeChainClient for SharedMock {
    async fn poll_challenges(&self) -> Result<Vec<DetectedChallenge>, Error> {
        self.0.poll_challenges().await
    }

    async fn submit_response(
        &self,
        challenge_id: (u32, u16),
        chunk_data: Vec<u8>,
        mmr_proof: storage_primitives::MmrProof,
        chunk_proof: storage_primitives::MerkleProof,
    ) -> Result<H256, Error> {
        self.0
            .submit_response(challenge_id, chunk_data, mmr_proof, chunk_proof)
            .await
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

// --- Tests with realistic storage data ---

/// Create a provider state with a bucket containing a single committed chunk,
/// and return the state along with a matching challenge.
fn test_state_with_data() -> (Arc<ProviderState>, DetectedChallenge) {
    let storage = Arc::new(Storage::new());
    storage.init_bucket(1, 1024 * 1024);

    // Store a chunk — blake2_256 of the data is the expected hash
    let chunk_data = b"test-chunk-data-for-challenge";
    let chunk_hash = blake2_256(chunk_data);
    storage
        .store_node(1, chunk_hash, chunk_data.to_vec(), None)
        .unwrap();

    // With a single chunk, build_padded_merkle_tree returns chunk_hash directly.
    let data_root = build_padded_merkle_tree(storage.as_ref(), 1, &[chunk_hash]);
    assert_eq!(data_root, chunk_hash);

    // Commit the data_root to the MMR
    let (mmr_root, start_seq, leaf_indices) = storage.commit(1, vec![data_root]).unwrap();
    assert_eq!(leaf_indices, vec![0]);

    let challenge = DetectedChallenge {
        bucket_id: 1,
        deadline: 1000,
        index: 0,
        mmr_root,
        start_seq,
        leaf_index: 0,
        chunk_index: 0,
        challenger: "5GrwvaEF5zXb26Fz9rcQpDWS57CtERHpNehXCPcNoHGKutQY".to_string(),
        created_at_block: 900,
    };

    let state = Arc::new(ProviderState::new(
        storage,
        "5GrwvaEF5zXb26Fz9rcQpDWS57CtERHpNehXCPcNoHGKutQY".to_string(),
    ));
    (state, challenge)
}

#[tokio::test]
async fn test_successful_challenge_response() {
    let (state, challenge) = test_state_with_data();
    let mock = Arc::new(MockChallengeChainClient::new().with_challenges(vec![challenge]));

    let config = ChallengeResponderConfig {
        poll_interval: Duration::from_millis(50),
        auto_respond: true,
        ..Default::default()
    };
    let responder = ChallengeResponder::new(config, state, Box::new(SharedMock(Arc::clone(&mock))));
    let handle = responder.start(None).await.unwrap();

    // Poll until the challenge is processed (with generous timeout for CI)
    tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            if !mock.submitted.lock().await.is_empty() {
                break;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .expect("timed out waiting for challenge submission");

    let submitted = mock.submitted.lock().await;
    assert_eq!(submitted[0], (1000, 0));

    handle.stop().await.unwrap();
}

#[tokio::test]
async fn test_proof_generation_failed_no_bucket() {
    // Empty state — no buckets at all
    let state = test_state();
    let challenge = DetectedChallenge {
        bucket_id: 999,
        deadline: 1000,
        index: 0,
        mmr_root: H256::zero(),
        start_seq: 0,
        leaf_index: 0,
        chunk_index: 0,
        challenger: "5GrwvaEF...".to_string(),
        created_at_block: 900,
    };

    let mock = Arc::new(MockChallengeChainClient::new().with_challenges(vec![challenge]));

    let result: Arc<std::sync::Mutex<Option<ChallengeResponseResult>>> =
        Arc::new(std::sync::Mutex::new(None));
    let result_clone = Arc::clone(&result);
    let callback: Arc<dyn Fn(ChallengeResponseResult) + Send + Sync> = Arc::new(move |r| {
        let mut guard = result_clone.lock().unwrap();
        if guard.is_none() {
            *guard = Some(r);
        }
    });

    let config = ChallengeResponderConfig {
        poll_interval: Duration::from_millis(50),
        auto_respond: true,
        ..Default::default()
    };
    let responder = ChallengeResponder::new(config, state, Box::new(SharedMock(Arc::clone(&mock))));
    let handle = responder.start(Some(callback)).await.unwrap();

    tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            if result.lock().unwrap().is_some() {
                break;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .expect("timed out waiting for callback");
    handle.stop().await.unwrap();

    let r = result.lock().unwrap();
    assert!(
        matches!(
            &*r,
            Some(ChallengeResponseResult::ProofGenerationFailed { .. })
        ),
        "expected ProofGenerationFailed, got {:?}",
        r
    );
}

#[tokio::test]
async fn test_data_not_found_bad_chunk_index() {
    let (state, mut challenge) = test_state_with_data();
    // Valid leaf_index=0 but chunk_index beyond what exists (only 1 chunk at index 0)
    challenge.chunk_index = 999;

    let result: Arc<std::sync::Mutex<Option<ChallengeResponseResult>>> =
        Arc::new(std::sync::Mutex::new(None));
    let result_clone = Arc::clone(&result);
    let callback: Arc<dyn Fn(ChallengeResponseResult) + Send + Sync> = Arc::new(move |r| {
        let mut guard = result_clone.lock().unwrap();
        if guard.is_none() {
            *guard = Some(r);
        }
    });

    let mock = Arc::new(MockChallengeChainClient::new().with_challenges(vec![challenge]));
    let config = ChallengeResponderConfig {
        poll_interval: Duration::from_millis(50),
        auto_respond: true,
        ..Default::default()
    };
    let responder = ChallengeResponder::new(config, state, Box::new(SharedMock(Arc::clone(&mock))));
    let handle = responder.start(Some(callback)).await.unwrap();

    tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            if result.lock().unwrap().is_some() {
                break;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .expect("timed out waiting for callback");
    handle.stop().await.unwrap();

    let r = result.lock().unwrap();
    assert!(
        matches!(&*r, Some(ChallengeResponseResult::DataNotFound { .. })),
        "expected DataNotFound, got {:?}",
        r
    );
}

#[tokio::test]
async fn test_submission_failed() {
    let (state, challenge) = test_state_with_data();
    let mock = Arc::new(
        MockChallengeChainClient::new()
            .with_challenges(vec![challenge])
            .with_submit_error("chain unavailable".to_string()),
    );

    let result: Arc<std::sync::Mutex<Option<ChallengeResponseResult>>> =
        Arc::new(std::sync::Mutex::new(None));
    let result_clone = Arc::clone(&result);
    let callback: Arc<dyn Fn(ChallengeResponseResult) + Send + Sync> = Arc::new(move |r| {
        let mut guard = result_clone.lock().unwrap();
        if guard.is_none() {
            *guard = Some(r);
        }
    });

    let config = ChallengeResponderConfig {
        poll_interval: Duration::from_millis(50),
        auto_respond: true,
        ..Default::default()
    };
    let responder = ChallengeResponder::new(config, state, Box::new(SharedMock(Arc::clone(&mock))));
    let handle = responder.start(Some(callback)).await.unwrap();

    tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            if result.lock().unwrap().is_some() {
                break;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .expect("timed out waiting for callback");
    handle.stop().await.unwrap();

    let r = result.lock().unwrap();
    assert!(
        matches!(&*r, Some(ChallengeResponseResult::SubmissionFailed { .. })),
        "expected SubmissionFailed, got {:?}",
        r
    );
}

#[tokio::test]
async fn test_callback_invoked_on_success() {
    let (state, challenge) = test_state_with_data();
    let mock = Arc::new(MockChallengeChainClient::new().with_challenges(vec![challenge]));

    let result: Arc<std::sync::Mutex<Option<ChallengeResponseResult>>> =
        Arc::new(std::sync::Mutex::new(None));
    let result_clone = Arc::clone(&result);
    let callback: Arc<dyn Fn(ChallengeResponseResult) + Send + Sync> = Arc::new(move |r| {
        let mut guard = result_clone.lock().unwrap();
        if guard.is_none() {
            *guard = Some(r);
        }
    });

    let config = ChallengeResponderConfig {
        poll_interval: Duration::from_millis(50),
        auto_respond: true,
        ..Default::default()
    };
    let responder = ChallengeResponder::new(config, state, Box::new(SharedMock(Arc::clone(&mock))));
    let handle = responder.start(Some(callback)).await.unwrap();

    tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            if result.lock().unwrap().is_some() {
                break;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .expect("timed out waiting for callback");
    handle.stop().await.unwrap();

    let r = result.lock().unwrap();
    match &*r {
        Some(ChallengeResponseResult::Success {
            challenge_id,
            block_hash,
        }) => {
            assert_eq!(*challenge_id, (1000, 0));
            assert_eq!(*block_hash, H256::zero());
        }
        other => panic!("expected Success callback, got {:?}", other),
    }
}

#[tokio::test]
async fn test_resume_after_pause() {
    let (state, challenge) = test_state_with_data();
    let mock = Arc::new(MockChallengeChainClient::new().with_challenges(vec![challenge]));

    let config = ChallengeResponderConfig {
        poll_interval: Duration::from_millis(50),
        auto_respond: true,
        ..Default::default()
    };
    let responder = ChallengeResponder::new(config, state, Box::new(SharedMock(Arc::clone(&mock))));
    let handle = responder.start(None).await.unwrap();

    // Pause immediately — no submissions
    handle.pause().await.unwrap();
    tokio::time::sleep(Duration::from_millis(200)).await;
    assert!(mock.submitted.lock().await.is_empty());

    // Resume — should process the challenge
    handle.resume().await.unwrap();
    tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            if !mock.submitted.lock().await.is_empty() {
                break;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .expect("timed out waiting for submission after resume");

    handle.stop().await.unwrap();
}
