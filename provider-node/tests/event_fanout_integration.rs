// SPDX-License-Identifier: GPL-3.0-only

//! Integration tests for the event-driven coordinator paths: coordinators
//! reacting to [`BlockEvent`]s fanned out by the chain-state coordinator,
//! including the bootstrap scan on `Resubscribed` and lag recovery.

use sp_core::H256;
use sp_runtime::AccountId32;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;
use storage_primitives::{blake2_256, BucketId};
use storage_provider_node::chain_events::BlockEvent;
use storage_provider_node::replica_sync_coordinator::{BucketSnapshot, ReplicaAgreementInfo};
use storage_provider_node::{
    build_padded_merkle_tree, ChallengeChainClient, ChallengeResponder, ChallengeResponderConfig,
    DetectedChallenge, Error, ProviderState, ReplicaSyncChainClient, ReplicaSyncCoordinator,
    ReplicaSyncCoordinatorConfig, Storage,
};

/// Full Alice SS58 address (substrate prefix 42) and its raw account bytes.
const ALICE_SS58: &str = "5GrwvaEF5zXb26Fz9rcQpDWS57CtERHpNehXCPcNoHGKutQY";

fn alice_account() -> AccountId32 {
    use std::str::FromStr;
    AccountId32::from_str(ALICE_SS58).unwrap()
}

fn test_state() -> Arc<ProviderState> {
    Arc::new(ProviderState::with_provider_id(
        Arc::new(Storage::new()),
        ALICE_SS58.to_string(),
    ))
}

