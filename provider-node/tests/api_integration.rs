//! Integration tests for the provider node HTTP API.
//!
//! These tests spin up a real HTTP server and test the full request/response cycle.

use axum::http::StatusCode;
use base64::{engine::general_purpose::STANDARD as BASE64, Engine};
use reqwest::Client;
use serde_json::{json, Value};
use std::net::SocketAddr;
use std::sync::Arc;
use storage_provider_node::{create_router, ProviderState, Storage};
use tokio::net::TcpListener;

/// Test server helper that starts the provider node on a random port.
struct TestServer {
    addr: SocketAddr,
    client: Client,
}

impl TestServer {
    async fn new() -> Self {
        let storage = Arc::new(Storage::new());
        let state = Arc::new(ProviderState::new(storage, "0xtest_provider".to_string()));

        let app = create_router(state);

        // Bind to port 0 to get a random available port
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();

        // Spawn the server
        tokio::spawn(async move {
            axum::serve(listener, app).await.unwrap();
        });

        // Give the server a moment to start
        tokio::time::sleep(tokio::time::Duration::from_millis(10)).await;

        Self {
            addr,
            client: Client::new(),
        }
    }

    fn url(&self, path: &str) -> String {
        format!("http://{}{}", self.addr, path)
    }
}

#[tokio::test]
async fn test_health_endpoint() {
    let server = TestServer::new().await;

    let response = server
        .client
        .get(server.url("/health"))
        .send()
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::OK);

    let body: Value = response.json().await.unwrap();
    assert_eq!(body["status"], "healthy");
    assert!(body["version"].is_string());
}

#[tokio::test]
async fn test_info_endpoint() {
    let server = TestServer::new().await;

    let response = server.client.get(server.url("/info")).send().await.unwrap();

    assert_eq!(response.status(), StatusCode::OK);

    let body: Value = response.json().await.unwrap();
    assert_eq!(body["status"], "healthy");
}

#[tokio::test]
async fn test_upload_and_download_node() {
    let server = TestServer::new().await;

    // Create test data
    let data = b"Hello, World!";
    let hash = storage_primitives::blake2_256(data);
    let hash_hex = format!("0x{}", hex_encode(hash.as_bytes()));

    // Upload node
    let upload_response = server
        .client
        .put(server.url("/node"))
        .json(&json!({
            "bucket_id": 1,
            "hash": hash_hex,
            "data": BASE64.encode(data),
            "children": null
        }))
        .send()
        .await
        .unwrap();

    assert_eq!(upload_response.status(), StatusCode::OK);

    let body: Value = upload_response.json().await.unwrap();
    assert_eq!(body["stored"], true);

    // Download node
    let download_response = server
        .client
        .get(server.url(&format!("/node?hash={hash_hex}")))
        .send()
        .await
        .unwrap();

    assert_eq!(download_response.status(), StatusCode::OK);

    let body: Value = download_response.json().await.unwrap();
    assert_eq!(body["hash"], hash_hex);

    let downloaded_data = BASE64.decode(body["data"].as_str().unwrap()).unwrap();
    assert_eq!(downloaded_data, data);
}

#[tokio::test]
async fn test_check_exists() {
    let server = TestServer::new().await;

    // Upload a node first
    let data = b"Test data for exists check";
    let hash = storage_primitives::blake2_256(data);
    let hash_hex = format!("0x{}", hex_encode(hash.as_bytes()));

    server
        .client
        .put(server.url("/node"))
        .json(&json!({
            "bucket_id": 1,
            "hash": hash_hex,
            "data": BASE64.encode(data),
            "children": null
        }))
        .send()
        .await
        .unwrap();

    // Check exists
    let non_existent_hash = "0x0000000000000000000000000000000000000000000000000000000000000001";

    let response = server
        .client
        .post(server.url("/exists"))
        .json(&json!({
            "bucket_id": 1,
            "hashes": [hash_hex, non_existent_hash]
        }))
        .send()
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::OK);

    let body: Value = response.json().await.unwrap();
    let exists = body["exists"].as_array().unwrap();
    let missing = body["missing"].as_array().unwrap();

    assert!(exists.iter().any(|h| h.as_str().unwrap() == hash_hex));
    assert!(missing
        .iter()
        .any(|h| h.as_str().unwrap() == non_existent_hash));
}

#[tokio::test]
async fn test_commit_and_get_commitment() {
    let server = TestServer::new().await;

    // Upload a data root (leaf chunk)
    let data = b"Chunk data for commit test";
    let hash = storage_primitives::blake2_256(data);
    let hash_hex = format!("0x{}", hex_encode(hash.as_bytes()));

    server
        .client
        .put(server.url("/node"))
        .json(&json!({
            "bucket_id": 1,
            "hash": hash_hex,
            "data": BASE64.encode(data),
            "children": null
        }))
        .send()
        .await
        .unwrap();

    // Commit
    let commit_response = server
        .client
        .post(server.url("/commit"))
        .json(&json!({
            "bucket_id": 1,
            "data_roots": [hash_hex]
        }))
        .send()
        .await
        .unwrap();

    assert_eq!(commit_response.status(), StatusCode::OK);

    let body: Value = commit_response.json().await.unwrap();
    assert!(body["mmr_root"].is_string());
    assert_eq!(body["start_seq"], 0);
    assert_eq!(body["leaf_indices"], json!([0]));
    assert!(body["provider_signature"].is_string());

    // Get commitment
    let commitment_response = server
        .client
        .get(server.url("/commitment?bucket_id=1"))
        .send()
        .await
        .unwrap();

    assert_eq!(commitment_response.status(), StatusCode::OK);

    let body: Value = commitment_response.json().await.unwrap();
    assert_eq!(body["bucket_id"], 1);
    assert_eq!(body["leaf_count"], 1);
}

