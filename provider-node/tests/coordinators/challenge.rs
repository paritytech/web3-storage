// SPDX-License-Identifier: GPL-3.0-only

//! Integration tests for the challenge responder.

use super::{
    alice_account, proof_source, test_deps, test_state, test_state_with_data, wait_for, ALICE_SS58,
};
use provider_http::ProviderState;
use provider_storage::{build_padded_merkle_tree, temp_rocksdb, StorageBackend};
use sp_core::H256;
use std::sync::{Arc, Mutex};
use std::time::Duration;
use storage_primitives::{blake2_256, BucketId, MerkleProof, MmrProof};
use storage_provider_node::challenge_responder::ChallengeError;
use storage_provider_node::{
    ChallengeChainClient, ChallengeResponder, ChallengeResponderConfig, ChallengeResponseResult,
    DetectedChallenge,
};
use tempfile::TempDir;

/// A response as submitted to `submit_response`, recorded for inspection.
struct RecordedResponse {
    chunk_data: Vec<u8>,
    mmr_proof: MmrProof,
    chunk_proof: MerkleProof,
}

/// Checks a recorded response the same way
/// `StorageProvider::respond_to_challenge`'s `Proof` arm does: the chunk
/// proves into the MMR leaf's `data_root`, and the MMR proof bags to the
/// challenged root, not whatever root the provider holds now.
fn response_matches_challenge(challenge: &DetectedChallenge, response: &RecordedResponse) -> bool {
    let chunk_hash = storage_primitives::blake2_256(&response.chunk_data);
    let chunk_ok = storage_primitives::verify_merkle_proof(
        chunk_hash,
        challenge.chunk_index,
        &response.chunk_proof,
        &response.mmr_proof.leaf.data_root,
    );
    let mmr_ok = storage_primitives::verify_mmr_proof(&response.mmr_proof, &challenge.mmr_root);
    chunk_ok && mmr_ok
}

struct MockChallengeChainClient {
    challenges: Mutex<Vec<DetectedChallenge>>,
    submitted: Mutex<Vec<(u32, u16)>>,
    responses: Mutex<Vec<RecordedResponse>>,
    submit_error: Mutex<Option<String>>,
}

