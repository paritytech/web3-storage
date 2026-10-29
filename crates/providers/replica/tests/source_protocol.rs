// SPDX-License-Identifier: Apache-2.0

//! `SourceClient` against mock sources: every way a source can fail or break
//! the paging rules, and what `SourceSet` does with a failed source.

mod common;

use axum::extract::State;
use axum::routing::post;
use axum::{Json, Router};
use common::*;
use provider_replica::source::{SourceClient, SourceSet};
use provider_replica::Error;
use provider_storage::ChunkTreeNode;
use serde_json::{json, Value};
use sp_core::H256;
use std::sync::{Arc, Mutex};
use std::time::Duration;
use storage_primitives::{hash_children, hash_leaf, mmr::MmrLeaf};

const BUCKET: u64 = 1;

fn client() -> SourceClient {
    SourceClient::new(Duration::from_secs(5))
}

fn leaf(i: u8) -> MmrLeaf {
    MmrLeaf {
        leaf_version: 1,
        data_root: H256::repeat_byte(i),
        data_size: 8,
        total_size: 8 * (u64::from(i) + 1),
    }
}

// ── /mmr_leaves ───────────────────────────────────────────────────────────────

#[tokio::test]
async fn a_leaf_page_is_returned_with_its_next_seq() {
    let leaves = [leaf(2), leaf(3)];
    let source = spawn_fixed(leaves_body(BUCKET, 2, 4, &leaves)).await;

    let page = client().mmr_leaves(&source, BUCKET, 2, 6).await.unwrap();

    assert_eq!(page.leaves, leaves);
    assert_eq!(page.next_seq, 4);
}

#[tokio::test]
async fn an_empty_range_is_an_empty_page() {
    let source = spawn_fixed(leaves_body(BUCKET, 4, 4, &[])).await;

    let page = client().mmr_leaves(&source, BUCKET, 4, 4).await.unwrap();

    assert!(page.leaves.is_empty());
    assert_eq!(page.next_seq, 4);
}

#[tokio::test]
async fn error_statuses_are_reported_with_their_code() {
    for code in [401, 404, 409, 500] {
        let source = spawn_fixed(status(code.try_into().unwrap())).await;

        let err = client()
            .mmr_leaves(&source, BUCKET, 0, 1)
            .await
            .unwrap_err();

        assert!(
            matches!(err, Error::SourceUnavailable { status, .. } if status == code),
            "{code}: {err:?}"
        );
    }
}

#[tokio::test]
async fn an_unreachable_source_is_a_transport_error() {
    let err = client()
        .mmr_leaves(&dead_address().await, BUCKET, 0, 1)
        .await
        .unwrap_err();

    assert!(matches!(err, Error::SourceRequest { .. }), "{err:?}");
}

#[tokio::test]
async fn a_source_that_stops_answering_times_out() {
    let source = spawn_slow(Duration::from_secs(10)).await;

    let err = SourceClient::new(Duration::from_millis(100))
        .mmr_leaves(&source, BUCKET, 0, 1)
        .await
        .unwrap_err();

    assert!(matches!(err, Error::SourceRequest { .. }), "{err:?}");
}

#[tokio::test]
async fn a_response_above_the_size_limit_is_a_bad_source() {
    let source = spawn_fixed(body(" ".repeat(17 * 1024 * 1024))).await;

    let err = client()
        .mmr_leaves(&source, BUCKET, 0, 1)
        .await
        .unwrap_err();

    assert!(matches!(err, Error::BadSource { .. }), "{err:?}");
}

#[tokio::test]
async fn a_malformed_leaf_body_is_a_decode_error() {
    let source = spawn_fixed(body("not json")).await;

    let err = client()
        .mmr_leaves(&source, BUCKET, 0, 1)
        .await
        .unwrap_err();

    assert!(matches!(err, Error::Decode { .. }), "{err:?}");
}

#[tokio::test]
async fn leaf_pages_that_break_the_paging_rules_are_a_bad_source() {
    let two = [leaf(0), leaf(1)];
    // Requested 0..4 of bucket 1 each time.
    let cases = [
        ("starts at the wrong seq", leaves_body(BUCKET, 1, 3, &two)),
        ("reaches past to_seq", leaves_body(BUCKET, 0, 5, &two)),
        (
            "has fewer leaves than it claims",
            leaves_body(BUCKET, 0, 3, &two),
        ),
        (
            "has more leaves than it claims",
            leaves_body(BUCKET, 0, 1, &two),
        ),
        ("makes no progress", leaves_body(BUCKET, 0, 0, &[])),
        ("is for another bucket", leaves_body(2, 0, 2, &two)),
    ];
    for (name, reply) in cases {
        let source = spawn_fixed(reply).await;

        let err = client()
            .mmr_leaves(&source, BUCKET, 0, 4)
            .await
            .unwrap_err();

        assert!(matches!(err, Error::BadSource { .. }), "{name}: {err:?}");
    }
}

// ── /fetch_nodes ──────────────────────────────────────────────────────────────

fn chunk(byte: u8) -> (H256, Value) {
    let data = vec![byte; 8];
    let hash = hash_leaf(&data);
    (
        hash,
        json!({ "hash": hex_hash(hash), "data": base64(&data) }),
    )
}

fn nodes_body(nodes: Vec<Value>, missing: &[H256], next: &[H256]) -> Reply {
    let hex = |hashes: &[H256]| hashes.iter().copied().map(hex_hash).collect::<Vec<_>>();
    body(json!({ "nodes": nodes, "missing": hex(missing), "next": hex(next) }).to_string())
}

