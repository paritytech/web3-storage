// SPDX-License-Identifier: GPL-3.0-only

//! Integration tests for the challenge responder.

use super::{alice_account, test_state, test_state_with_data, wait_for};
use sp_core::H256;
use std::sync::{Arc, Mutex};
use std::time::Duration;
use storage_primitives::{BucketId, MmrLeaf};
use storage_provider_node::challenge_responder::ChallengeError;
use storage_provider_node::{
    ChallengeChainClient, ChallengeProofSource, ChallengeResponder, ChallengeResponderConfig,
    ChallengeResponse, ChallengeResponseResult, DetectedChallenge,
};

struct MockChallengeChainClient {
    challenges: Mutex<Vec<DetectedChallenge>>,
    submitted: Mutex<Vec<((u32, u16), ChallengeResponse)>>,
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
    async fn poll_challenges(&self) -> Result<Vec<DetectedChallenge>, ChallengeError> {
        Ok(self.challenges.lock().unwrap().clone())
    }

    async fn fetch_challenge(
        &self,
        deadline: u32,
        index: u16,
    ) -> Result<Option<DetectedChallenge>, ChallengeError> {
        Ok(self
            .challenges
            .lock()
            .unwrap()
            .iter()
            .find(|c| c.deadline == deadline && c.index == index)
            .cloned())
    }

    async fn submit_response(
        &self,
        challenge_id: (u32, u16),
        response: ChallengeResponse,
    ) -> Result<H256, ChallengeError> {
        self.submitted
            .lock()
            .unwrap()
            .push((challenge_id, response));
        if let Some(err) = self.submit_error.lock().unwrap().as_ref() {
            return Err(ChallengeError::Internal(err.clone()));
        }
        Ok(H256::zero())
    }
}

fn make_challenge(bucket_id: BucketId, deadline: u32, index: u16) -> DetectedChallenge {
    DetectedChallenge {
        bucket_id,
        deadline,
        index,
        commitment: storage_primitives::Commitment {
            mmr_root: H256::zero(),
            start_seq: 0,
            leaf_count: 6,
        },
        target: storage_primitives::ChunkLocation {
            leaf_index: 5,
            chunk_index: 0,
        },
    }
}

#[test]
fn test_challenge_responder_config_new() {
    let config = ChallengeResponderConfig::new(alice_account());
    assert_eq!(config.provider_account, alice_account());
    assert_eq!(config.poll_interval, Duration::from_secs(300));
    assert!(config.auto_respond);
}

#[test]
fn test_detected_challenge() {
    let challenge = make_challenge(1, 1000, 0);
    assert_eq!(challenge.bucket_id, 1);
    assert_eq!(challenge.deadline, 1000);
    assert_eq!(challenge.target.leaf_index, 5);
}

#[tokio::test(start_paused = true)]
async fn test_no_challenges() {
    let mock = Arc::new(MockChallengeChainClient::new());
    let (state, _dir) = test_state();
    let config = ChallengeResponderConfig {
        poll_interval: Duration::from_millis(50),
        ..ChallengeResponderConfig::new(alice_account())
    };
    let responder = ChallengeResponder::new(
        config,
        state.challenge_proof_source(),
        Box::new(Arc::clone(&mock)),
    );
    let handle = responder
        .start(tokio::sync::broadcast::channel(16).1, None)
        .await
        .unwrap();

    tokio::time::sleep(Duration::from_millis(200)).await;

    assert!(mock.submitted.lock().unwrap().is_empty());
    handle.stop().await.unwrap();
}

#[tokio::test(start_paused = true)]
async fn test_paused_skips_poll() {
    let mock =
        Arc::new(MockChallengeChainClient::new().with_challenges(vec![make_challenge(1, 100, 0)]));
    let (state, _dir) = test_state();
    let config = ChallengeResponderConfig {
        poll_interval: Duration::from_millis(50),
        ..ChallengeResponderConfig::new(alice_account())
    };
    let responder = ChallengeResponder::new(
        config,
        state.challenge_proof_source(),
        Box::new(Arc::clone(&mock)),
    );
    let handle = responder
        .start(tokio::sync::broadcast::channel(16).1, None)
        .await
        .unwrap();

    handle.pause().await.unwrap();
    tokio::time::sleep(Duration::from_millis(200)).await;

    assert!(mock.submitted.lock().unwrap().is_empty());

    handle.stop().await.unwrap();
}

