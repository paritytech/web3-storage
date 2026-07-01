// SPDX-License-Identifier: GPL-3.0-only

//! Integration tests for the challenge responder.

use super::{test_state, wait_for, ALICE_SS58};
use sp_core::H256;
use std::sync::{Arc, Mutex};
use std::time::Duration;
use storage_primitives::{blake2_256, BucketId};
use storage_provider_node::{
    build_padded_merkle_tree, ChallengeChainClient, ChallengeResponder, ChallengeResponderConfig,
    ChallengeResponseResult, DetectedChallenge, Error, ProviderState, Storage,
};

struct MockChallengeChainClient {
    challenges: Mutex<Vec<DetectedChallenge>>,
    submitted: Mutex<Vec<(u32, u16)>>,
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
        Ok(self.challenges.lock().unwrap().clone())
    }

    async fn submit_response(
        &self,
        challenge_id: (u32, u16),
        _chunk_data: Vec<u8>,
        _mmr_proof: storage_primitives::MmrProof,
        _chunk_proof: storage_primitives::MerkleProof,
    ) -> Result<H256, Error> {
        self.submitted.lock().unwrap().push(challenge_id);
        if let Some(err) = self.submit_error.lock().unwrap().as_ref() {
            return Err(Error::Internal(err.clone()));
        }
        Ok(H256::zero())
    }
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
        challenger: ALICE_SS58.to_string(),
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

#[tokio::test(start_paused = true)]
async fn test_no_challenges() {
    let mock = Arc::new(MockChallengeChainClient::new());
    let state = test_state();
    let config = ChallengeResponderConfig {
        poll_interval: Duration::from_millis(50),
        ..Default::default()
    };
    let responder = ChallengeResponder::new(config, state, Box::new(Arc::clone(&mock)));
    let handle = responder.start(None).await.unwrap();

    tokio::time::sleep(Duration::from_millis(200)).await;

    assert!(mock.submitted.lock().unwrap().is_empty());
    handle.stop().await.unwrap();
}

#[tokio::test(start_paused = true)]
async fn test_paused_skips_poll() {
    let mock =
        Arc::new(MockChallengeChainClient::new().with_challenges(vec![make_challenge(1, 100, 0)]));
    let state = test_state();
    let config = ChallengeResponderConfig {
        poll_interval: Duration::from_millis(50),
        ..Default::default()
    };
    let responder = ChallengeResponder::new(config, state, Box::new(Arc::clone(&mock)));
    let handle = responder.start(None).await.unwrap();

    handle.pause().await.unwrap();
    tokio::time::sleep(Duration::from_millis(200)).await;

    assert!(mock.submitted.lock().unwrap().is_empty());

    handle.stop().await.unwrap();
}

#[tokio::test(start_paused = true)]
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

    let chunk_data = b"test-chunk-data-for-challenge";
    let chunk_hash = blake2_256(chunk_data);
    storage
        .store_node(1, chunk_hash, chunk_data.to_vec(), None)
        .unwrap();

    let data_root = build_padded_merkle_tree(storage.as_ref(), 1, &[chunk_hash]);
    assert_eq!(data_root, chunk_hash);

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
        challenger: ALICE_SS58.to_string(),
    };

    let state = Arc::new(ProviderState::with_provider_id(
        storage,
        ALICE_SS58.to_string(),
    ));
    (state, challenge)
}

#[tokio::test(start_paused = true)]
async fn test_successful_challenge_response() {
    let (state, challenge) = test_state_with_data();
    let mock = Arc::new(MockChallengeChainClient::new().with_challenges(vec![challenge]));

    let config = ChallengeResponderConfig {
        poll_interval: Duration::from_millis(50),
        auto_respond: true,
        ..Default::default()
    };
    let responder = ChallengeResponder::new(config, state, Box::new(Arc::clone(&mock)));
    let handle = responder.start(None).await.unwrap();

    let mock_ref = Arc::clone(&mock);
    assert!(
        wait_for(5, 10, || {
            let m = Arc::clone(&mock_ref);
            async move { !m.submitted.lock().unwrap().is_empty() }
        })
        .await,
        "timed out waiting for challenge submission"
    );

    {
        let submitted = mock.submitted.lock().unwrap();
        assert_eq!(submitted[0], (1000, 0));
    }

    handle.stop().await.unwrap();
}