#[tokio::test]
async fn a_node_page_returns_nodes_and_the_hashes_it_left_out() {
    let (chunk_hash, chunk_json) = chunk(1);
    let (left, right) = (hash_leaf(b"l"), hash_leaf(b"r"));
    let parent = hash_children(left, right);
    let parent_json =
        json!({ "hash": hex_hash(parent), "children": [hex_hash(left), hex_hash(right)] });
    let (missing, later) = (H256::repeat_byte(7), H256::repeat_byte(8));
    let source = spawn_fixed(nodes_body(
        vec![chunk_json, parent_json],
        &[missing],
        &[later],
    ))
    .await;

    let page = client()
        .fetch_nodes(&source, BUCKET, &[chunk_hash, parent, missing, later])
        .await
        .unwrap();

    assert_eq!(
        page.nodes,
        [
            (chunk_hash, ChunkTreeNode::Chunk(vec![1; 8])),
            (parent, ChunkTreeNode::Internal { left, right }),
        ]
    );
    assert_eq!(page.missing, [missing]);
    assert_eq!(page.next, [later]);
}

#[tokio::test]
async fn the_request_names_the_bucket_and_the_hashes() {
    let seen = Arc::new(Mutex::new(Vec::<Value>::new()));
    let (hash, node) = chunk(1);
    let reply = nodes_body(vec![node], &[], &[]);
    let app = Router::new()
        .route(
            "/fetch_nodes",
            post(
                |State((seen, reply)): State<(Arc<Mutex<Vec<Value>>>, Reply)>,
                 Json(request): Json<Value>| async move {
                    seen.lock().unwrap().push(request);
                    (reply.0, [("content-type", "application/json")], reply.1)
                },
            ),
        )
        .with_state((Arc::clone(&seen), reply));
    let source = spawn_app(app).await;

    client().fetch_nodes(&source, 9, &[hash]).await.unwrap();

    assert_eq!(
        seen.lock().unwrap().as_slice(),
        [json!({ "bucket_id": 9, "hashes": [hex_hash(hash)] })]
    );
}

#[tokio::test]
async fn a_node_that_does_not_hash_to_its_hash_is_rejected() {
    let (hash, _) = chunk(1);
    let (_, other_json) = chunk(2);
    // The node is a valid chunk, listed under the wrong hash.
    let forged = json!({ "hash": hex_hash(hash), "data": other_json["data"] });
    let source = spawn_fixed(nodes_body(vec![forged], &[], &[])).await;

    let err = client()
        .fetch_nodes(&source, BUCKET, &[hash])
        .await
        .unwrap_err();

    assert!(matches!(err, Error::NodeHashMismatch { .. }), "{err:?}");
}

#[tokio::test]
async fn node_pages_that_break_the_paging_rules_are_a_bad_source() {
    let (a, a_json) = chunk(1);
    let (b, _) = chunk(2);
    let (_, stranger_json) = chunk(3);
    let cases = [
        (
            "lists a hash it was not asked for",
            nodes_body(vec![a_json.clone(), stranger_json], &[b], &[]),
        ),
        (
            "lists a hash twice",
            nodes_body(vec![a_json.clone()], &[b], &[a]),
        ),
        (
            "leaves a requested hash out",
            nodes_body(vec![a_json.clone()], &[], &[]),
        ),
        ("makes no progress", nodes_body(vec![], &[], &[a, b])),
    ];
    for (name, reply) in cases {
        let source = spawn_fixed(reply).await;

        let err = client()
            .fetch_nodes(&source, BUCKET, &[a, b])
            .await
            .unwrap_err();

        assert!(matches!(err, Error::BadSource { .. }), "{name}: {err:?}");
    }
}

#[tokio::test]
async fn a_node_with_both_data_and_children_is_rejected() {
    let (hash, node) = chunk(1);
    let mut both = node;
    both["children"] = json!([hex_hash(hash), hex_hash(hash)]);
    let source = spawn_fixed(nodes_body(vec![both], &[], &[])).await;

    let err = client()
        .fetch_nodes(&source, BUCKET, &[hash])
        .await
        .unwrap_err();

    assert!(matches!(err, Error::Backend(_)), "{err:?}");
}

#[tokio::test]
async fn node_requests_report_error_statuses() {
    let source = spawn_fixed(status(503.try_into().unwrap())).await;

    let err = client()
        .fetch_nodes(&source, BUCKET, &[H256::repeat_byte(1)])
        .await
        .unwrap_err();

    assert!(
        matches!(err, Error::SourceUnavailable { status: 503, .. }),
        "{err:?}"
    );
}

// ── SourceSet ─────────────────────────────────────────────────────────────────

#[test]
fn sources_are_tried_in_the_given_order_and_dropped_for_the_pass() {
    let mut sources = SourceSet::new(vec!["http://primary".into(), "http://replica".into()]);
    assert_eq!(sources.current(), Some("http://primary"));

    sources.drop_source("http://primary");
    assert_eq!(sources.current(), Some("http://replica"));

    sources.drop_source("http://replica");
    assert_eq!(sources.current(), None);
}

#[test]
fn dropping_an_unknown_source_changes_nothing() {
    let mut sources = SourceSet::new(vec!["http://primary".into()]);

    sources.drop_source("http://other");

    assert_eq!(sources.current(), Some("http://primary"));
}
