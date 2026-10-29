// SPDX-License-Identifier: Apache-2.0

//! `ReplicaSync` against mock sources serving real buckets: what a replica
//! copies, in what order, and how it reacts to sources that fail or lie.

mod common;

use common::*;
use provider_replica::source::SourceSet;
use provider_replica::{Error, ReplicaSync};
use provider_storage::{ChunkTreeNode, StorageBackend};
use sp_core::H256;
use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use storage_primitives::Commitment;

const BUCKET: u64 = 1;

type Replica = (Arc<dyn StorageBackend>, tempfile::TempDir);

/// A replica with an empty bucket of `max_bytes`.
fn replica(max_bytes: u64) -> Replica {
    let (storage, dir) = provider_storage::temp_rocksdb();
    storage.init_bucket(BUCKET, max_bytes).unwrap();
    (storage, dir)
}

async fn sync(
    replica: &Replica,
    target: &Commitment,
    sources: &mut SourceSet,
) -> Result<(), Error> {
    ReplicaSync::new(Arc::clone(&replica.0))
        .sync_to_target(BUCKET, target, sources)
        .await
}

fn sources(urls: &[&str]) -> SourceSet {
    SourceSet::new(urls.iter().map(|url| url.to_string()).collect())
}

/// Every node under the data roots of `storage`'s leaves, with its depth below
/// its data root.
fn tree_depths(storage: &dyn StorageBackend) -> HashMap<H256, usize> {
    let info = storage.get_bucket(BUCKET).unwrap();
    let leaves = storage
        .get_leaves(
            BUCKET,
            info.start_seq,
            info.start_seq + info.leaf_count,
            usize::MAX,
            usize::MAX,
        )
        .unwrap()
        .leaves;
    let mut depths = HashMap::new();
    let mut pending: Vec<(H256, usize)> = leaves.iter().map(|leaf| (leaf.data_root, 0)).collect();
    while let Some((hash, depth)) = pending.pop() {
        depths.insert(hash, depth);
        if let Some(ChunkTreeNode::Internal { left, right }) = storage.get_node(&hash) {
            pending.push((left, depth + 1));
            pending.push((right, depth + 1));
        }
    }
    depths
}

fn assert_synced(source: &dyn StorageBackend, replica: &dyn StorageBackend, target: &Commitment) {
    let info = replica.get_bucket(BUCKET).unwrap();
    assert_eq!(
        (info.mmr_root, info.start_seq, info.leaf_count),
        (target.mmr_root, target.start_seq, target.leaf_count)
    );
    for hash in tree_depths(source).into_keys() {
        assert_eq!(replica.get_node(&hash), source.get_node(&hash));
    }
}

#[tokio::test]
async fn a_fresh_replica_copies_the_bucket_and_installs_the_target() {
    let (source, _dir, target) = source_bucket(&[3, 1, 5]);
    let node = spawn_node_source(Arc::clone(&source), NodeSourceConfig::default()).await;
    let replica = replica(u64::MAX);

    sync(&replica, &target, &mut sources(&[&node.url]))
        .await
        .unwrap();

    assert_synced(source.as_ref(), replica.0.as_ref(), &target);
    let leaf = source
        .get_leaves(BUCKET, 0, 3, usize::MAX, usize::MAX)
        .unwrap()
        .leaves[2]
        .data_root;
    assert_eq!(replica.0.collect_chunks(leaf), source.collect_chunks(leaf));
}

#[tokio::test]
async fn data_deleted_from_the_target_is_not_fetched() {
    let (source, _dir, _) = source_bucket(&[2, 2, 2]);
    let deleted_root = source
        .get_leaves(BUCKET, 0, 1, usize::MAX, usize::MAX)
        .unwrap()
        .leaves[0]
        .data_root;
    source.delete_before(BUCKET, 1).unwrap();
    let target = commitment_of(source.as_ref());
    assert_eq!(target.start_seq, 1);
    let node = spawn_node_source(Arc::clone(&source), NodeSourceConfig::default()).await;
    let replica = replica(u64::MAX);

    sync(&replica, &target, &mut sources(&[&node.url]))
        .await
        .unwrap();

    assert_eq!(replica.0.get_bucket(BUCKET).unwrap().start_seq, 1);
    assert!(!node.requested().contains(&deleted_root));
    assert!(replica.0.get_node(&deleted_root).is_none());
}

#[tokio::test]
async fn each_request_holds_one_level_and_the_levels_go_down_the_tree() {
    let (source, _dir, target) = source_bucket(&[8]);
    let depths = tree_depths(source.as_ref());
    let node = spawn_node_source(Arc::clone(&source), NodeSourceConfig::default()).await;
    let replica = replica(u64::MAX);

    sync(&replica, &target, &mut sources(&[&node.url]))
        .await
        .unwrap();

    let levels: Vec<HashSet<usize>> = node
        .node_requests()
        .iter()
        .map(|hashes| hashes.iter().map(|hash| depths[hash]).collect())
        .collect();
    let expected: Vec<HashSet<usize>> = (0..4).map(|depth| HashSet::from([depth])).collect();
    assert_eq!(levels, expected);
}

