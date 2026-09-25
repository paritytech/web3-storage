// SPDX-License-Identifier: Apache-2.0

//! HTTP-level tests for the replica sync protocol: what `ReplicaSync` does
//! with the answers a primary provider gives it, including the failing and
//! the malformed ones. A primary is another provider, not a trusted peer, so
//! every one of its answers is an input this code must handle.
//!
//! The mock primary answers what this code asks for, not what a real one
//! returns - see the two defect tests below. These cover the client's
//! handling, not the protocol working end to end.

mod common;

use axum::http::StatusCode;
use codec::Encode;
use common::{
    base64, body, dead_address, hex_hash, node_body, peaks_body, spawn_primary,
    spawn_primary_with_nodes, status, test_storage, Reply,
};
use provider_replica::{Error, ReplicaSync};
use provider_storage::{ChunkTreeNode, StorageBackend};
use sp_core::H256;
use std::collections::HashMap;
use std::sync::Arc;
use storage_primitives::{blake2_256, hash_children, BucketId, MmrLeaf};
use tempfile::TempDir;

const BUCKET: BucketId = 1;

/// Asserts the error matches `$pattern`, printing it when it does not.
macro_rules! assert_err {
    ($result:expr, $pattern:pat) => {
        let err = $result.unwrap_err();
        assert!(matches!(err, $pattern), "got {err}");
    };
}

struct Fixture {
    sync: ReplicaSync,
    storage: Arc<dyn StorageBackend>,
    /// Dropping this takes the database with it.
    _dir: TempDir,
}

fn fixture() -> Fixture {
    let (storage, dir) = test_storage();
    Fixture {
        sync: ReplicaSync::new(Arc::clone(&storage)),
        storage,
        _dir: dir,
    }
}

/// A root no local bucket holds, so a sync never short-circuits.
fn unheld_root() -> H256 {
    H256::repeat_byte(0xAB)
}

/// Sync against a primary whose `/mmr_peaks` answers `peaks`.
async fn sync_serving_peaks(peaks: Reply) -> Result<H256, Error> {
    let f = fixture();
    let url = spawn_primary(peaks).await;
    f.sync.sync_from_primary(BUCKET, &url).await
}

/// Sync against a primary offering one peak, whose `/node` answers `reply`.
async fn sync_serving_node(peak: H256, reply: Reply) -> Result<H256, Error> {
    let f = fixture();
    let nodes = HashMap::from([(hex_hash(peak), reply)]);
    let url = spawn_primary_with_nodes(
        peaks_body(&hex_hash(unheld_root()), &[hex_hash(peak)]),
        nodes,
    )
    .await;
    f.sync.sync_from_primary(BUCKET, &url).await
}

#[tokio::test]
async fn unreachable_primary_is_a_transport_error() {
    let f = fixture();
    let url = dead_address().await;

    assert_err!(
        f.sync.sync_from_primary(BUCKET, &url).await,
        Error::PrimaryRequest {
            what: "mmr peaks",
            ..
        }
    );
}

#[tokio::test]
async fn peaks_error_status_is_reported_with_the_status_code() {
    assert_err!(
        sync_serving_peaks(status(StatusCode::SERVICE_UNAVAILABLE)).await,
        Error::PrimaryUnavailable {
            what: "mmr peaks",
            status: 503
        }
    );
}

#[tokio::test]
async fn malformed_peaks_body_is_a_decode_error() {
    assert_err!(
        sync_serving_peaks(body("not json at all")).await,
        Error::Decode {
            what: "mmr peaks response",
            ..
        }
    );
}

#[tokio::test]
async fn non_hex_mmr_root_is_a_decode_error() {
    assert_err!(
        sync_serving_peaks(peaks_body("0xnothex", &[])).await,
        Error::Decode {
            what: "mmr_root",
            ..
        }
    );
}

#[tokio::test]
async fn non_hex_peak_is_a_decode_error() {
    assert_err!(
        sync_serving_peaks(peaks_body(
            &hex_hash(unheld_root()),
            &["0xnothex".to_string()]
        ))
        .await,
        Error::Decode {
            what: "peak hash",
            ..
        }
    );
}

