//! Integration tests for the provider node HTTP API.
//!
//! These tests spin up a real HTTP server and test the full request/response cycle.

use axum::http::StatusCode;
use base64::{engine::general_purpose::STANDARD as BASE64, Engine};
use codec::Encode;
use reqwest::Client;
use serde_json::{json, Value};
use sp_core::{sr25519, ByteArray, Pair, H256};
use std::net::SocketAddr;
use std::sync::Arc;
use storage_primitives::CommitmentPayload;
use storage_provider_node::{create_router, ProviderState, Storage};
use tokio::net::TcpListener;

/// Test server helper that starts the provider node on a random port.
struct TestServer {
    addr: SocketAddr,
    client: Client,
}

impl TestServer {
    /// Spin up a server with `//Alice` as the signing key.
    ///
    /// Endpoints that sign commitments (`/commit`, `/commitment`, ...) work
    /// because a real sr25519 keypair is available.
    async fn new() -> Self {
        Self::with_state(Arc::new(
            ProviderState::with_seed(Arc::new(Storage::new()), "//Alice")
                .expect("//Alice is a valid SURI"),
        ))
        .await
    }

    /// Spin up a server with no signing key.
    ///
    /// Used to verify that signing-bound endpoints return 503 rather than
    /// silently emitting zero-byte placeholder signatures.
    async fn new_unsigned() -> Self {
        Self::with_state(Arc::new(ProviderState::new(
            Arc::new(Storage::new()),
            "0xtest_provider".to_string(),
        )))
        .await
    }

    async fn with_state(state: Arc<ProviderState>) -> Self {
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
    assert_eq!(body["provider_id"], "0xtest_provider");
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

// ─────────────────────────────────────────────────────────────────────────────
// Signing security: commit/commitment endpoints must never emit a placeholder
// 64-zero-byte "signature" when the provider has no keypair configured.
// They must refuse with 503 Service Unavailable instead.
// ─────────────────────────────────────────────────────────────────────────────

/// Upload a single chunk and commit it. Returns the chunk hash hex and
/// the parsed commit-response body.
async fn upload_and_commit(server: &TestServer, bucket_id: u64) -> (String, Value) {
    let data = b"chunk-for-signature-tests";
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
        }))
        .send()
        .await
        .unwrap();

    let status = commit_resp.status();
    let body: Value = commit_resp.json().await.unwrap();
    assert_eq!(status, StatusCode::OK, "commit failed: {body:?}");
    (hash_hex, body)
}

/// `//Alice`'s sr25519 public key, used to verify signatures produced by the
/// server. Derived once via `sr25519::Pair::from_string("//Alice", None)`.
fn alice_public() -> sr25519::Public {
    sr25519::Pair::from_string("//Alice", None)
        .expect("//Alice is a valid SURI")
        .public()
}

#[tokio::test]
async fn commit_returns_valid_sr25519_signature_over_commitment_payload() {
    // The whole point of the zero-byte-signing fix: when a key IS configured,
    // the server must produce a real sr25519 signature that verifies under the
    // provider's public key against the exact bytes the pallet will hash.
    let server = TestServer::new().await;
    let bucket_id = 7;

    let (_chunk_hash, body) = upload_and_commit(&server, bucket_id).await;

    let mmr_root_hex = body["mmr_root"].as_str().expect("mmr_root present");
    let start_seq = body["start_seq"].as_u64().expect("start_seq present");
    let sig_hex = body["provider_signature"]
        .as_str()
        .expect("provider_signature present");

    // Defensive: the zero-byte placeholder this PR removes was exactly 64
    // hex-encoded zero bytes. If it ever returns, this catches it.
    let sig_bytes = hex_decode(sig_hex).expect("signature hex parses");
    assert_eq!(sig_bytes.len(), 64, "sr25519 signatures are 64 bytes");
    assert_ne!(
        sig_bytes,
        vec![0u8; 64],
        "server returned zero-byte placeholder instead of a real signature"
    );

    // Reconstruct exactly what the handler signed: CommitmentPayload with
    // leaf_count = 0 (matches /commit's encoding).
    let mmr_root_bytes = hex_decode(mmr_root_hex).unwrap();
    let mmr_root = H256::from_slice(&mmr_root_bytes);
    let payload = CommitmentPayload::new(bucket_id, mmr_root, start_seq, 0);
    let encoded = payload.encode();

    let sig = sr25519::Signature::from_slice(&sig_bytes).expect("64-byte signature");
    assert!(
        sr25519::Pair::verify(&sig, &encoded, &alice_public()),
        "signature did not verify against //Alice over the expected commitment payload"
    );
}

#[tokio::test]
async fn checkpoint_signature_verifies_with_real_leaf_count() {
    // `/checkpoint-signature` signs the payload with the *real* leaf_count
    // (unlike /commit which uses 0). Verify both that a real signature comes
    // back and that it round-trips against the public key.
    let server = TestServer::new().await;
    let bucket_id = 8;

    upload_and_commit(&server, bucket_id).await;

    let resp = server
        .client
        .get(server.url(&format!("/checkpoint-signature?bucket_id={bucket_id}")))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let body: Value = resp.json().await.unwrap();

    let mmr_root = H256::from_slice(&hex_decode(body["mmr_root"].as_str().unwrap()).unwrap());
    let start_seq = body["start_seq"].as_u64().unwrap();
    let leaf_count = body["leaf_count"].as_u64().unwrap();
    assert!(leaf_count > 0, "leaf_count must be the real on-disk value");

    let sig_bytes = hex_decode(body["provider_signature"].as_str().unwrap()).unwrap();
    assert_ne!(sig_bytes, vec![0u8; 64]);

    let payload = CommitmentPayload::new(bucket_id, mmr_root, start_seq, leaf_count);
    let sig = sr25519::Signature::from_slice(&sig_bytes).unwrap();
    assert!(sr25519::Pair::verify(
        &sig,
        payload.encode(),
        &alice_public()
    ));
}

