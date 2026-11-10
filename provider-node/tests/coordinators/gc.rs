// SPDX-License-Identifier: GPL-3.0-only

//! Integration tests for the GC coordinator.
//!
//! Each test seeds real committed data in a temp RocksDB backend, scripts
//! chain truth through a mock [`GcChainClient`], and drives passes via the
//! coordinator's safety-net interval (50ms here, so passes repeat quickly).

use parking_lot::Mutex;
use provider_events::EVENT_CHANNEL_CAPACITY;
use provider_storage::{ChunkTreeNode, GarbageCollectorConfiguration};
use provider_types::ChainClientError;
use sp_core::H256;
use std::collections::{HashMap, HashSet};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::Duration;
use storage_primitives::{hash_leaf, BucketId};
use storage_provider_node::{
    CanonicalBucketState, GcChainClient, GcCoordinator, GcCoordinatorConfig, ProviderState,
};

use super::{signed_receipt, test_state, wait_for};

struct MockGcChainClient {
    buckets: Mutex<HashMap<BucketId, CanonicalBucketState>>,
    agreements: Mutex<HashMap<BucketId, u64>>,
    pending: Mutex<HashSet<BucketId>>,
    /// Block the next read reports; advances by `block_step` after each read.
    block_number: AtomicU64,
    block_step: AtomicU64,
    next_bucket_id: AtomicU64,
    reads: AtomicU64,
}

impl MockGcChainClient {
    fn new() -> Self {
        Self {
            buckets: Mutex::new(HashMap::new()),
            agreements: Mutex::new(HashMap::new()),
            pending: Mutex::new(HashSet::new()),
            block_number: AtomicU64::new(1),
            block_step: AtomicU64::new(1),
            next_bucket_id: AtomicU64::new(u64::MAX),
            reads: AtomicU64::new(0),
        }
    }

    /// Report `block` on every read from now on.
    fn freeze_at(&self, block: u64) {
        self.block_number.store(block, Ordering::Relaxed);
        self.block_step.store(0, Ordering::Relaxed);
    }

    fn set_next_bucket_id(&self, next_bucket_id: u64) {
        self.next_bucket_id.store(next_bucket_id, Ordering::Relaxed);
    }

    /// Wait until the GC has read chain state at least `n` more times.
    async fn wait_for_reads(&self, n: u64) -> bool {
        let target = self.reads.load(Ordering::Relaxed) + n;
        wait_for(5, 25, || async {
            self.reads.load(Ordering::Relaxed) >= target
        })
        .await
    }