async fn wait_for<F: FnMut() -> bool>(timeout: Duration, mut f: F) -> bool {
    let deadline = tokio::time::Instant::now() + timeout;
    while tokio::time::Instant::now() < deadline {
        if f() {
            return true;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    false
}

// ── challenge responder ───────────────────────────────────────────────────────

/// Mock recording which challenges were point-fetched and responded to.
struct MockChallengeClient {
    challenge: DetectedChallenge,
    fetched: AtomicUsize,
    scanned: AtomicUsize,
    submitted: Mutex<Vec<(u32, u16)>>,
}

#[async_trait::async_trait]
impl ChallengeChainClient for MockChallengeClient {
    async fn poll_challenges(&self) -> Result<Vec<DetectedChallenge>, Error> {
        self.scanned.fetch_add(1, Ordering::SeqCst);
        Ok(vec![])
    }

    async fn fetch_challenge(
        &self,
        deadline: u32,
        index: u16,
    ) -> Result<Option<DetectedChallenge>, Error> {
        self.fetched.fetch_add(1, Ordering::SeqCst);
        if (deadline, index) == (self.challenge.deadline, self.challenge.index) {
            Ok(Some(self.challenge.clone()))
        } else {
            Ok(None)
        }
    }

    async fn submit_response(
        &self,
        challenge_id: (u32, u16),
        _chunk_data: Vec<u8>,
        _mmr_proof: storage_primitives::MmrProof,
        _chunk_proof: storage_primitives::MerkleProof,
    ) -> Result<H256, Error> {
        self.submitted.lock().unwrap().push(challenge_id);
        Ok(H256::zero())
    }
}

/// A state with one committed chunk in bucket 1 plus the matching challenge,
/// so proof generation for `(leaf 0, chunk 0)` succeeds.
fn state_with_chunk() -> (Arc<ProviderState>, DetectedChallenge) {
    let storage = Arc::new(Storage::new());
    storage.init_bucket(1, 1024 * 1024);

    let chunk_data = b"event-fanout-test-data";
    let chunk_hash = blake2_256(chunk_data);
    storage
        .store_node(1, chunk_hash, chunk_data.to_vec(), None)
        .unwrap();
    let data_root = build_padded_merkle_tree(storage.as_ref(), 1, &[chunk_hash]);
    let (mmr_root, start_seq, _) = storage.commit(1, vec![data_root]).unwrap();

    let challenge = DetectedChallenge {
        bucket_id: 1,
        deadline: 500,
        index: 3,
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

#[tokio::test]
async fn challenge_event_triggers_point_read_and_response() {
    let (state, challenge) = state_with_chunk();
    let mock = Arc::new(MockChallengeClient {
        challenge,
        fetched: AtomicUsize::new(0),
        scanned: AtomicUsize::new(0),
        submitted: Mutex::new(Vec::new()),
    });
    let config = ChallengeResponderConfig {
        // Safety net disabled: only the event path may drive the response.
        poll_interval: Duration::ZERO,
        ..Default::default()
    };
    let responder = ChallengeResponder::new(config, state, Box::new(mock.clone()));

    let (events_tx, events_rx) = tokio::sync::broadcast::channel(16);
    let handle = responder.start(events_rx, None).await.unwrap();

    events_tx
        .send(BlockEvent::ChallengeCreated {
            deadline: 500,
            index: 3,
            bucket_id: 1,
            provider: alice_account(),
        })
        .unwrap();

    assert!(
        wait_for(Duration::from_secs(5), || !mock
            .submitted
            .lock()
            .unwrap()
            .is_empty())
        .await,
        "challenge event should trigger an autonomous response"
    );
    assert_eq!(mock.submitted.lock().unwrap()[0], (500, 3));
    assert_eq!(mock.fetched.load(Ordering::SeqCst), 1);
    assert_eq!(
        mock.scanned.load(Ordering::SeqCst),
        0,
        "no full scan should run for a targeted event"
    );

    handle.stop().await.unwrap();
}

#[tokio::test]
async fn foreign_challenge_event_is_ignored() {
    let (state, challenge) = state_with_chunk();
    let mock = Arc::new(MockChallengeClient {
        challenge,
        fetched: AtomicUsize::new(0),
        scanned: AtomicUsize::new(0),
        submitted: Mutex::new(Vec::new()),
    });
    let config = ChallengeResponderConfig {
        poll_interval: Duration::ZERO,
        ..Default::default()
    };
    let responder = ChallengeResponder::new(config, state, Box::new(mock.clone()));

    let (events_tx, events_rx) = tokio::sync::broadcast::channel(16);
    let handle = responder.start(events_rx, None).await.unwrap();

    events_tx
        .send(BlockEvent::ChallengeCreated {
            deadline: 500,
            index: 3,
            bucket_id: 1,
            provider: AccountId32::new([9u8; 32]), // someone else's challenge
        })
        .unwrap();

    tokio::time::sleep(Duration::from_millis(200)).await;
    assert_eq!(
        mock.fetched.load(Ordering::SeqCst),
        0,
        "a challenge against another provider must not even be fetched"
    );
    assert!(mock.submitted.lock().unwrap().is_empty());

    handle.stop().await.unwrap();
}

#[tokio::test]
async fn resubscribe_triggers_bootstrap_scan() {
    let (state, challenge) = state_with_chunk();
    let mock = Arc::new(MockChallengeClient {
        challenge,
        fetched: AtomicUsize::new(0),
        scanned: AtomicUsize::new(0),
        submitted: Mutex::new(Vec::new()),
    });
    let config = ChallengeResponderConfig {
        poll_interval: Duration::ZERO,
        ..Default::default()
    };
    let responder = ChallengeResponder::new(config, state, Box::new(mock.clone()));

    let (events_tx, events_rx) = tokio::sync::broadcast::channel(16);
    let handle = responder.start(events_rx, None).await.unwrap();

    events_tx
        .send(BlockEvent::Resubscribed { at_block: 42 })
        .unwrap();

    assert!(
        wait_for(Duration::from_secs(5), || mock
            .scanned
            .load(Ordering::SeqCst)
            > 0)
        .await,
        "resubscribe should trigger a full reconciliation scan"
    );

    handle.stop().await.unwrap();
}

// ── replica sync coordinator ──────────────────────────────────────────────────

/// Mock counting duty passes (each pass calls `fetch_replica_agreements`).
struct MockReplicaClient {
    duty_passes: AtomicUsize,
}

#[async_trait::async_trait]
impl ReplicaSyncChainClient for MockReplicaClient {
    async fn get_current_block(&self) -> Result<u64, Error> {
        Ok(100)
    }

    async fn fetch_replica_agreements(
        &self,
        _provider_account: &str,
        _local_buckets: Vec<BucketId>,
    ) -> Result<Vec<ReplicaAgreementInfo>, Error> {
        self.duty_passes.fetch_add(1, Ordering::SeqCst);
        Ok(vec![])
    }

    async fn fetch_bucket_snapshot(&self, _bucket_id: BucketId) -> Result<BucketSnapshot, Error> {
        Ok(BucketSnapshot {
            mmr_root: H256::zero(),
            leaf_count: 0,
        })
    }

    async fn fetch_primary_endpoints(&self, _bucket_id: BucketId) -> Result<Vec<String>, Error> {
        Ok(vec![])
    }

    async fn submit_sync_confirmation(
        &self,
        _bucket_id: BucketId,
        _target_mmr_root: H256,
    ) -> Result<(u8, u128), Error> {
        Ok((0, 0))
    }
}

#[tokio::test]
async fn replica_agreement_event_triggers_duty_pass() {
    let mock = Arc::new(MockReplicaClient {
        duty_passes: AtomicUsize::new(0),
    });
    let config = ReplicaSyncCoordinatorConfig {
        // Safety net disabled: only the event path may trigger duty passes.
        poll_interval: Duration::ZERO,
        ..Default::default()
    };
    let coordinator = ReplicaSyncCoordinator::new(config, test_state(), Box::new(mock.clone()));

    let (events_tx, events_rx) = tokio::sync::broadcast::channel(16);
    let handle = coordinator.start(events_rx, None).await.unwrap();

    // An agreement for another provider is irrelevant and must not trigger.
    events_tx
        .send(BlockEvent::ReplicaAgreementEstablished {
            bucket_id: 7,
            provider: AccountId32::new([9u8; 32]),
        })
        .unwrap();
    tokio::time::sleep(Duration::from_millis(200)).await;
    assert_eq!(mock.duty_passes.load(Ordering::SeqCst), 0);

    // Our own replica agreement triggers a duty pass.
    events_tx
        .send(BlockEvent::ReplicaAgreementEstablished {
            bucket_id: 7,
            provider: alice_account(),
        })
        .unwrap();
    assert!(
        wait_for(Duration::from_secs(5), || mock
            .duty_passes
            .load(Ordering::SeqCst)
            > 0)
        .await,
        "own replica agreement should trigger a duty pass"
    );

    handle.stop().await.unwrap();
}
