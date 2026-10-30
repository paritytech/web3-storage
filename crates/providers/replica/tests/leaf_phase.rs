// SPDX-License-Identifier: Apache-2.0

//! The leaf phase against mock sources and a real local storage backend.

mod common;

use common::*;
use provider_replica::leaves::fetch_leaves;
use provider_replica::source::{SourceClient, SourceSet};
use provider_replica::Error;
use provider_storage::{temp_rocksdb, ChunkTreeNode, StorageBackend};
use std::sync::Arc;
use std::time::Duration;
use storage_primitives::{hash_leaf, mmr::MmrLeaf, Commitment};

const BUCKET: u64 = 1;

type Backend = (Arc<dyn StorageBackend>, tempfile::TempDir);

/// A bucket that committed one single-chunk upload per id, in order. Chunks
/// with equal ids hold equal data.
fn bucket_with(ids: impl IntoIterator<Item = u8>) -> Backend {
    let (storage, dir) = temp_rocksdb();
    storage.init_bucket(BUCKET, u64::MAX).unwrap();
    for id in ids {
        let data = vec![id; 8];
        let hash = hash_leaf(&data);
        storage
            .store_node(BUCKET, hash, ChunkTreeNode::Chunk(data))
            .unwrap();
        storage.commit(BUCKET, vec![hash]).unwrap();
    }
    (storage, dir)
}

/// The commitment of the bucket's current state and its leaves.
fn state_of(storage: &dyn StorageBackend) -> (Commitment, Vec<MmrLeaf>) {
    let info = storage.get_bucket(BUCKET).unwrap();
    let end = info.start_seq + info.leaf_count;
    let leaves = storage
        .get_leaves(BUCKET, info.start_seq, end, usize::MAX, usize::MAX)
        .unwrap()
        .leaves;
    let commitment = Commitment {
        mmr_root: info.mmr_root,
        start_seq: info.start_seq,
        leaf_count: info.leaf_count,
    };
    (commitment, leaves)
}

fn client() -> SourceClient {
    SourceClient::new(Duration::from_secs(5))
}

async fn run(
    local: &Backend,
    sources: Vec<String>,
    target: &Commitment,
) -> Result<Vec<MmrLeaf>, Error> {
    fetch_leaves(
        local.0.as_ref(),
        &client(),
        &mut SourceSet::new(sources),
        BUCKET,
        target,
    )
    .await
}

#[tokio::test]
async fn only_the_missing_range_is_fetched() {
    let (target, leaves) = state_of(bucket_with(0..6).0.as_ref());
    let source = spawn_leaf_source(0, leaves.clone(), 100, usize::MAX).await;
    let local = bucket_with(0..3);

    let fetched = run(&local, vec![source.url.clone()], &target)
        .await
        .unwrap();

    assert_eq!(fetched, leaves);
    assert_eq!(source.requests(), [(3, 6)]);
}

#[tokio::test]
async fn nothing_is_fetched_when_the_local_leaves_cover_the_target() {
    let (target, leaves) = state_of(bucket_with(0..4).0.as_ref());
    let source = spawn_leaf_source(0, leaves.clone(), 100, usize::MAX).await;
    // The local bucket holds the target and two more leaves.
    let local = bucket_with(0..6);

    let fetched = run(&local, vec![source.url.clone()], &target)
        .await
        .unwrap();

    assert_eq!(fetched, leaves);
    assert!(source.requests().is_empty());
}

#[tokio::test]
async fn a_fresh_replica_fetches_the_whole_range_page_by_page() {
    let (target, leaves) = state_of(bucket_with(0..5).0.as_ref());
    let source = spawn_leaf_source(0, leaves.clone(), 2, usize::MAX).await;
    // No local bucket at all.
    let (empty, _dir) = temp_rocksdb();

    let fetched = fetch_leaves(
        empty.as_ref(),
        &client(),
        &mut SourceSet::new(vec![source.url.clone()]),
        BUCKET,
        &target,
    )
    .await
    .unwrap();

    assert_eq!(fetched, leaves);
    assert_eq!(source.requests(), [(0, 5), (2, 5), (4, 5)]);
}

#[tokio::test]
async fn local_leaves_before_the_target_start_are_dropped() {
    let (source_storage, _dir) = bucket_with(0..5);
    source_storage.delete_before(BUCKET, 2).unwrap();
    let (target, leaves) = state_of(source_storage.as_ref());
    assert_eq!(target.start_seq, 2);
    let source = spawn_leaf_source(2, leaves.clone(), 100, usize::MAX).await;
    // The local bucket has not deleted anything yet.
    let local = bucket_with(0..5);

    let fetched = run(&local, vec![source.url.clone()], &target)
        .await
        .unwrap();

    assert_eq!(fetched, leaves);
    assert!(source.requests().is_empty());
}