#[tokio::test]
async fn an_interrupted_sync_resumes_without_fetching_stored_nodes() {
    let (source, _dir, target) = source_bucket(&[4, 4, 4]);
    let replica = replica(u64::MAX);

    // Pages of three nodes, and the source fails after six requests: part of
    // the trees is stored.
    let flaky = spawn_node_source(
        Arc::clone(&source),
        NodeSourceConfig {
            max_items: 3,
            max_node_requests: 6,
            ..Default::default()
        },
    )
    .await;
    let err = sync(&replica, &target, &mut sources(&[&flaky.url]))
        .await
        .unwrap_err();
    assert!(matches!(err, Error::NoSourceLeft { .. }), "{err:?}");
    assert!(replica.0.get_bucket(BUCKET).unwrap().leaf_count == 0);

    let stored: HashSet<H256> = tree_depths(source.as_ref())
        .into_keys()
        .filter(|hash| replica.0.get_node(hash).is_some())
        .collect();
    assert!(!stored.is_empty(), "the interrupted sync stored nothing");

    let steady = spawn_node_source(Arc::clone(&source), NodeSourceConfig::default()).await;
    sync(&replica, &target, &mut sources(&[&steady.url]))
        .await
        .unwrap();

    assert_synced(source.as_ref(), replica.0.as_ref(), &target);
    assert!(
        steady.requested().is_disjoint(&stored),
        "stored nodes were requested again"
    );
}

#[tokio::test]
async fn a_source_serving_wrong_data_is_dropped_for_the_next_one() {
    let (source, _dir, target) = source_bucket(&[3, 3]);
    let liar = spawn_node_source(
        Arc::clone(&source),
        NodeSourceConfig {
            tamper: true,
            ..Default::default()
        },
    )
    .await;
    let honest = spawn_node_source(Arc::clone(&source), NodeSourceConfig::default()).await;
    let replica = replica(u64::MAX);
    let mut set = sources(&[&liar.url, &honest.url]);

    sync(&replica, &target, &mut set).await.unwrap();

    assert_synced(source.as_ref(), replica.0.as_ref(), &target);
    assert_eq!(set.current(), Some(honest.url.as_str()));
    // The liar served the internal levels and was dropped at the chunks.
    assert!(!liar.node_requests().is_empty());
}

#[tokio::test]
async fn a_hash_missing_from_one_source_is_fetched_from_the_next() {
    let (source, _dir, target) = source_bucket(&[4]);
    let depths = tree_depths(source.as_ref());
    let hidden = *depths.iter().find(|(_, depth)| **depth == 2).unwrap().0;
    let partial = spawn_node_source(
        Arc::clone(&source),
        NodeSourceConfig {
            hidden: HashSet::from([hidden]),
            ..Default::default()
        },
    )
    .await;
    let complete = spawn_node_source(Arc::clone(&source), NodeSourceConfig::default()).await;
    let replica = replica(u64::MAX);

    sync(
        &replica,
        &target,
        &mut sources(&[&partial.url, &complete.url]),
    )
    .await
    .unwrap();

    assert_synced(source.as_ref(), replica.0.as_ref(), &target);
    // Only the hidden hash needed the second source.
    assert_eq!(complete.requested(), HashSet::from([hidden]));
}

#[tokio::test]
async fn a_hash_missing_from_every_source_fails_the_sync() {
    let (source, _dir, target) = source_bucket(&[4]);
    let hidden = *tree_depths(source.as_ref())
        .iter()
        .find(|(_, depth)| **depth == 2)
        .unwrap()
        .0;
    let config = NodeSourceConfig {
        hidden: HashSet::from([hidden]),
        ..Default::default()
    };
    let (a, b) = (
        spawn_node_source(Arc::clone(&source), config.clone()).await,
        spawn_node_source(Arc::clone(&source), config).await,
    );
    let replica = replica(u64::MAX);

    let err = sync(&replica, &target, &mut sources(&[&a.url, &b.url]))
        .await
        .unwrap_err();

    assert!(matches!(err, Error::NoSourceLeft { .. }), "{err:?}");
    assert_eq!(replica.0.get_bucket(BUCKET).unwrap().leaf_count, 0);
}

#[tokio::test]
async fn hashes_a_source_did_not_process_are_requested_from_it_again() {
    let (source, _dir, target) = source_bucket(&[4, 4]);
    let limited = spawn_node_source(
        Arc::clone(&source),
        NodeSourceConfig {
            max_items: 2,
            ..Default::default()
        },
    )
    .await;
    let spare = spawn_node_source(Arc::clone(&source), NodeSourceConfig::default()).await;
    let replica = replica(u64::MAX);

    sync(&replica, &target, &mut sources(&[&limited.url, &spare.url]))
        .await
        .unwrap();

    assert_synced(source.as_ref(), replica.0.as_ref(), &target);
    assert!(limited.node_requests().len() > 2);
    assert!(spare.node_requests().is_empty());
}

#[tokio::test]
async fn a_source_cannot_choose_the_target() {
    let (other, _dir, _) = source_bucket(&[2, 2]);
    let (_, _chain_dir, chain_target) = source_bucket(&[3]);
    let node = spawn_node_source(Arc::clone(&other), NodeSourceConfig::default()).await;
    let replica = replica(u64::MAX);

    let err = sync(&replica, &chain_target, &mut sources(&[&node.url]))
        .await
        .unwrap_err();

    assert!(matches!(err, Error::NoSourceLeft { .. }), "{err:?}");
    assert_eq!(replica.0.get_bucket(BUCKET).unwrap().leaf_count, 0);
    assert_eq!(replica.0.total_nodes(), 0);
    assert!(node.node_requests().is_empty());
}

#[tokio::test]
async fn running_out_of_quota_stops_the_sync_without_blaming_the_source() {
    let (source, _dir, target) = source_bucket(&[3]);
    let node = spawn_node_source(Arc::clone(&source), NodeSourceConfig::default()).await;
    let replica = replica(100);
    let mut set = sources(&[&node.url]);

    let err = sync(&replica, &target, &mut set).await.unwrap_err();

    assert!(
        matches!(
            err,
            Error::Backend(provider_storage::Error::QuotaExceeded { .. })
        ),
        "{err:?}"
    );
    assert_eq!(set.current(), Some(node.url.as_str()));
    assert_eq!(replica.0.get_bucket(BUCKET).unwrap().leaf_count, 0);
}