impl MockChallengeChainClient {
    fn new() -> Self {
        Self {
            challenges: Mutex::new(Vec::new()),
            submitted: Mutex::new(Vec::new()),
            responses: Mutex::new(Vec::new()),
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
        chunk_data: Vec<u8>,
        mmr_proof: storage_primitives::MmrProof,
        chunk_proof: storage_primitives::MerkleProof,
    ) -> Result<H256, ChallengeError> {
        self.submitted.lock().unwrap().push(challenge_id);
        self.responses.lock().unwrap().push(RecordedResponse {
            chunk_data,
            mmr_proof,
            chunk_proof,
        });
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
        mmr_root: H256::zero(),
        start_seq: 0,
        leaf_index: 5,
        chunk_index: 0,
        challenger: ALICE_SS58.to_string(),
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
    assert_eq!(challenge.leaf_index, 5);
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
    let mock = Arc::new(MockChallengeChainClient::new().with_challenges(vec![challenge.clone()]));

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
        assert_eq!(submitted[0], (1000, 0));
    }

    {
        let responses = mock.responses.lock().unwrap();
        assert!(
            response_matches_challenge(&challenge, &responses[0]),
            "submitted proof does not verify against the challenged root {:?}",
            challenge.mmr_root
        );
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
async fn test_data_not_found_bad_chunk_index() {
    let (state, mut challenge, _dir) = test_state_with_data();
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
        matches!(&*r, Some(ChallengeResponseResult::DataNotFound { .. })),
        "expected DataNotFound, got {:?}",
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

// --- Reproductions: proof must be against the CHALLENGED commitment ---
//
// A challenge names the commitment (`mmr_root` + `start_seq`) the provider
// signed for, not "whatever the provider holds now". Both scenarios below
// build a bucket with two commits, then challenge against the first
// (still-valid) commitment. The responder must find and prove the leaf as it
// stood under that commitment.

/// A bucket committed to twice: `chunk_a` alone, then `chunk_a` + `chunk_b`.
/// Returns the provider state, a handle to the same storage backend (so a
/// test can prune it independently), and one challenge per commitment.
fn two_commit_bucket() -> (
    Arc<ProviderState>,
    Arc<dyn StorageBackend>,
    DetectedChallenge,
    DetectedChallenge,
    TempDir,
) {
    let (storage, nonce_store, dir) = temp_rocksdb();
    storage
        .init_bucket(1, 1024 * 1024)
        .expect("bucket initialises");

    let chunk_a = b"chunk-a-for-stale-commitment-test";
    let hash_a = blake2_256(chunk_a);
    storage
        .store_node(1, hash_a, chunk_a.to_vec(), None)
        .unwrap();
    let root_a = build_padded_merkle_tree(storage.as_ref(), 1, &[hash_a]);
    let (mmr_root_a, start_seq_a, leaf_indices_a) = storage.commit(1, vec![root_a]).unwrap();
    assert_eq!(leaf_indices_a, vec![0]);

    let chunk_b = b"chunk-b-for-stale-commitment-test";
    let hash_b = blake2_256(chunk_b);
    storage
        .store_node(1, hash_b, chunk_b.to_vec(), None)
        .unwrap();
    let root_b = build_padded_merkle_tree(storage.as_ref(), 1, &[hash_b]);
    let (mmr_root_ab, start_seq_ab, leaf_indices_b) = storage.commit(1, vec![root_b]).unwrap();
    assert_eq!(leaf_indices_b, vec![1]);

    let challenge_a = DetectedChallenge {
        bucket_id: 1,
        deadline: 1000,
        index: 0,
        mmr_root: mmr_root_a,
        start_seq: start_seq_a,
        leaf_index: 0,
        chunk_index: 0,
        challenger: ALICE_SS58.to_string(),
    };
    let challenge_ab = DetectedChallenge {
        bucket_id: 1,
        deadline: 1000,
        index: 0,
        mmr_root: mmr_root_ab,
        start_seq: start_seq_ab,
        leaf_index: 1,
        chunk_index: 0,
        challenger: ALICE_SS58.to_string(),
    };

    let state = Arc::new(ProviderState::with_provider_id(
        test_deps(Arc::clone(&storage), nonce_store),
        ALICE_SS58.to_string(),
    ));

    (state, storage, challenge_a, challenge_ab, dir)
}

/// Waits for a single challenge response and returns it, or panics on timeout.
async fn respond_once(
    state: &Arc<ProviderState>,
    mock: &Arc<MockChallengeChainClient>,
) -> ChallengeResponseResult {
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
    let responder =
        ChallengeResponder::new(config, proof_source(state), Box::new(Arc::clone(mock)));
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
        "timed out waiting for challenge response"
    );
    handle.stop().await.unwrap();

    let outcome = result.lock().unwrap().take().unwrap();
    outcome
}

#[tokio::test(start_paused = true)]
async fn respond_proves_against_challenged_root_after_a_later_commit() {
    let (state, _storage, challenge, _newer_challenge, _dir) = two_commit_bucket();
    let mock = Arc::new(MockChallengeChainClient::new().with_challenges(vec![challenge.clone()]));

    let outcome = respond_once(&state, &mock).await;
    assert!(
        matches!(outcome, ChallengeResponseResult::Success { .. }),
        "expected a proof to be generated and submitted for challenged root {:?}, got {:?}",
        challenge.mmr_root,
        outcome
    );

    let responses = mock.responses.lock().unwrap();
    assert!(
        response_matches_challenge(&challenge, &responses[0]),
        "submitted proof does not verify against the challenged root {:?} - the provider proved \
         against its current MMR root instead, which a later commit changed",
        challenge.mmr_root
    );
}

#[tokio::test(start_paused = true)]
async fn respond_proves_against_challenged_root_after_a_delete() {
    let (state, storage, _older_challenge, challenge, _dir) = two_commit_bucket();

    // Prune the leaf that predates this commitment. `challenge` still names
    // the pre-delete root (over both chunks) and leaf_index 1 (chunk_b),
    // which the provider still holds and must still be able to prove.
    storage
        .delete_before(1, 1)
        .expect("delete_before prunes the leaf preceding the challenged commitment");

    let mock = Arc::new(MockChallengeChainClient::new().with_challenges(vec![challenge.clone()]));

    let outcome = respond_once(&state, &mock).await;
    assert!(
        matches!(outcome, ChallengeResponseResult::Success { .. }),
        "expected chunk_b to still be found and proved against challenged root {:?} after the \
         delete shifted local leaf indices, got {:?}",
        challenge.mmr_root,
        outcome
    );

    let responses = mock.responses.lock().unwrap();
    assert!(
        response_matches_challenge(&challenge, &responses[0]),
        "submitted proof does not verify against the challenged root {:?} after delete_before",
        challenge.mmr_root
    );
}