#[tokio::test]
async fn test_list_buckets() {
    let server = TestServer::new().await;

    // Upload to bucket 1 to create it
    let data = b"Data for bucket 1";
    let hash = storage_primitives::blake2_256(data);
    let hash_hex = format!("0x{}", hex_encode(hash.as_bytes()));

    server
        .client
        .put(server.url("/node"))
        .json(&json!({
            "bucket_id": 1,
            "hash": hash_hex,
            "data": BASE64.encode(data),
            "children": null
        }))
        .send()
        .await
        .unwrap();

    // List buckets
    let response = server
        .client
        .get(server.url("/buckets"))
        .send()
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::OK);

    let body: Value = response.json().await.unwrap();
    assert!(body["buckets"].is_array());
}

#[tokio::test]
async fn test_upload_with_invalid_hash_fails() {
    let server = TestServer::new().await;

    let data = b"Some data";
    let wrong_hash = "0x0000000000000000000000000000000000000000000000000000000000000001";

    let response = server
        .client
        .put(server.url("/node"))
        .json(&json!({
            "bucket_id": 1,
            "hash": wrong_hash,
            "data": BASE64.encode(data),
            "children": null
        }))
        .send()
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::BAD_REQUEST);

    let body: Value = response.json().await.unwrap();
    assert_eq!(body["error"], "invalid_hash");
}

#[tokio::test]
async fn test_upload_internal_node_with_missing_children_fails() {
    let server = TestServer::new().await;

    let child1 = "0x0000000000000000000000000000000000000000000000000000000000000001";
    let child2 = "0x0000000000000000000000000000000000000000000000000000000000000002";

    // Create internal node data (child hashes concatenated)
    let mut node_data = Vec::new();
    node_data.extend_from_slice(&hex_decode(child1).unwrap());
    node_data.extend_from_slice(&hex_decode(child2).unwrap());

    let hash = storage_primitives::blake2_256(&node_data);
    let hash_hex = format!("0x{}", hex_encode(hash.as_bytes()));

    let response = server
        .client
        .put(server.url("/node"))
        .json(&json!({
            "bucket_id": 1,
            "hash": hash_hex,
            "data": BASE64.encode(&node_data),
            "children": [child1, child2]
        }))
        .send()
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::BAD_REQUEST);

    let body: Value = response.json().await.unwrap();
    assert_eq!(body["error"], "children_missing");
}

#[tokio::test]
async fn test_full_upload_commit_read_flow() {
    let server = TestServer::new().await;

    // Step 1: Upload multiple chunks
    let chunks: Vec<&[u8]> = vec![b"Chunk 1 data", b"Chunk 2 data", b"Chunk 3 data"];
    let mut chunk_hashes = Vec::new();

    for chunk in &chunks {
        let hash = storage_primitives::blake2_256(chunk);
        let hash_hex = format!("0x{}", hex_encode(hash.as_bytes()));
        chunk_hashes.push(hash_hex.clone());

        let response = server
            .client
            .put(server.url("/node"))
            .json(&json!({
                "bucket_id": 1,
                "hash": hash_hex,
                "data": BASE64.encode(chunk),
                "children": null
            }))
            .send()
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::OK);
    }

    // Step 2: Build a simple internal node (just use first chunk as root for simplicity)
    let data_root = &chunk_hashes[0];

    // Step 3: Commit
    let commit_response = server
        .client
        .post(server.url("/commit"))
        .json(&json!({
            "bucket_id": 1,
            "data_roots": [data_root]
        }))
        .send()
        .await
        .unwrap();

    assert_eq!(commit_response.status(), StatusCode::OK);

    let commit_body: Value = commit_response.json().await.unwrap();
    assert!(commit_body["mmr_root"].is_string());

    // Step 4: Read back
    let read_response = server
        .client
        .get(server.url(&format!("/read?data_root={data_root}&offset=0&length=100")))
        .send()
        .await
        .unwrap();

    assert_eq!(read_response.status(), StatusCode::OK);

    let read_body: Value = read_response.json().await.unwrap();
    assert!(read_body["chunks"].is_array());
}

// Helper functions

fn hex_encode(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

fn hex_decode(s: &str) -> Result<Vec<u8>, &'static str> {
    let s = s.strip_prefix("0x").unwrap_or(s);
    if !s.len().is_multiple_of(2) {
        return Err("invalid hex length");
    }

    (0..s.len())
        .step_by(2)
        .map(|i| u8::from_str_radix(&s[i..i + 2], 16).map_err(|_| "invalid hex"))
        .collect()
}