#[tokio::test]
async fn node_error_status_is_reported_with_the_status_code() {
    assert_err!(
        sync_serving_node(
            H256::repeat_byte(0x11),
            status(StatusCode::INTERNAL_SERVER_ERROR)
        )
        .await,
        Error::PrimaryUnavailable {
            what: "node",
            status: 500
        }
    );
}

#[tokio::test]
async fn malformed_node_body_is_a_decode_error() {
    assert_err!(
        sync_serving_node(H256::repeat_byte(0x11), body("{")).await,
        Error::Decode {
            what: "node response",
            ..
        }
    );
}

#[tokio::test]
async fn non_base64_node_data_is_a_decode_error() {
    let peak = H256::repeat_byte(0x11);
    assert_err!(
        sync_serving_node(peak, node_body(&hex_hash(peak), "not base64 !!", None)).await,
        Error::Decode {
            what: "node data",
            ..
        }
    );
}

#[tokio::test]
async fn non_hex_child_hash_is_a_decode_error() {
    let peak = H256::repeat_byte(0x11);
    let reply = node_body(
        &hex_hash(peak),
        &base64(b"payload"),
        Some(vec!["0xnothex".to_string()]),
    );
    assert_err!(
        sync_serving_node(peak, reply).await,
        Error::Decode {
            what: "child hash",
            ..
        }
    );
}

#[tokio::test]
async fn a_node_whose_hash_does_not_match_its_data_is_rejected() {
    let claimed = H256::repeat_byte(0x11);
    let reply = node_body(
        &hex_hash(claimed),
        &base64(b"payload hashing to something else"),
        None,
    );
    assert_err!(
        sync_serving_node(claimed, reply).await,
        Error::Backend(provider_storage::Error::InvalidHash { .. })
    );
}

/// KNOWN DEFECT: `/mmr_peaks` answers with MMR hashes - the provider builds
/// them as `blake2_256(MmrLeaf.encode())` plus internal MMR hashes that are
/// never persisted - while `/node` is keyed by chunk-data hash. So every sync
/// of a non-empty bucket ends at this 404, whatever the tree shape. Described
/// in #392 (closed as not planned), fixed by the rework in #65, caught by the
/// end-to-end test in #420.
#[tokio::test]
async fn peak_hashes_are_not_node_keys_so_every_real_sync_404s() {
    // An MMR peak: the hash of a leaf record, not of any stored chunk, so the
    // mock has no `/node` entry for it - exactly as a real primary would not.
    let peak = blake2_256(
        &MmrLeaf {
            data_root: blake2_256(b"chunk"),
            data_size: 5,
            total_size: 5,
        }
        .encode(),
    );
    let f = fixture();
    let url = spawn_primary(peaks_body(&hex_hash(unheld_root()), &[hex_hash(peak)])).await;

    assert_err!(
        f.sync.sync_from_primary(BUCKET, &url).await,
        Error::PrimaryUnavailable {
            what: "node",
            status: 404
        }
    );
}