    fn with_bucket(self, bucket_id: BucketId, canonical_start_seq: Option<u64>) -> Self {
        self.buckets.lock().insert(
            bucket_id,
            CanonicalBucketState {
                exists: true,
                canonical_start_seq,
                ..CanonicalBucketState::default()
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
    ) -> Result<CanonicalBucketState, ChainClientError> {
        self.reads.fetch_add(1, Ordering::Relaxed);
        let bucket = self
            .buckets
            .lock()
            .get(&bucket_id)
            .cloned()
            .unwrap_or_default();
        Ok(CanonicalBucketState {
            block_number: self
                .block_number
                .fetch_add(self.block_step.load(Ordering::Relaxed), Ordering::Relaxed),
            next_bucket_id: self.next_bucket_id.load(Ordering::Relaxed),
            ..bucket
        })
    }

    async fn fetch_agreement_max_bytes(
        &self,
        bucket_id: BucketId,
    ) -> Result<Option<u64>, ChainClientError> {
        Ok(self.agreements.lock().get(&bucket_id).copied())
    }

    async fn has_pending_challenges(&self, bucket_id: BucketId) -> Result<bool, ChainClientError> {
        Ok(self.pending.lock().contains(&bucket_id))
    }
}

/// Commit `n` single-chunk leaves into a bucket, returning their hashes.
fn seed_bucket(state: &Arc<ProviderState>, bucket_id: BucketId, n: u8) -> Vec<H256> {
    state.storage.init_bucket(bucket_id, u64::MAX).unwrap();
    (0..n)
        .map(|i| {
            let data = vec![i; 8];
            let hash = hash_leaf(&data);
            state
                .storage
                .store_node(bucket_id, hash, ChunkTreeNode::Chunk(data))
                .unwrap();
            state.storage.commit(bucket_id, vec![hash]).unwrap();
            hash
        })
        .collect()
}

/// Prune to `new_start_seq` and attach an admin deletion receipt for it
/// signed by the test admin.
fn prune_with_receipt(state: &Arc<ProviderState>, bucket_id: BucketId, new_start_seq: u64) {
    let mmr_root = state
        .storage
        .delete_before(bucket_id, new_start_seq)
        .unwrap()
        .mmr_root;
    state
        .storage
        .attach_deletion_receipt(
            bucket_id,
            signed_receipt(bucket_id, mmr_root, new_start_seq),
        )
        .unwrap();
}

/// Start a coordinator with a fast safety-net interval over the given mock.
fn start_gc(
    state: Arc<ProviderState>,
    mock: MockGcChainClient,
) -> (
    storage_provider_node::GcCoordinatorHandle,
    tokio::sync::broadcast::Sender<provider_events::BlockEvent>,
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
    tokio::sync::broadcast::Sender<provider_events::BlockEvent>,
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
            state.storage.pruned_ranges(1).unwrap().is_empty()
                && state.storage.get_node(&hashes[0]).unwrap().is_none()
        })
        .await,
        "pruned range should be physically erased"
    );
    // Quota credited exactly for the erased leaf; the live one stays charged.
    assert_eq!(state.storage.stats().bytes, used_before - 8);
    assert!(state.storage.get_node(&hashes[1]).unwrap().is_some());
    // The receipt outlives the erasure — it is the permanent defense.
    assert!(state
        .storage
        .deletion_receipt_covering(1, 0)
        .unwrap()
        .is_some());
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
        state.storage.pruned_ranges(1).unwrap().len(),
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
        state.storage.pruned_ranges(1).unwrap().len(),
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
        state.storage.pruned_ranges(1).unwrap().len(),
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
        wait_for(5, 25, || async {
            state.storage.get_bucket(1).unwrap().is_none()
        })
        .await,
        "bucket row should be gone after teardown erasure"
    );
    for hash in hashes {
        assert!(state.storage.get_node(&hash).unwrap().is_none());
    }
    handle.stop();
}

#[tokio::test]
async fn ended_agreement_condemns_bucket_and_rejects_writes_until_erased() {
    let (state, _dir) = test_state();
    let hashes = seed_bucket(&state, 1, 2);

    // The bucket stays on chain but this provider's agreement ends. A pending
    // challenge defers erasure, so the condemned bucket stays on disk.
    let mock = Arc::new(
        MockGcChainClient::new()
            .with_bucket(1, Some(0))
            .with_agreement(1, u64::MAX)
            .with_pending_challenge(1),
    );
    let (handle, _tx, mock) = start_gc_shared(state.clone(), Arc::clone(&mock));
    mock.agreements.lock().remove(&1);

    assert!(
        wait_for(5, 25, || async { state.storage.is_condemned(1).unwrap() }).await,
        "bucket should be condemned once the agreement is gone"
    );

    let data = vec![9u8; 8];
    let hash = hash_leaf(&data);
    let stored = state
        .storage
        .store_node(1, hash, ChunkTreeNode::Chunk(data));
    assert!(
        matches!(stored, Err(provider_storage::Error::BucketCondemned(1))),
        "store_node on a condemned bucket: {stored:?}"
    );
    let committed = state.storage.commit(1, vec![hashes[0]]);
    assert!(
        matches!(committed, Err(provider_storage::Error::BucketCondemned(1))),
        "commit on a condemned bucket: {committed:?}"
    );

    mock.pending.lock().remove(&1);
    assert!(
        wait_for(5, 25, || async {
            state.storage.get_bucket(1).unwrap().is_none()
        })
        .await,
        "bucket row should be gone once the challenge clears"
    );
    for hash in hashes {
        assert!(state.storage.get_node(&hash).unwrap().is_none());
    }
    handle.stop();
}

fn last_scanned_block(state: &Arc<ProviderState>) -> u64 {
    state
        .storage
        .garbage_collector_configuration()
        .unwrap()
        .last_scanned_block
}

fn set_last_scanned_block(state: &Arc<ProviderState>, block: u64) {
    state
        .storage
        .set_garbage_collector_configuration(&GarbageCollectorConfiguration {
            last_scanned_block: block,
        })
        .unwrap();
}

