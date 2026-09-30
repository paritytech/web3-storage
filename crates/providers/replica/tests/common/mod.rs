// SPDX-License-Identifier: Apache-2.0

//! Shared harness for the crate's integration tests: a storage fixture and a
//! mock primary provider.
//!
//! Each test binary compiles this module separately and uses part of it, so
//! unused items here are expected.
#![allow(dead_code)]

use axum::extract::Query;
use axum::http::{header, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::get;
use axum::Router;
use base64::Engine;
use provider_storage::{temp_rocksdb, StorageBackend};
use sp_core::H256;
use std::collections::HashMap;
use std::sync::Arc;
use tempfile::TempDir;

/// One canned answer from the mock primary: a status and a body. Tests serve
/// bodies a real primary could not produce, so this is a raw string rather
/// than a typed response.
pub type Reply = (StatusCode, String);

/// A non-success answer with an empty body.
pub fn status(status: StatusCode) -> Reply {
    (status, String::new())
}

/// A 200 answer with this body, valid JSON or not.
pub fn body(body: impl Into<String>) -> Reply {
    (StatusCode::OK, body.into())
}

/// Fresh empty storage backend. The returned `TempDir` must outlive the
/// backend - dropping it takes the database with it.
pub fn test_storage() -> (Arc<dyn StorageBackend>, TempDir) {
    let (storage, _nonce_store, dir) = temp_rocksdb();
    (storage, dir)
}

/// `0x`-prefixed hex, the form both provider endpoints use for hashes.
pub fn hex_hash(hash: H256) -> String {
    format!("0x{}", hex::encode(hash.as_bytes()))
}

pub fn base64(data: &[u8]) -> String {
    base64::engine::general_purpose::STANDARD.encode(data)
}

/// A `/mmr_peaks` body naming `mmr_root` and `peaks` verbatim.
pub fn peaks_body(mmr_root: &str, peaks: &[String]) -> Reply {
    body(serde_json::json!({ "bucket_id": 1, "mmr_root": mmr_root, "peaks": peaks }).to_string())
}

/// A `/node` body carrying `data` and `children` verbatim.
pub fn node_body(hash: &str, data: &str, children: Option<Vec<String>>) -> Reply {
    body(serde_json::json!({ "hash": hash, "data": data, "children": children }).to_string())
}

#[derive(serde::Deserialize)]
struct NodeQuery {
    hash: String,
}

fn render(reply: &Reply) -> Response {
    (
        reply.0,
        [(header::CONTENT_TYPE, "application/json")],
        reply.1.clone(),
    )
        .into_response()
}

/// Mock primary serving only `/mmr_peaks`.
pub async fn spawn_primary(peaks: Reply) -> String {
    spawn_primary_with_nodes(peaks, HashMap::new()).await
}

/// Mock primary: `/mmr_peaks` always answers `peaks`; `/node` answers the
/// entry keyed by the requested `0x`-prefixed hash, or 404 for a hash it was
/// not given - as a real primary does for a hash it never stored.
pub async fn spawn_primary_with_nodes(peaks: Reply, nodes: HashMap<String, Reply>) -> String {
    let nodes = Arc::new(nodes);
    let app = Router::new()
        .route(
            "/mmr_peaks",
            get(move || {
                let peaks = peaks.clone();
                async move { render(&peaks) }
            }),
        )
        .route(
            "/node",
            get(move |Query(query): Query<NodeQuery>| {
                let nodes = Arc::clone(&nodes);
                async move {
                    render(
                        nodes
                            .get(&query.hash)
                            .unwrap_or(&status(StatusCode::NOT_FOUND)),
                    )
                }
            }),
        );

    // `bind` has already called listen(2), so the port accepts connections
    // before the serve task is first polled: no readiness wait is needed.
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    format!("http://{addr}")
}

/// An address with nothing listening on it: bind a port, then release it.
pub async fn dead_address() -> String {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    drop(listener);
    format!("http://{addr}")
}
