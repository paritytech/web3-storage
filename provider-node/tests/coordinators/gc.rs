// SPDX-License-Identifier: GPL-3.0-only

//! Integration tests for the GC coordinator.
//!
//! Each test seeds real committed data in a temp RocksDB backend, scripts
//! chain truth through a mock [`GcChainClient`], and drives passes via the
//! coordinator's safety-net interval (50ms here, so passes repeat quickly).

use parking_lot::Mutex;
use provider_chain::EVENT_CHANNEL_CAPACITY;
use provider_storage::DeletionReceipt;
use sp_core::H256;
use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use std::time::Duration;
use storage_primitives::{blake2_256, BucketId};
use storage_provider_node::{
    CanonicalBucketState, Error, GcChainClient, GcCoordinator, GcCoordinatorConfig, ProviderState,
};

use super::{test_state, wait_for};

struct MockGcChainClient {
    buckets: Mutex<HashMap<BucketId, CanonicalBucketState>>,
    agreements: Mutex<HashMap<BucketId, u64>>,
    pending: Mutex<HashSet<BucketId>>,
}

impl MockGcChainClient {
    fn new() -> Self {
        Self {
            buckets: Mutex::new(HashMap::new()),
            agreements: Mutex::new(HashMap::new()),
            pending: Mutex::new(HashSet::new()),
        }
    }

    fn with_bucket(self, bucket_id: BucketId, canonical_start_seq: Option<u64>) -> Self {
        self.buckets.lock().insert(
            bucket_id,
            CanonicalBucketState {
                exists: true,
                frozen_start_seq: None,
                canonical_start_seq,
            },
        );
        self
    }

    fn with_agreement(self, bucket_id: BucketId, max_bytes: u64) -> Self {
        self.agreements.lock().insert(bucket_id, max_bytes);
        self
    }

    fn with_pending_challenge(self, bucket_id: BucketId) -> Self {
        self.pending.lock().insert(bucket_id);
        self
    }
}

#[async_trait::async_trait]
impl GcChainClient for MockGcChainClient {
    async fn fetch_canonical_bucket(
        &self,
        bucket_id: BucketId,
    ) -> Result<CanonicalBucketState, Error> {
        Ok(self
            .buckets
            .lock()
            .get(&bucket_id)
            .cloned()
            .unwrap_or_default())
    }

    async fn fetch_agreement_max_bytes(&self, bucket_id: BucketId) -> Result<Option<u64>, Error> {
        Ok(self.agreements.lock().get(&bucket_id).copied())
    }

    async fn has_pending_challenges(&self, bucket_id: BucketId) -> Result<bool, Error> {
        Ok(self.pending.lock().contains(&bucket_id))
    }
}

/// Commit `n` single-chunk leaves into a bucket, returning their hashes.
fn seed_bucket(state: &Arc<ProviderState>, bucket_id: BucketId, n: u8) -> Vec<H256> {
    state.storage.init_bucket(bucket_id, u64::MAX).unwrap();
    (0..n)
        .map(|i| {
            let data = vec![i; 8];
            let hash = blake2_256(&data);
            state
                .storage
                .store_node(bucket_id, hash, data, None)
                .unwrap();
            state.storage.commit(bucket_id, vec![hash]).unwrap();
            hash
        })
        .collect()
}

/// Prune to `new_start_seq` and attach an admin deletion receipt for it
/// (the backend stores receipts opaquely; validity is the pallet's concern).
fn prune_with_receipt(state: &Arc<ProviderState>, bucket_id: BucketId, new_start_seq: u64) {
    let (mmr_root, _, _) = state
        .storage
        .delete_before(bucket_id, new_start_seq)
        .unwrap();
    state
        .storage
        .attach_deletion_receipt(
            bucket_id,
            DeletionReceipt {
                mmr_root,
                new_start_seq,
                admin: sp_core::crypto::AccountId32::new([7u8; 32]),
                signature: sp_runtime::MultiSignature::Sr25519(sp_core::sr25519::Signature::from(
                    [0u8; 64],
                )),
            },
        )
        .unwrap();
}

