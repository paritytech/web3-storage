// SPDX-License-Identifier: GPL-3.0-only

//! Integration tests for the event-driven coordinator paths: coordinators
//! reacting to [`BlockEvent`]s fanned out by the chain-state coordinator,
//! including the bootstrap scan on `Resubscribed` and lag recovery.

use super::{test_state, test_state_with_data, wait_for, ALICE_SS58};
use provider_chain::chain_events::BlockEvent;
use sp_core::H256;
use sp_runtime::AccountId32;
use std::str::FromStr;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;
use storage_primitives::BucketId;
use storage_provider_node::challenge_responder::ChallengeError;
use storage_provider_node::replica_sync_coordinator::{BucketSnapshot, ReplicaAgreementInfo};
use storage_provider_node::{
    ChallengeChainClient, ChallengeResponder, ChallengeResponderConfig, DetectedChallenge, Error,
    ReplicaSyncChainClient, ReplicaSyncCoordinator, ReplicaSyncCoordinatorConfig,
};

fn alice_account() -> AccountId32 {
    AccountId32::from_str(ALICE_SS58).unwrap()
}

// ── challenge responder ───────────────────────────────────────────────────────

/// Mock recording which challenges were point-fetched and responded to.
struct MockChallengeClient {
    challenge: DetectedChallenge,
    fetched: AtomicUsize,
    scanned: AtomicUsize,
    submitted: Mutex<Vec<(u32, u16)>>,
}

impl MockChallengeClient {
    fn new(challenge: DetectedChallenge) -> Arc<Self> {
        Arc::new(Self {
            challenge,
            fetched: AtomicUsize::new(0),
            scanned: AtomicUsize::new(0),
            submitted: Mutex::new(Vec::new()),
        })
    }
}

#[async_trait::async_trait]
impl ChallengeChainClient for MockChallengeClient {
    async fn poll_challenges(&self) -> Result<Vec<DetectedChallenge>, ChallengeError> {
        self.scanned.fetch_add(1, Ordering::SeqCst);
        Ok(vec![])
    }

    async fn fetch_challenge(
        &self,
        deadline: u32,
        index: u16,
    ) -> Result<Option<DetectedChallenge>, ChallengeError> {
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
    ) -> Result<H256, ChallengeError> {
        self.submitted.lock().unwrap().push(challenge_id);
        Ok(H256::zero())
    }
}

/// Config with the safety net disabled: only the event path may act.
fn event_only_config() -> ChallengeResponderConfig {
    ChallengeResponderConfig {
        poll_interval: Duration::ZERO,
        ..Default::default()
    }
}