#[tokio::test]
async fn commitment_signature_does_not_verify_under_a_different_key() {
    // Negative control: //Bob must not be able to take credit for //Alice's
    // signature. Catches accidental no-op verifiers.
    let server = TestServer::new().await;
    let bucket_id = 9;

    let (_h, body) = upload_and_commit(&server, bucket_id).await;
    let mmr_root = H256::from_slice(&hex_decode(body["mmr_root"].as_str().unwrap()).unwrap());
    let start_seq = body["start_seq"].as_u64().unwrap();
    let sig = sr25519::Signature::from_slice(
        &hex_decode(body["provider_signature"].as_str().unwrap()).unwrap(),
    )
    .unwrap();
    let encoded = CommitmentPayload::new(bucket_id, mmr_root, start_seq, 0).encode();

    let bob = sr25519::Pair::from_string("//Bob", None).unwrap().public();
    assert!(
        !sr25519::Pair::verify(&sig, &encoded, &bob),
        "Alice's signature wrongly verifies under Bob's key"
    );
}

#[tokio::test]
async fn commit_returns_503_when_no_signing_key_configured() {
    // The pre-fix behaviour silently returned 64 zero bytes here. Post-fix,
    // the handler must refuse with 503 Service Unavailable so callers do not
    // mistake a placeholder for a real provider commitment.
    let server = TestServer::new_unsigned().await;

    let data = b"chunk-without-key";
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

    let resp = server
        .client
        .post(server.url("/commit"))
        .json(&json!({ "bucket_id": 1, "data_roots": [hash_hex] }))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::SERVICE_UNAVAILABLE);
    let body: Value = resp.json().await.unwrap();
    assert_eq!(body["error"], "signing_unavailable");
}

#[tokio::test]
async fn commitment_endpoint_returns_503_when_no_signing_key() {
    // GET /commitment also signs the response — it must also refuse rather
    // than emit zero bytes.
    let server = TestServer::new_unsigned().await;

    // Seed the bucket via storage so the bucket-lookup gate passes and the
    // handler actually reaches the signing step. Calling /commit on the
    // unsigned server returns 503 but still mutates storage, which is enough
    // to make /commitment reach state.sign(...).
    let data = b"chunk-for-commitment-503";
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
    server
        .client
        .post(server.url("/commit"))
        .json(&json!({ "bucket_id": 1, "data_roots": [hash_hex] }))
        .send()
        .await
        .unwrap();

    let resp = server
        .client
        .get(server.url("/commitment?bucket_id=1"))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::SERVICE_UNAVAILABLE);
    let body: Value = resp.json().await.unwrap();
    assert_eq!(body["error"], "signing_unavailable");
}

#[tokio::test]
async fn checkpoint_sign_endpoint_returns_503_when_no_signing_key() {
    // POST /checkpoint/sign is the second of the two duplicated zero-byte
    // sites the audit flagged (provider-node/src/api.rs:604-608). It must
    // also refuse with 503 — and crucially, never emit a 0x00..00 signature.
    let unsigned = TestServer::new_unsigned().await;
    let keyed = TestServer::new().await;

    // Seed identical state on both servers so the unsigned server's local
    // MMR matches the proposal and the handler reaches the signing step
    // (it short-circuits with `agreed: false` if the local root differs).
    let data = b"chunk-for-checkpoint-sign";
    let hash = storage_primitives::blake2_256(data);
    let hash_hex = format!("0x{}", hex_encode(hash.as_bytes()));
    for s in [&unsigned, &keyed] {
        s.client
            .put(s.url("/node"))
            .json(&json!({
                "bucket_id": 1,
                "hash": hash_hex,
                "data": BASE64.encode(data),
                "children": null,
            }))
            .send()
            .await
            .unwrap();
        s.client
            .post(s.url("/commit"))
            .json(&json!({ "bucket_id": 1, "data_roots": [hash_hex] }))
            .send()
            .await
            .unwrap();
    }
    // Take the proposal parameters from the keyed server (its /commit
    // succeeded). The unsigned server's storage mutated on its own 503'd
    // /commit, so both servers' MMR roots are identical.
    let proposal = keyed
        .client
        .get(keyed.url("/commitment?bucket_id=1"))
        .send()
        .await
        .unwrap()
        .json::<Value>()
        .await
        .unwrap();

    let resp = unsigned
        .client
        .post(unsigned.url("/checkpoint/sign"))
        .json(&json!({
            "bucket_id": 1,
            "mmr_root": proposal["mmr_root"],
            "start_seq": proposal["start_seq"],
            "leaf_count": proposal["leaf_count"],
            "window": 0,
        }))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::SERVICE_UNAVAILABLE);
    let body: Value = resp.json().await.unwrap();
    assert_eq!(body["error"], "signing_unavailable");
}

#[tokio::test]
async fn delete_endpoint_returns_503_when_no_signing_key() {
    let server = TestServer::new_unsigned().await;

    // Seed and commit so the delete handler can reach its sign() call.
    let data = b"chunk-for-delete-503";
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
    server
        .client
        .post(server.url("/commit"))
        .json(&json!({ "bucket_id": 1, "data_roots": [hash_hex] }))
        .send()
        .await
        .unwrap();

    let resp = server
        .client
        .post(server.url("/delete"))
        .json(&json!({
            "bucket_id": 1,
            "new_start_seq": 1,
            "admin_signature": "0x00",
        }))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::SERVICE_UNAVAILABLE);
    let body: Value = resp.json().await.unwrap();
    assert_eq!(body["error"], "signing_unavailable");
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