/// Start a coordinator with a fast safety-net interval over the given mock.
fn start_gc(
    state: Arc<ProviderState>,
    mock: MockGcChainClient,
) -> (
    storage_provider_node::GcCoordinatorHandle,
    tokio::sync::broadcast::Sender<provider_chain::BlockEvent>,
) {
    let (handle, tx, _mock) = start_gc_shared(state, Arc::new(mock));
    (handle, tx)
}

/// Like [`start_gc`], but hands the mock back so a test can mutate chain
/// truth (e.g. a top-up raising the agreement's max_bytes) mid-flight.
fn start_gc_shared(
    state: Arc<ProviderState>,
    mock: Arc<MockGcChainClient>,
) -> (
    storage_provider_node::GcCoordinatorHandle,
    tokio::sync::broadcast::Sender<provider_chain::BlockEvent>,
    Arc<MockGcChainClient>,
) {
    let (tx, rx) = tokio::sync::broadcast::channel(EVENT_CHANNEL_CAPACITY);
    let config = GcCoordinatorConfig {
        scan_interval: Duration::from_millis(50),
    };
    let handle = GcCoordinator::new(config, state, Arc::clone(&mock) as Arc<_>).start(rx);
    (handle, tx, mock)
}

#[tokio::test]
async fn erases_after_canonical_checkpoint_with_receipt() {
    let (state, _dir) = test_state();
    let hashes = seed_bucket(&state, 1, 2);
    let used_before = 16;

    // Prune the first leaf, hold the admin's receipt; the chain has
    // checkpointed the prune.
    prune_with_receipt(&state, 1, 1);

    let mock = MockGcChainClient::new()
        .with_bucket(1, Some(1))
        .with_agreement(1, u64::MAX);
    let (handle, _tx) = start_gc(state.clone(), mock);

    assert!(
        wait_for(5, 25, || async {
            state.storage.pruned_ranges(1).is_empty()
                && state.storage.get_node(&hashes[0]).is_none()
        })
        .await,
        "pruned range should be physically erased"
    );
    // Quota credited exactly for the erased leaf; the live one stays charged.
    let stats = state.storage.get_bucket_stats();
    let bucket = stats.iter().find(|s| s.bucket_id == 1).unwrap();
    assert_eq!(bucket.bytes_stored, used_before - 8);
    assert!(state.storage.get_node(&hashes[1]).is_some());
    // The receipt outlives the erasure — it is the permanent defense.
    assert!(state.storage.deletion_receipt_covering(1, 0).is_some());
    handle.stop();
}

#[tokio::test(start_paused = true)]
async fn missing_receipt_blocks_erasure() {
    let (state, _dir) = test_state();
    seed_bucket(&state, 1, 2);
    // Prune without confirming: no admin receipt held.
    state.storage.delete_before(1, 1).unwrap();

    let mock = MockGcChainClient::new()
        .with_bucket(1, Some(1))
        .with_agreement(1, u64::MAX);
    let (handle, _tx) = start_gc(state.clone(), mock);

    tokio::time::sleep(Duration::from_millis(400)).await;
    assert_eq!(
        state.storage.pruned_ranges(1).len(),
        1,
        "stash must survive until the admin's deletion receipt is held"
    );
    handle.stop();
}

#[tokio::test(start_paused = true)]
async fn canonical_behind_blocks_erasure() {
    let (state, _dir) = test_state();
    seed_bucket(&state, 1, 2);
    prune_with_receipt(&state, 1, 1);

    // Chain snapshot still at start_seq 0: the prune was never checkpointed.
    let mock = MockGcChainClient::new()
        .with_bucket(1, Some(0))
        .with_agreement(1, u64::MAX);
    let (handle, _tx) = start_gc(state.clone(), mock);

    tokio::time::sleep(Duration::from_millis(400)).await;
    assert_eq!(
        state.storage.pruned_ranges(1).len(),
        1,
        "stash must survive while the canonical checkpoint lags"
    );
    handle.stop();
}

#[tokio::test(start_paused = true)]
async fn pending_challenge_blocks_erasure() {
    let (state, _dir) = test_state();
    seed_bucket(&state, 1, 2);
    prune_with_receipt(&state, 1, 1);

    let mock = MockGcChainClient::new()
        .with_bucket(1, Some(1))
        .with_agreement(1, u64::MAX)
        .with_pending_challenge(1);
    let (handle, _tx) = start_gc(state.clone(), mock);

    tokio::time::sleep(Duration::from_millis(400)).await;
    assert_eq!(
        state.storage.pruned_ranges(1).len(),
        1,
        "stash must survive while a challenge is pending"
    );
    handle.stop();
}