#[tokio::test(start_paused = true)]
async fn test_stop_command() {
    let mock = MockChallengeChainClient::new();
    let (state, _dir) = test_state();
    let config = ChallengeResponderConfig {
        poll_interval: Duration::from_secs(60),
        ..ChallengeResponderConfig::new(alice_account())
    };
    let responder = ChallengeResponder::new(config, state.challenge_proof_source(), Box::new(mock));
    let handle = responder
        .start(tokio::sync::broadcast::channel(16).1, None)
        .await
        .unwrap();

    assert!(handle.is_running());
    handle.stop().await.unwrap();
    tokio::time::sleep(Duration::from_millis(50)).await;
    assert!(!handle.is_running());
}

// --- Tests with realistic storage data ---

#[tokio::test(start_paused = true)]
async fn test_successful_challenge_response() {
    let (state, challenge, _dir) = test_state_with_data();
    let mock = Arc::new(MockChallengeChainClient::new().with_challenges(vec![challenge]));

    let config = ChallengeResponderConfig {
        poll_interval: Duration::from_millis(50),
        auto_respond: true,
        ..ChallengeResponderConfig::new(alice_account())
    };
    let responder = ChallengeResponder::new(
        config,
        state.challenge_proof_source(),
        Box::new(Arc::clone(&mock)),
    );
    let handle = responder
        .start(tokio::sync::broadcast::channel(16).1, None)
        .await
        .unwrap();

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
        assert!(matches!(
            submitted.first(),
            Some(((1000, 0), ChallengeResponse::Proof { .. }))
        ));
    }

    handle.stop().await.unwrap();
}

#[tokio::test(start_paused = true)]
async fn test_proof_generation_failed_no_bucket() {
    let (state, _dir) = test_state();
    let challenge = DetectedChallenge {
        bucket_id: 999,
        deadline: 1000,
        index: 0,
        commitment: storage_primitives::Commitment {
            mmr_root: H256::zero(),
            start_seq: 0,
            leaf_count: 1,
        },
        target: storage_primitives::ChunkLocation {
            leaf_index: 0,
            chunk_index: 0,
        },
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
        ..ChallengeResponderConfig::new(alice_account())
    };
    let responder = ChallengeResponder::new(
        config,
        state.challenge_proof_source(),
        Box::new(Arc::clone(&mock)),
    );
    let handle = responder
        .start(tokio::sync::broadcast::channel(16).1, Some(callback))
        .await
        .unwrap();

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
async fn test_submission_failed() {
    let (state, challenge, _dir) = test_state_with_data();
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
        ..ChallengeResponderConfig::new(alice_account())
    };
    let responder = ChallengeResponder::new(
        config,
        state.challenge_proof_source(),
        Box::new(Arc::clone(&mock)),
    );
    let handle = responder
        .start(tokio::sync::broadcast::channel(16).1, Some(callback))
        .await
        .unwrap();

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
    let (state, challenge, _dir) = test_state_with_data();
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
        ..ChallengeResponderConfig::new(alice_account())
    };
    let responder = ChallengeResponder::new(
        config,
        state.challenge_proof_source(),
        Box::new(Arc::clone(&mock)),
    );
    let handle = responder
        .start(tokio::sync::broadcast::channel(16).1, Some(callback))
        .await
        .unwrap();

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
    let (state, challenge, _dir) = test_state_with_data();
    let mock = Arc::new(MockChallengeChainClient::new().with_challenges(vec![challenge]));

    let config = ChallengeResponderConfig {
        poll_interval: Duration::from_millis(50),
        auto_respond: true,
        ..ChallengeResponderConfig::new(alice_account())
    };
    let responder = ChallengeResponder::new(
        config,
        state.challenge_proof_source(),
        Box::new(Arc::clone(&mock)),
    );
    let handle = responder
        .start(tokio::sync::broadcast::channel(16).1, None)
        .await
        .unwrap();

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