/// The defect #392 and #65 describe - `fetch_subtree` storing a node before
/// the children it names exist - is fixed: children are fetched first, so an
/// internal peak stores cleanly together with the subtree beneath it.
#[tokio::test]
async fn an_internal_peak_is_stored_after_the_children_beneath_it() {
    let left_data = b"left-chunk".to_vec();
    let left = blake2_256(&left_data);
    let right_data = b"right-chunk".to_vec();
    let right = blake2_256(&right_data);
    // An internal node is identified by its children, and a real primary
    // serves their concatenation as its `data`.
    let parent = hash_children(left, right);
    let parent_data = [left.as_bytes(), right.as_bytes()].concat();

    let nodes = HashMap::from([
        (
            hex_hash(parent),
            node_body(
                &hex_hash(parent),
                &base64(&parent_data),
                Some(vec![hex_hash(left), hex_hash(right)]),
            ),
        ),
        (
            hex_hash(left),
            node_body(&hex_hash(left), &base64(&left_data), None),
        ),
        (
            hex_hash(right),
            node_body(&hex_hash(right), &base64(&right_data), None),
        ),
    ]);
    let f = fixture();
    f.storage.init_bucket(BUCKET, u64::MAX).unwrap();
    let target = unheld_root();
    let url =
        spawn_primary_with_nodes(peaks_body(&hex_hash(target), &[hex_hash(parent)]), nodes).await;

    assert_eq!(
        f.sync.sync_from_primary(BUCKET, &url).await.unwrap(),
        target
    );
    assert_eq!(
        f.storage.get_node(&parent),
        Some(ChunkTreeNode::Internal([left, right])),
        "the parent must be stored once its children are present"
    );
    assert_eq!(
        f.storage.get_node(&left),
        Some(ChunkTreeNode::Chunk(left_data))
    );
    assert_eq!(
        f.storage.get_node(&right),
        Some(ChunkTreeNode::Chunk(right_data))
    );
}

#[tokio::test]
async fn a_root_we_already_hold_returns_without_fetching_any_node() {
    let f = fixture();
    let data = b"already stored".to_vec();
    let leaf = blake2_256(&data);
    f.storage.init_bucket(BUCKET, u64::MAX).unwrap();
    f.storage
        .store_node(BUCKET, leaf, ChunkTreeNode::Chunk(data))
        .unwrap();
    let (local_root, _, _) = f.storage.commit(BUCKET, vec![leaf]).unwrap();

    // Answering the leaf with an error proves `/node` is never requested.
    let nodes = HashMap::from([(hex_hash(leaf), status(StatusCode::INTERNAL_SERVER_ERROR))]);
    let url =
        spawn_primary_with_nodes(peaks_body(&hex_hash(local_root), &[hex_hash(leaf)]), nodes).await;

    assert_eq!(
        f.sync.sync_from_primary(BUCKET, &url).await.unwrap(),
        local_root
    );
}

#[tokio::test]
async fn a_node_we_already_hold_is_not_refetched() {
    let f = fixture();
    let data = b"cached payload".to_vec();
    let leaf = blake2_256(&data);
    f.storage.init_bucket(BUCKET, u64::MAX).unwrap();
    f.storage
        .store_node(BUCKET, leaf, ChunkTreeNode::Chunk(data))
        .unwrap();

    let nodes = HashMap::from([(hex_hash(leaf), status(StatusCode::INTERNAL_SERVER_ERROR))]);
    let target = unheld_root();
    let url =
        spawn_primary_with_nodes(peaks_body(&hex_hash(target), &[hex_hash(leaf)]), nodes).await;

    assert_eq!(
        f.sync.sync_from_primary(BUCKET, &url).await.unwrap(),
        target
    );
}

/// Covers `fetch_subtree`'s fetch-decode-store path. The mock serves a peak
/// hash that is also a key in the node store, which a real primary cannot do
/// - see `peak_hashes_are_not_node_keys_so_every_real_sync_404s` and #392.
#[tokio::test]
async fn a_peak_naming_a_stored_node_is_fetched_and_stored() {
    let f = fixture();
    let data = b"leaf payload".to_vec();
    let leaf = blake2_256(&data);
    let target = unheld_root();

    let nodes = HashMap::from([(
        hex_hash(leaf),
        node_body(&hex_hash(leaf), &base64(&data), None),
    )]);
    let url =
        spawn_primary_with_nodes(peaks_body(&hex_hash(target), &[hex_hash(leaf)]), nodes).await;

    // `sync_from_primary` reports the root the primary claims; it never
    // recomputes one, which is why the coordinator verifies afterwards.
    assert_eq!(
        f.sync.sync_from_primary(BUCKET, &url).await.unwrap(),
        target
    );
    assert_eq!(
        f.storage.get_node(&leaf),
        Some(ChunkTreeNode::Chunk(b"leaf payload".to_vec()))
    );
}