#[tokio::test(start_paused = true)]
async fn chain_state_older_than_one_already_read_does_not_condemn_the_bucket() {
    let (state, _dir) = test_state();
    let hashes = seed_bucket(&state, 1, 2);
    set_last_scanned_block(&state, 100);

    // An RPC node still at block 50 knows no bucket and no agreement.
    let mock = Arc::new(MockGcChainClient::new());
    mock.freeze_at(50);
    let (handle, _tx, mock) = start_gc_shared(state.clone(), mock);

    assert!(mock.wait_for_reads(2).await, "the GC should run passes");
    assert!(!state.storage.is_condemned(1).unwrap());
    for hash in &hashes {
        assert!(state.storage.get_node(hash).unwrap().is_some());
    }
    let data = vec![9u8; 8];
    state
        .storage
        .store_node(1, hash_leaf(&data), ChunkTreeNode::Chunk(data))
        .expect("the bucket still accepts writes");
    assert_eq!(
        last_scanned_block(&state),
        100,
        "a read at an older block must not lower the stored block"
    );
    handle.stop();
}

#[tokio::test]
async fn chain_state_read_before_a_restart_still_protects_the_bucket() {
    let (state, _dir) = test_state();
    let hashes = seed_bucket(&state, 1, 2);

    let live = Arc::new(
        MockGcChainClient::new()
            .with_bucket(1, Some(0))
            .with_agreement(1, u64::MAX),
    );
    live.freeze_at(100);
    let (handle, _tx, _live) = start_gc_shared(state.clone(), live);
    assert!(
        wait_for(5, 25, || async { last_scanned_block(&state) == 100 }).await,
        "a pass at block 100 should store it as the last scanned block"
    );
    handle.stop();

    // A new coordinator over the same storage reads from a node at block 50.
    let behind = Arc::new(MockGcChainClient::new());
    behind.freeze_at(50);
    let (handle, _tx, behind) = start_gc_shared(state.clone(), behind);

    assert!(
        behind.wait_for_reads(2).await,
        "the new coordinator should run passes"
    );
    assert!(!state.storage.is_condemned(1).unwrap());
    for hash in &hashes {
        assert!(state.storage.get_node(hash).unwrap().is_some());
    }
    assert_eq!(last_scanned_block(&state), 100);
    handle.stop();
}

#[tokio::test(start_paused = true)]
async fn bucket_the_chain_had_not_created_yet_is_not_condemned() {
    let (state, _dir) = test_state();
    let hashes = seed_bucket(&state, 1, 2);

    // No bucket row at block 10, and `NextBucketId` is 1: bucket 1 does not
    // exist yet at that block, which is not the same as deleted.
    let mock = Arc::new(MockGcChainClient::new());
    mock.freeze_at(10);
    mock.set_next_bucket_id(1);
    let (handle, _tx, mock) = start_gc_shared(state.clone(), mock);

    assert!(mock.wait_for_reads(2).await, "the GC should run passes");
    assert!(!state.storage.is_condemned(1).unwrap());
    for hash in &hashes {
        assert!(state.storage.get_node(hash).unwrap().is_some());
    }
    handle.stop();
}

#[tokio::test(start_paused = true)]
async fn chain_state_older_than_one_already_read_does_not_erase_a_pruned_range() {
    let (state, _dir) = test_state();
    let hashes = seed_bucket(&state, 1, 2);
    prune_with_receipt(&state, 1, 1);
    set_last_scanned_block(&state, 100);

    // Everything an erasure needs is present, but the read is at block 50.
    let mock = Arc::new(
        MockGcChainClient::new()
            .with_bucket(1, Some(1))
            .with_agreement(1, u64::MAX),
    );
    mock.freeze_at(50);
    let (handle, _tx, mock) = start_gc_shared(state.clone(), mock);

    assert!(mock.wait_for_reads(2).await, "the GC should run passes");
    assert_eq!(state.storage.pruned_ranges(1).unwrap().len(), 1);
    assert!(state.storage.get_node(&hashes[0]).unwrap().is_some());
    handle.stop();
}