/// Proof source that returns the real MMR proof with its leaf changed by `edit`.
struct EditLeaf(Arc<dyn ChallengeProofSource>, fn(&mut MmrLeaf));

impl ChallengeProofSource for EditLeaf {
    fn get_mmr_proof_for_commitment(
        &self,
        bucket_id: BucketId,
        commitment: &storage_primitives::Commitment,
        leaf_index: u64,
    ) -> Result<storage_primitives::MmrProof, ChallengeError> {
        let mut proof = self
            .0
            .get_mmr_proof_for_commitment(bucket_id, commitment, leaf_index)?;
        (self.1)(&mut proof.leaf);
        Ok(proof)
    }

    fn get_chunk_at_index(
        &self,
        data_root: H256,
        chunk_index: u64,
    ) -> Result<(Vec<u8>, Vec<H256>), ChallengeError> {
        self.0.get_chunk_at_index(data_root, chunk_index)
    }
}

/// Runs the responder on `challenge` until it reports a result, and returns
/// that result and the submitted responses.
async fn respond_once(
    source: Arc<dyn ChallengeProofSource>,
    challenge: DetectedChallenge,
) -> (
    ChallengeResponseResult,
    Vec<((u32, u16), ChallengeResponse)>,
) {
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
        ..ChallengeResponderConfig::new(alice_account())
    };
    let responder = ChallengeResponder::new(config, source, Box::new(Arc::clone(&mock)));
    let handle = responder
        .start(tokio::sync::broadcast::channel(16).1, Some(callback))
        .await
        .unwrap();

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

    let result = result.lock().unwrap().take().unwrap();
    let submitted = std::mem::take(&mut *mock.submitted.lock().unwrap());
    (result, submitted)
}

/// A `chunk_index` at or past the leaf's chunk count has no chunk to prove;
/// the responder answers with `ChunkOutOfRange` instead of failing, so the
/// provider is not slashed on timeout.
#[tokio::test(start_paused = true)]
async fn test_chunk_index_past_the_leaf_is_answered_as_out_of_range() {
    let (state, mut challenge, _dir) = test_state_with_data();
    challenge.target.chunk_index = 999;

    let (result, submitted) = respond_once(state.challenge_proof_source(), challenge).await;
    assert!(
        matches!(result, ChallengeResponseResult::Success { .. }),
        "expected Success, got {result:?}"
    );
    assert!(matches!(
        submitted.as_slice(),
        [((1000, 0), ChallengeResponse::ChunkOutOfRange { .. })]
    ));
}

/// An empty leaf (`data_size == 0`, committed before `/commit` rejected empty
/// data) has no chunks, so the responder answers chunk index 0 with
/// `ChunkOutOfRange`.
#[tokio::test(start_paused = true)]
async fn test_empty_leaf_is_answered_as_out_of_range() {
    let (state, challenge, _dir) = test_state_with_data();
    assert_eq!(challenge.target.chunk_index, 0);

    let source = Arc::new(EditLeaf(state.challenge_proof_source(), |leaf| {
        leaf.data_size = 0
    }));
    let (result, submitted) = respond_once(source, challenge).await;
    assert!(
        matches!(result, ChallengeResponseResult::Success { .. }),
        "expected Success, got {result:?}"
    );
    assert!(matches!(
        submitted.as_slice(),
        [((1000, 0), ChallengeResponse::ChunkOutOfRange { .. })]
    ));
}

/// A leaf with an unknown `leaf_version` has no chunk count, so the responder
/// reports `ProofGenerationFailed` and submits nothing.
#[tokio::test(start_paused = true)]
async fn test_unknown_leaf_version_fails_without_submission() {
    let (state, challenge, _dir) = test_state_with_data();

    let source = Arc::new(EditLeaf(state.challenge_proof_source(), |leaf| {
        leaf.leaf_version = storage_primitives::LEAF_VERSION + 1
    }));
    let (result, submitted) = respond_once(source, challenge).await;
    assert!(
        matches!(
            &result,
            ChallengeResponseResult::ProofGenerationFailed { error, .. }
                if error.contains("unknown leaf version")
        ),
        "expected ProofGenerationFailed, got {result:?}"
    );
    assert!(submitted.is_empty());
}
