// SPDX-License-Identifier: GPL-3.0-only

//! Integration tests that exercise the HTTP API with DiskStorage (RocksDB).
//!
//! These mirror the core flows in `api_integration.rs` but use the persistent
//! disk backend instead of in-memory storage, ensuring the full RocksDB
//! serialization/deserialization path is exercised through real HTTP requests.

use axum::http::StatusCode;
use base64::{engine::general_purpose::STANDARD as BASE64, Engine};
use reqwest::Client;
use serde_json::{json, Value};
use std::sync::Arc;
use storage_provider_node::{create_router, DiskStorage, ProviderState};
use tempfile::TempDir;
use tokio::net::TcpListener;

struct DiskTestServer {
    addr: std::net::SocketAddr,
    client: Client,
    // Keep TempDir alive so RocksDB path isn't removed
    _dir: TempDir,
}

impl DiskTestServer {
    async fn new() -> Self {
        let dir = TempDir::new().unwrap();
        let disk = DiskStorage::new(dir.path()).expect("RocksDB should open");
        // Functional tests, not auth tests: run with auth disabled (auth is
        // enforced by default; see auth_integration.rs for the auth coverage).
        let state = Arc::new(
            ProviderState::with_seed(Arc::new(disk), "//Alice")
                .expect("//Alice is valid")
                .with_auth_disabled(),
        );

        let app = create_router(state);
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });

        Self {
            addr,
            client: Client::new(),
            _dir: dir,
        }
    }

    fn url(&self, path: &str) -> String {
        format!("http://{}{}", self.addr, path)
    }
}

/// Upload a chunk, commit it, return (hash_hex, commit_body).
async fn upload_and_commit(server: &DiskTestServer, bucket_id: u64) -> (String, Value) {
    let data = b"chunk-for-disk-tests";
    let hash = storage_primitives::blake2_256(data);
    let hash_hex = format!("0x{}", hex_encode(hash.as_bytes()));

    let resp = server
        .client
        .put(server.url("/node"))
        .json(&json!({
            "bucket_id": bucket_id,
            "hash": hash_hex,
            "data": BASE64.encode(data),
            "children": null,
        }))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);

    let commit_resp = server
        .client
        .post(server.url("/commit"))
        .json(&json!({
            "bucket_id": bucket_id,
            "data_roots": [hash_hex],
            "nonce": 0u64,
        }))
        .send()
        .await
        .unwrap();

    let status = commit_resp.status();
    let body: Value = commit_resp.json().await.unwrap();
    assert_eq!(status, StatusCode::OK, "commit failed: {body:?}");
    (hash_hex, body)
}

// ─────────────────────────────────────────────────────────────────────────────
// Core API flows on DiskStorage
// ─────────────────────────────────────────────────────────────────────────────

#[tokio::test]
async fn disk_upload_and_download_node() {
    let server = DiskTestServer::new().await;

    let data = b"Hello from disk!";
    let hash = storage_primitives::blake2_256(data);
    let hash_hex = format!("0x{}", hex_encode(hash.as_bytes()));

    // Upload
    let resp = server
        .client
        .put(server.url("/node"))
        .json(&json!({
            "bucket_id": 1,
            "hash": hash_hex,
            "data": BASE64.encode(data),
            "children": null,
        }))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);

    // Download
    let resp = server
        .client
        .get(server.url(&format!("/node?hash={hash_hex}")))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);

    let body: Value = resp.json().await.unwrap();
    let downloaded = BASE64.decode(body["data"].as_str().unwrap()).unwrap();
    assert_eq!(downloaded, data);
}

#[tokio::test]
async fn disk_commit_and_commitment() {
    let server = DiskTestServer::new().await;
    let (_hash_hex, body) = upload_and_commit(&server, 1).await;

    assert!(body["mmr_root"].is_string());
    assert_eq!(body["start_seq"], 0);
    assert_eq!(body["leaf_indices"], json!([0]));
    assert!(body["provider_signature"].is_string());

    // GET /commitment should return consistent state
    let resp = server
        .client
        .get(server.url("/commitment?bucket_id=1&nonce=0"))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);

    let commitment: Value = resp.json().await.unwrap();
    assert_eq!(commitment["bucket_id"], 1);
    assert_eq!(commitment["leaf_count"], 1);
    assert_eq!(commitment["mmr_root"], body["mmr_root"]);
}

#[tokio::test]
async fn disk_check_exists() {
    let server = DiskTestServer::new().await;

    let data = b"exists-check-disk";
    let hash = storage_primitives::blake2_256(data);
    let hash_hex = format!("0x{}", hex_encode(hash.as_bytes()));

    server
        .client
        .put(server.url("/node"))
        .json(&json!({
            "bucket_id": 1,
            "hash": hash_hex,
            "data": BASE64.encode(data),
            "children": null,
        }))
        .send()
        .await
        .unwrap();

    let fake = "0x0000000000000000000000000000000000000000000000000000000000000001";
    let resp = server
        .client
        .post(server.url("/exists"))
        .json(&json!({ "bucket_id": 1, "hashes": [hash_hex, fake] }))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);

    let body: Value = resp.json().await.unwrap();
    assert!(body["exists"]
        .as_array()
        .unwrap()
        .iter()
        .any(|h| h == &hash_hex));
    assert!(body["missing"]
        .as_array()
        .unwrap()
        .iter()
        .any(|h| h == fake));
}