#[tokio::test]
async fn condemned_bucket_is_erased_only_after_a_newer_block_confirms_it() {
    let (state, _dir) = test_state();
    let hashes = seed_bucket(&state, 1, 2);

    // The chain has no bucket 1 at block 10 (and `NextBucketId` says it was
    // created, so the bucket was deleted).
    let mock = Arc::new(MockGcChainClient::new());
    mock.freeze_at(10);
    let (handle, _tx, mock) = start_gc_shared(state.clone(), mock);

    assert!(
        wait_for(5, 25, || async { state.storage.is_condemned(1).unwrap() }).await,
        "the bucket is condemned at block 10"
    );
    assert!(
        mock.wait_for_reads(2).await,
        "the GC should run more passes"
    );
    assert!(
        state.storage.is_condemned(1).unwrap(),
        "the bucket stays on disk, condemned, while only block 10 has been read"
    );
    for hash in &hashes {
        assert!(state.storage.get_node(hash).unwrap().is_some());
    }

    mock.freeze_at(11);
    assert!(
        wait_for(5, 25, || async {
            state.storage.get_bucket(1).unwrap().is_none()
        })
        .await,
        "a read at block 11 that still shows the bucket gone erases it"
    );
    for hash in hashes {
        assert!(state.storage.get_node(&hash).unwrap().is_none());
    }
    handle.stop();
}

#[tokio::test]
async fn obligation_that_reappears_resets_the_erase_confirmation() {
    let (state, _dir) = test_state();
    let hashes = seed_bucket(&state, 1, 2);

    let mock = Arc::new(MockGcChainClient::new());
    mock.freeze_at(10);
    let (handle, _tx, mock) = start_gc_shared(state.clone(), mock);
    assert!(
        wait_for(5, 25, || async { state.storage.is_condemned(1).unwrap() }).await,
        "the bucket is condemned at block 10"
    );

    // The bucket and agreement are back at block 11, then gone again at 20.
    mock.buckets.lock().insert(
        1,
        CanonicalBucketState {
            exists: true,
            canonical_start_seq: Some(0),
            ..CanonicalBucketState::default()
        },
    );
    mock.agreements.lock().insert(1, u64::MAX);
    mock.freeze_at(11);
    assert!(
        wait_for(5, 25, || async { last_scanned_block(&state) == 11 }).await,
        "the GC should read block 11"
    );
    mock.buckets.lock().clear();
    mock.agreements.lock().clear();
    mock.freeze_at(20);
    assert!(
        wait_for(5, 25, || async { last_scanned_block(&state) == 20 }).await,
        "the GC should read block 20"
    );
    for hash in &hashes {
        assert!(
            state.storage.get_node(hash).unwrap().is_some(),
            "the first read at block 20 must not erase: the obligation was present at 11"
        );
    }

    mock.freeze_at(21);
    assert!(
        wait_for(5, 25, || async {
            state.storage.get_bucket(1).unwrap().is_none()
        })
        .await,
        "a second read at block 21 confirms the obligation is gone"
    );
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
            let hash = hash_leaf(&data);
            matches!(
                state
                    .storage
                    .store_node(1, hash, ChunkTreeNode::Chunk(data)),
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
                .unwrap()
                .is_some_and(|info| info.start_seq == 1)
        })
        .await,
        "local start_seq should converge on the canonical prune"
    );
    // The converged prune is stashed like any other; without an admin
    // receipt it stays stashed.
    assert_eq!(state.storage.pruned_ranges(1).unwrap().len(), 1);
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
    let hash = hash_leaf(&data);
    assert!(
        wait_for(5, 25, || async {
            matches!(
                state
                    .storage
                    .store_node(1, hash, ChunkTreeNode::Chunk(vec![9u8; 8])),
                Err(provider_storage::Error::QuotaExceeded { .. })
            )
        })
        .await,
        "the pre-top-up quota should be enforced first"
    );

    // Top-up on chain, announced by its agreement event.
    mock.agreements.lock().insert(1, 100);
    let _ = tx.send(provider_events::BlockEvent::AgreementChanged {
        bucket_id: 1,
        provider: sp_runtime::AccountId32::new([0u8; 32]),
    });

    assert!(
        wait_for(5, 25, || async {
            state
                .storage
                .store_node(1, hash, ChunkTreeNode::Chunk(vec![9u8; 8]))
                .is_ok()
        })
        .await,
        "the exact upload refused before the top-up should now fit"
    );
    handle.stop();
}