#[tokio::test(start_paused = true)]
async fn test_proof_generation_failed_no_bucket() {
    let state = test_state();
    let challenge = DetectedChallenge {
        bucket_id: 999,
        deadline: 1000,
        index: 0,
        mmr_root: H256::zero(),
        start_seq: 0,
        leaf_index: 0,
        chunk_index: 0,
        challenger: ALICE_SS58.to_string(),
    };

    let mock = Arc::new(MockChallengeChainClient::new().with_challenges(vec![challenge]));

    let result: Arc<Mutex<Option<ChallengeResponseResult>>> = Arc::new(Mutex::new(None));
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
    let responder = ChallengeResponder::new(config, state, Box::new(Arc::clone(&mock)));
    let handle = responder.start(Some(callback)).await.unwrap();

    let result_ref = Arc::clone(&result);
    assert!(
        wait_for(5, 10, || {
            let r = Arc::clone(&result_ref);
            async move { r.lock().unwrap().is_some() }
        })
        .await,
        "timed out waiting for callback"
    );
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

#[tokio::test(start_paused = true)]
async fn test_data_not_found_bad_chunk_index() {
    let (state, mut challenge) = test_state_with_data();
    challenge.chunk_index = 999;

    let result: Arc<Mutex<Option<ChallengeResponseResult>>> = Arc::new(Mutex::new(None));
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
    let responder = ChallengeResponder::new(config, state, Box::new(Arc::clone(&mock)));
    let handle = responder.start(Some(callback)).await.unwrap();

    let result_ref = Arc::clone(&result);
    assert!(
        wait_for(5, 10, || {
            let r = Arc::clone(&result_ref);
            async move { r.lock().unwrap().is_some() }
        })
        .await,
        "timed out waiting for callback"
    );
    handle.stop().await.unwrap();

    let r = result.lock().unwrap();
    assert!(
        matches!(&*r, Some(ChallengeResponseResult::DataNotFound { .. })),
        "expected DataNotFound, got {:?}",
        r
    );
}

#[tokio::test(start_paused = true)]
async fn test_submission_failed() {
    let (state, challenge) = test_state_with_data();
    let mock = Arc::new(
        MockChallengeChainClient::new()
            .with_challenges(vec![challenge])
            .with_submit_error("chain unavailable".to_string()),
    );

    let result: Arc<Mutex<Option<ChallengeResponseResult>>> = Arc::new(Mutex::new(None));
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
    let responder = ChallengeResponder::new(config, state, Box::new(Arc::clone(&mock)));
    let handle = responder.start(Some(callback)).await.unwrap();

    let result_ref = Arc::clone(&result);
    assert!(
        wait_for(5, 10, || {
            let r = Arc::clone(&result_ref);
            async move { r.lock().unwrap().is_some() }
        })
        .await,
        "timed out waiting for callback"
    );
    handle.stop().await.unwrap();

    let r = result.lock().unwrap();
    assert!(
        matches!(&*r, Some(ChallengeResponseResult::SubmissionFailed { .. })),
        "expected SubmissionFailed, got {:?}",
        r
    );
}

#[tokio::test(start_paused = true)]
async fn test_callback_invoked_on_success() {
    let (state, challenge) = test_state_with_data();
    let mock = Arc::new(MockChallengeChainClient::new().with_challenges(vec![challenge]));

    let result: Arc<Mutex<Option<ChallengeResponseResult>>> = Arc::new(Mutex::new(None));
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
    let responder = ChallengeResponder::new(config, state, Box::new(Arc::clone(&mock)));
    let handle = responder.start(Some(callback)).await.unwrap();

    let result_ref = Arc::clone(&result);
    assert!(
        wait_for(5, 10, || {
            let r = Arc::clone(&result_ref);
            async move { r.lock().unwrap().is_some() }
        })
        .await,
        "timed out waiting for callback"
    );
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

#[tokio::test(start_paused = true)]
async fn test_resume_after_pause() {
    let (state, challenge) = test_state_with_data();
    let mock = Arc::new(MockChallengeChainClient::new().with_challenges(vec![challenge]));

    let config = ChallengeResponderConfig {
        poll_interval: Duration::from_millis(50),
        auto_respond: true,
        ..Default::default()
    };
    let responder = ChallengeResponder::new(config, state, Box::new(Arc::clone(&mock)));
    let handle = responder.start(None).await.unwrap();

    handle.pause().await.unwrap();
    tokio::time::sleep(Duration::from_millis(200)).await;
    assert!(mock.submitted.lock().unwrap().is_empty());

    handle.resume().await.unwrap();
    let mock_ref = Arc::clone(&mock);
    assert!(
        wait_for(5, 10, || {
            let m = Arc::clone(&mock_ref);
            async move { !m.submitted.lock().unwrap().is_empty() }
        })
        .await,
        "timed out waiting for submission after resume"
    );

    handle.stop().await.unwrap();
}