#[tokio::test]
async fn disk_mmr_proof() {
    let server = DiskTestServer::new().await;
    upload_and_commit(&server, 1).await;

    let resp = server
        .client
        .get(server.url("/mmr_proof?bucket_id=1&leaf_index=0"))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);

    let body: Value = resp.json().await.unwrap();
    assert!(body["leaf"]["data_root"].is_string());
    assert!(body["proof"]["peaks"].is_array());
}

#[tokio::test]
async fn disk_chunk_proof() {
    let server = DiskTestServer::new().await;
    let (hash_hex, _) = upload_and_commit(&server, 1).await;

    let resp = server
        .client
        .get(server.url(&format!("/chunk_proof?data_root={hash_hex}&chunk_index=0")))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);

    let body: Value = resp.json().await.unwrap();
    assert!(body["chunk_hash"].is_string());
    assert!(body["chunk_data"].is_string());
}

#[tokio::test]
async fn disk_mmr_peaks() {
    let server = DiskTestServer::new().await;
    upload_and_commit(&server, 1).await;

    let resp = server
        .client
        .get(server.url("/mmr_peaks?bucket_id=1"))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);

    let body: Value = resp.json().await.unwrap();
    assert!(body["mmr_root"].is_string());
    assert!(!body["peaks"].as_array().unwrap().is_empty());
}

#[tokio::test]
async fn disk_delete_before() {
    let server = DiskTestServer::new().await;

    // Commit two data roots
    let data1 = b"disk-leaf-0";
    let h1 = storage_primitives::blake2_256(data1);
    let h1_hex = format!("0x{}", hex_encode(h1.as_bytes()));

    let data2 = b"disk-leaf-1";
    let h2 = storage_primitives::blake2_256(data2);
    let h2_hex = format!("0x{}", hex_encode(h2.as_bytes()));

    for (hash_hex, data) in [(&h1_hex, &data1[..]), (&h2_hex, &data2[..])] {
        server
            .client
            .put(server.url("/node"))
            .json(&json!({
                "bucket_id": 1,
                "hash": hash_hex,
                "data": BASE64.encode(data),
                "children": null,
            }))
            .send()
            .await
            .unwrap();
    }

    server
        .client
        .post(server.url("/commit"))
        .json(&json!({ "bucket_id": 1, "data_roots": [h1_hex, h2_hex], "nonce": 0u64 }))
        .send()
        .await
        .unwrap();

    // Delete first leaf
    let resp = server
        .client
        .post(server.url("/delete"))
        .json(&json!({
            "bucket_id": 1,
            "new_start_seq": 1,
            "admin_signature": "0x00",
            "nonce": 0u64,
        }))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);

    let body: Value = resp.json().await.unwrap();
    assert_eq!(body["start_seq"], 1);
    assert_eq!(body["leaf_count"], 1);
}

#[tokio::test]
async fn disk_stats_endpoint() {
    let server = DiskTestServer::new().await;
    upload_and_commit(&server, 1).await;

    let resp = server
        .client
        .get(server.url("/stats"))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);

    let body: Value = resp.json().await.unwrap();
    assert_eq!(body["total_buckets"], 1);
    assert!(body["total_nodes"].as_u64().unwrap() > 0);
    assert!(body["total_bytes"].as_u64().unwrap() > 0);
}

#[tokio::test]
async fn disk_list_buckets() {
    let server = DiskTestServer::new().await;
    upload_and_commit(&server, 1).await;
    upload_and_commit(&server, 2).await;

    let resp = server
        .client
        .get(server.url("/buckets"))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);

    let body: Value = resp.json().await.unwrap();
    let buckets = body["buckets"].as_array().unwrap();
    assert_eq!(buckets.len(), 2);
}

#[tokio::test]
async fn disk_upload_invalid_hash() {
    let server = DiskTestServer::new().await;

    let wrong_hash = "0x0000000000000000000000000000000000000000000000000000000000000001";
    let resp = server
        .client
        .put(server.url("/node"))
        .json(&json!({
            "bucket_id": 1,
            "hash": wrong_hash,
            "data": BASE64.encode(b"data"),
            "children": null,
        }))
        .send()
        .await
        .unwrap();

    assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
    let body: Value = resp.json().await.unwrap();
    assert_eq!(body["error"], "invalid_hash");
}

#[tokio::test]
async fn disk_s3_put_get_flow() {
    let server = DiskTestServer::new().await;

    // S3 PUT
    let resp = server
        .client
        .put(server.url("/s3/1/object?key=disk-test.txt"))
        .body(b"disk s3 content".to_vec())
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);

    // S3 GET
    let resp = server
        .client
        .get(server.url("/s3/1/object?key=disk-test.txt"))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    assert_eq!(resp.bytes().await.unwrap().as_ref(), b"disk s3 content");
}

#[tokio::test]
async fn disk_fs_put_get_flow() {
    let server = DiskTestServer::new().await;

    // FS PUT
    let resp = server
        .client
        .put(server.url("/fs/1/file?path=/disk-test.txt"))
        .body(b"disk fs content".to_vec())
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);

    // FS GET
    let resp = server
        .client
        .get(server.url("/fs/1/file?path=/disk-test.txt"))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    assert_eq!(resp.bytes().await.unwrap().as_ref(), b"disk fs content");
}

// ─────────────────────────────────────────────────────────────────────────────
// Helpers
// ─────────────────────────────────────────────────────────────────────────────

fn hex_encode(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}