#[tokio::test]
async fn a_gap_before_the_local_leaves_keeps_nothing() {
    let (target, leaves) = state_of(bucket_with(0..5).0.as_ref());
    let source = spawn_leaf_source(0, leaves.clone(), 100, usize::MAX).await;
    // The local bucket has deleted the first leaves the target still has.
    let (local_storage, dir) = bucket_with(0..5);
    local_storage.delete_before(BUCKET, 2).unwrap();
    let local = (local_storage, dir);

    let fetched = run(&local, vec![source.url.clone()], &target)
        .await
        .unwrap();

    assert_eq!(fetched, leaves);
    assert_eq!(source.requests(), [(0, 5)]);
}

#[tokio::test]
async fn a_failing_source_is_skipped_for_the_next_one() {
    let (target, leaves) = state_of(bucket_with(0..4).0.as_ref());
    let good = spawn_leaf_source(0, leaves.clone(), 100, usize::MAX).await;
    let failures = [
        spawn_fixed(status(401.try_into().unwrap())).await,
        spawn_fixed(status(404.try_into().unwrap())).await,
        spawn_fixed(status(409.try_into().unwrap())).await,
        spawn_fixed(status(500.try_into().unwrap())).await,
        dead_address().await,
        // A page that breaks the paging rules.
        spawn_fixed(leaves_body(BUCKET, 1, 2, &leaves[..1])).await,
    ];

    for failing in failures {
        let local = bucket_with([]);
        let fetched = run(&local, vec![failing.clone(), good.url.clone()], &target)
            .await
            .unwrap();
        assert_eq!(fetched, leaves, "after {failing}");
    }
}

#[tokio::test]
async fn a_page_that_fails_midway_is_retried_from_its_start_on_the_next_source() {
    let (target, leaves) = state_of(bucket_with(0..5).0.as_ref());
    // Serves one page of two leaves, then fails.
    let flaky = spawn_leaf_source(0, leaves.clone(), 2, 1).await;
    let steady = spawn_leaf_source(0, leaves.clone(), 2, usize::MAX).await;
    let local = bucket_with([]);

    let fetched = run(&local, vec![flaky.url.clone(), steady.url.clone()], &target)
        .await
        .unwrap();

    assert_eq!(fetched, leaves);
    assert_eq!(flaky.requests(), [(0, 5), (2, 5)]);
    assert_eq!(steady.requests(), [(2, 5), (4, 5)]);
}

#[tokio::test]
async fn stale_local_leaves_are_replaced_by_the_whole_range() {
    let (target, leaves) = state_of(bucket_with(0..6).0.as_ref());
    let source = spawn_leaf_source(0, leaves.clone(), 100, usize::MAX).await;
    // Same length as the target's first leaves, different data.
    let local = bucket_with(100..103);

    let fetched = run(&local, vec![source.url.clone()], &target)
        .await
        .unwrap();

    assert_eq!(fetched, leaves);
    // The missing range first, then the whole range once the root was wrong.
    assert_eq!(source.requests(), [(3, 6), (0, 6)]);
}

#[tokio::test]
async fn a_source_with_wrong_leaves_is_dropped_and_the_next_one_used() {
    let (target, leaves) = state_of(bucket_with(0..5).0.as_ref());
    let (_, wrong) = state_of(bucket_with(50..55).0.as_ref());
    let liar = spawn_leaf_source(0, wrong, 100, usize::MAX).await;
    let honest = spawn_leaf_source(0, leaves.clone(), 100, usize::MAX).await;
    let local = bucket_with([]);

    let fetched = run(&local, vec![liar.url.clone(), honest.url.clone()], &target)
        .await
        .unwrap();

    assert_eq!(fetched, leaves);
    // The first mismatch does not say who is wrong, so the liar gets the whole
    // range once more; its second mismatch drops it.
    assert_eq!(liar.requests(), [(0, 5), (0, 5)]);
    assert_eq!(honest.requests(), [(0, 5)]);
}

#[tokio::test]
async fn no_source_left_is_an_error() {
    let (target, _) = state_of(bucket_with(0..3).0.as_ref());
    let (_, wrong) = state_of(bucket_with(50..53).0.as_ref());
    let liar = spawn_leaf_source(0, wrong, 100, usize::MAX).await;
    let local = bucket_with([]);

    let err = run(&local, vec![liar.url.clone()], &target)
        .await
        .unwrap_err();
    assert!(matches!(err, Error::NoSourceLeft { .. }), "{err:?}");

    let err = run(&local, vec![], &target).await.unwrap_err();
    assert!(matches!(err, Error::NoSourceLeft { .. }), "{err:?}");

    let down = dead_address().await;
    let err = run(&local, vec![down], &target).await.unwrap_err();
    assert!(matches!(err, Error::NoSourceLeft { .. }), "{err:?}");
}