#[tokio::test]
async fn challenge_event_triggers_point_read_and_response() {
    let (state, challenge, _dir) = test_state_with_data();
    let (deadline, index) = (challenge.deadline, challenge.index);
    let bucket_id = challenge.bucket_id;
    let mock = MockChallengeClient::new(challenge);
    let responder =
        ChallengeResponder::new(event_only_config(), state, Box::new(Arc::clone(&mock)));

    let (events_tx, events_rx) = tokio::sync::broadcast::channel(16);
    let handle = responder.start(events_rx, None).await.unwrap();

    events_tx
        .send(BlockEvent::ChallengeCreated {
            deadline,
            index,
            bucket_id,
            provider: alice_account(),
        })
        .unwrap();

    let mock_ref = Arc::clone(&mock);
    assert!(
        wait_for(5, 10, || {
            let m = Arc::clone(&mock_ref);
            async move { !m.submitted.lock().unwrap().is_empty() }
        })
        .await,
        "challenge event should trigger an autonomous response"
    );
    assert_eq!(mock.submitted.lock().unwrap()[0], (deadline, index));
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
    let (state, challenge, _dir) = test_state_with_data();
    let (deadline, index) = (challenge.deadline, challenge.index);
    let bucket_id = challenge.bucket_id;
    let mock = MockChallengeClient::new(challenge);
    let responder =
        ChallengeResponder::new(event_only_config(), state, Box::new(Arc::clone(&mock)));

    let (events_tx, events_rx) = tokio::sync::broadcast::channel(16);
    let handle = responder.start(events_rx, None).await.unwrap();

    events_tx
        .send(BlockEvent::ChallengeCreated {
            deadline,
            index,
            bucket_id,
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
    let (state, challenge, _dir) = test_state_with_data();
    let mock = MockChallengeClient::new(challenge);
    let responder =
        ChallengeResponder::new(event_only_config(), state, Box::new(Arc::clone(&mock)));

    let (events_tx, events_rx) = tokio::sync::broadcast::channel(16);
    let handle = responder.start(events_rx, None).await.unwrap();

    events_tx
        .send(BlockEvent::Resubscribed { at_block: 42 })
        .unwrap();

    let mock_ref = Arc::clone(&mock);
    assert!(
        wait_for(5, 10, || {
            let m = Arc::clone(&mock_ref);
            async move { m.scanned.load(Ordering::SeqCst) > 0 }
        })
        .await,
        "resubscribe should trigger a full reconciliation scan"
    );

    handle.stop().await.unwrap();
}

#[tokio::test]
async fn event_sent_while_paused_survives_until_resume() {
    // The safety net is off here, so a dropped event would be unrecoverable:
    // the queued event is the only thing that can produce a response.
    let (state, challenge, _dir) = test_state_with_data();
    let (deadline, index) = (challenge.deadline, challenge.index);
    let bucket_id = challenge.bucket_id;
    let mock = MockChallengeClient::new(challenge);
    let responder =
        ChallengeResponder::new(event_only_config(), state, Box::new(Arc::clone(&mock)));

    let (events_tx, events_rx) = tokio::sync::broadcast::channel(16);
    let handle = responder.start(events_rx, None).await.unwrap();

    handle.pause().await.unwrap();
    events_tx
        .send(BlockEvent::ChallengeCreated {
            deadline,
            index,
            bucket_id,
            provider: alice_account(),
        })
        .unwrap();

    tokio::time::sleep(Duration::from_millis(100)).await;
    assert!(
        mock.submitted.lock().unwrap().is_empty(),
        "a paused responder must not act on events"
    );

    handle.resume().await.unwrap();

    let mock_ref = Arc::clone(&mock);
    assert!(
        wait_for(5, 10, || {
            let m = Arc::clone(&mock_ref);
            async move { !m.submitted.lock().unwrap().is_empty() }
        })
        .await,
        "the event queued during the pause should be handled on resume"
    );
    assert_eq!(mock.submitted.lock().unwrap()[0], (deadline, index));

    handle.stop().await.unwrap();
}

// ── replica sync coordinator ──────────────────────────────────────────────────

/// Mock counting duty passes (each pass calls `fetch_replica_agreements`),
/// optionally serving one agreement + bucket snapshot so a duty reaches
/// `sync_and_confirm` (with no primary endpoints, it returns without any
/// network access).
struct MockReplicaClient {
    duty_passes: AtomicUsize,
    agreement: Option<ReplicaAgreementInfo>,
    snapshot_root: H256,
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
        Ok(self.agreement.clone().into_iter().collect())
    }

    async fn fetch_bucket_snapshot(&self, _bucket_id: BucketId) -> Result<BucketSnapshot, Error> {
        Ok(BucketSnapshot {
            mmr_root: self.snapshot_root,
            leaf_count: 1,
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
        agreement: None,
        snapshot_root: H256::zero(),
    });
    let config = ReplicaSyncCoordinatorConfig {
        // Safety net disabled: only the event path may trigger duty passes.
        poll_interval: Duration::ZERO,
        ..Default::default()
    };
    let (state, _dir) = test_state();
    let coordinator = ReplicaSyncCoordinator::new(config, state, Box::new(Arc::clone(&mock)));

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
    let mock_ref = Arc::clone(&mock);
    assert!(
        wait_for(5, 10, || {
            let m = Arc::clone(&mock_ref);
            async move { m.duty_passes.load(Ordering::SeqCst) > 0 }
        })
        .await,
        "own replica agreement should trigger a duty pass"
    );

    handle.stop().await.unwrap();
}

#[tokio::test]
async fn bucket_checkpointed_event_drives_duty_through_sync_attempt() {
    // A client checkpoint on a bucket we hold locally must trigger a duty pass, and
    // the resulting duty (new root, no reachable primaries) must surface as
    // PrimaryUnavailable through the callback — all without any network.
    let (state, _dir) = test_state();
    state
        .storage
        .init_bucket(7, 1024 * 1024)
        .expect("bucket initialises");

    let mock = Arc::new(MockReplicaClient {
        duty_passes: AtomicUsize::new(0),
        agreement: Some(ReplicaAgreementInfo {
            bucket_id: 7,
            sync_balance: 10,
            sync_price: 1,
            min_sync_interval: 0,
            last_sync: None,
        }),
        snapshot_root: H256::repeat_byte(0xAB),
    });
    let config = ReplicaSyncCoordinatorConfig {
        poll_interval: Duration::ZERO,
        ..Default::default()
    };
    let coordinator = ReplicaSyncCoordinator::new(config, state, Box::new(Arc::clone(&mock)));

    let results: Arc<Mutex<Vec<storage_provider_node::SyncResult>>> =
        Arc::new(Mutex::new(Vec::new()));
    let results_cb = Arc::clone(&results);
    let (events_tx, events_rx) = tokio::sync::broadcast::channel(16);
    let handle = coordinator
        .start(
            events_rx,
            Some(Arc::new(move |result| {
                results_cb.lock().unwrap().push(result);
            })),
        )
        .await
        .unwrap();

    // A client checkpoint on a bucket we do NOT hold is irrelevant.
    events_tx
        .send(BlockEvent::BucketCheckpointed { bucket_id: 999 })
        .unwrap();
    tokio::time::sleep(Duration::from_millis(100)).await;
    assert_eq!(mock.duty_passes.load(Ordering::SeqCst), 0);

    // One on bucket 7 drives the full duty pass.
    events_tx
        .send(BlockEvent::BucketCheckpointed { bucket_id: 7 })
        .unwrap();
    let results_ref = Arc::clone(&results);
    assert!(
        wait_for(5, 10, || {
            let r = Arc::clone(&results_ref);
            async move { !r.lock().unwrap().is_empty() }
        })
        .await,
        "duty result should reach the callback"
    );
    assert!(
        matches!(
            results.lock().unwrap()[0],
            storage_provider_node::SyncResult::PrimaryUnavailable { bucket_id: 7, .. }
        ),
        "unexpected result: {:?}",
        results.lock().unwrap()[0]
    );

    handle.stop().await.unwrap();
}