#[tokio::test]
async fn missing_chain_row_condemns_and_erases_bucket() {
    let (state, _dir) = test_state();
    let hashes = seed_bucket(&state, 1, 2);

    // The mock knows nothing about bucket 1: the chain row is gone (a missed
    // one-shot BucketDeleted) — the rescan alone must recover it. With the
    // obligation gone, no receipt is needed: nothing is challengeable.
    let mock = MockGcChainClient::new();
    let (handle, _tx) = start_gc(state.clone(), mock);

    assert!(
        wait_for(5, 25, || async { state.storage.get_bucket(1).is_none() }).await,
        "bucket row should be gone after teardown erasure"
    );
    for hash in hashes {
        assert!(state.storage.get_node(&hash).is_none());
    }
    handle.stop();
}

#[tokio::test]
async fn quota_synced_from_agreement() {
    let (state, _dir) = test_state();
    seed_bucket(&state, 1, 1); // 8 bytes used

    let mock = MockGcChainClient::new()
        .with_bucket(1, Some(0))
        .with_agreement(1, 10);
    let (handle, _tx) = start_gc(state.clone(), mock);

    // Once the quota lands, any further store must bounce: 8 used + 8 > 10.
    assert!(
        wait_for(5, 25, || async {
            let data = vec![9u8; 8];
            let hash = blake2_256(&data);
            matches!(
                state.storage.store_node(1, hash, data, None),
                Err(provider_storage::Error::QuotaExceeded { .. })
            )
        })
        .await,
        "agreement max_bytes should be enforced after a GC pass"
    );
    handle.stop();
}

#[tokio::test]
async fn converges_local_prune_on_canonical_start_seq() {
    let (state, _dir) = test_state();
    seed_bucket(&state, 1, 2);

    // The chain checkpointed a prune this node never performed locally
    // (replica case): canonical start_seq is ahead of the local one.
    let mock = MockGcChainClient::new()
        .with_bucket(1, Some(1))
        .with_agreement(1, u64::MAX);
    let (handle, _tx) = start_gc(state.clone(), mock);

    assert!(
        wait_for(5, 25, || async {
            state
                .storage
                .get_bucket(1)
                .is_some_and(|info| info.start_seq == 1)
        })
        .await,
        "local start_seq should converge on the canonical prune"
    );
    // The converged prune is stashed like any other; without an admin
    // receipt it stays stashed.
    assert_eq!(state.storage.pruned_ranges(1).len(), 1);
    handle.stop();
}

/// #382 acceptance: after a top-up raises the on-chain agreement's
/// max_bytes, the same upload that bounced on the old quota succeeds —
/// the node picks the raise up from the agreement event.
#[tokio::test]
async fn top_up_raises_the_enforced_quota() {
    let (state, _dir) = test_state();
    seed_bucket(&state, 1, 1); // 8 bytes used

    let mock = Arc::new(
        MockGcChainClient::new()
            .with_bucket(1, Some(0))
            .with_agreement(1, 10),
    );
    let (handle, tx, mock) = start_gc_shared(state.clone(), Arc::clone(&mock));

    // Quota 10 lands and blocks the next 8-byte store (8 used + 8 > 10).
    let data = vec![9u8; 8];
    let hash = blake2_256(&data);
    assert!(
        wait_for(5, 25, || async {
            matches!(
                state.storage.store_node(1, hash, vec![9u8; 8], None),
                Err(provider_storage::Error::QuotaExceeded { .. })
            )
        })
        .await,
        "the pre-top-up quota should be enforced first"
    );

    // Top-up on chain, announced by its agreement event.
    mock.agreements.lock().insert(1, 100);
    let _ = tx.send(provider_chain::BlockEvent::AgreementChanged {
        bucket_id: 1,
        provider: sp_runtime::AccountId32::new([0u8; 32]),
    });

    assert!(
        wait_for(5, 25, || async {
            state
                .storage
                .store_node(1, hash, vec![9u8; 8], None)
                .is_ok()
        })
        .await,
        "the exact upload refused before the top-up should now fit"
    );
    handle.stop();
}
