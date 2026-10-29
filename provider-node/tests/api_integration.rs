// SPDX-License-Identifier: GPL-3.0-only

//! Integration tests for the provider node HTTP API.
//!
//! These tests spin up a real HTTP server and test the full request/response cycle.

mod common;

use axum::http::StatusCode;
use base64::{engine::general_purpose::STANDARD as BASE64, Engine};
use codec::Encode;
use provider_types::KeyScheme;
use reqwest::Method;
use serde_json::{json, Value};
use sp_core::crypto::Ss58Codec;
use sp_core::{sr25519, Pair, H256};
use storage_primitives::{Commitment, CommitmentPayload};
use storage_provider_node::{PageLimits, ProviderState};

use common::{StorageBackendKind, TestServer};

impl TestServer {
    /// Signing provider (`//Alice`): endpoints that sign commitments work.
    async fn new(backend: StorageBackendKind) -> Self {
        Self::new_with_scheme(backend, KeyScheme::Sr25519).await
    }

    /// Signing provider (`//Alice`) under the given scheme.
    async fn new_with_scheme(backend: StorageBackendKind, scheme: KeyScheme) -> Self {
        Self::start(backend, move |deps| {
            ProviderState::with_seed_scheme(deps, common::PROVIDER_SEED, scheme)
                .expect("//Alice is a valid SURI")
        })
        .await
    }

    /// Signing provider (`//Alice`) serving pages within `limits`.
    async fn new_with_page_limits(backend: StorageBackendKind, limits: PageLimits) -> Self {
        Self::start(backend, move |deps| {
            ProviderState::with_seed(deps, common::PROVIDER_SEED)
                .expect("//Alice is a valid SURI")
                .with_page_limits(limits)
        })
        .await
    }

    /// No signing key, so signing-bound endpoints must answer 503 rather than
    /// emit zero-byte placeholder signatures.
    async fn new_unsigned(backend: StorageBackendKind) -> Self {
        Self::start(backend, |deps| {
            ProviderState::with_provider_id(deps, "0xtest_provider".to_string())
        })
        .await
    }

    /// Signing provider (`//Alice`) with no on-chain registration published,
    /// so signing-bound endpoints must answer 503 rather than sign with a key
    /// the chain does not (yet) know about.
    async fn new_unregistered(backend: StorageBackendKind) -> Self {
        Self::start_unregistered(backend, |deps| {
            ProviderState::with_seed(deps, common::PROVIDER_SEED).expect("//Alice is a valid SURI")
        })
        .await
    }
}

common::backend_tests! {
    async fn test_health_endpoint(backend) {
        let server = TestServer::new(backend).await;

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
}

common::backend_tests! {
    async fn test_info_endpoint(backend) {
        let server = TestServer::new(backend).await;

        let response = server.client.get(server.url("/info")).send().await.unwrap();

        assert_eq!(response.status(), StatusCode::OK);

        let expect_provider_id = sr25519::Pair::from_string(common::PROVIDER_SEED, None)
            .expect("Invalid provider seed")
            .public()
            .to_ss58check();

        let body: Value = response.json().await.unwrap();
        assert_eq!(body["provider_id"], expect_provider_id);

        // `TestServer::new` seeds a signing key and a matching on-chain
        // registration, so readiness must reflect "can sign and negotiate".
        assert_eq!(body["readiness"]["signing_configured"], true);
        assert_eq!(body["readiness"]["provider_info_loaded"], true);
    }
}

common::backend_tests! {
    async fn test_upload_and_download_node(backend) {
        let server = TestServer::new(backend).await;

        // Create test data
        let data = b"Hello, World!";
        let hash = storage_primitives::hash_leaf(data);
        let hash_hex = format!("{hash:#x}");

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
            .get(server.url(&format!("/node?bucket_id=1&hash={hash_hex}")))
            .send()
            .await
            .unwrap();

        assert_eq!(download_response.status(), StatusCode::OK);

        let body: Value = download_response.json().await.unwrap();
        assert_eq!(body["hash"], hash_hex);

        let downloaded_data = BASE64.decode(body["data"].as_str().unwrap()).unwrap();
        assert_eq!(downloaded_data, data);

        // A bare hex request gets the `0x`-prefixed hash back.
        let body: Value = server
            .client
            .get(server.url(&format!("/node?bucket_id=1&hash={}", &hash_hex[2..])))
            .send()
            .await
            .unwrap()
            .json()
            .await
            .unwrap();
        assert_eq!(body["hash"], hash_hex);
    }
}

common::backend_tests! {
    async fn test_check_exists(backend) {
        let server = TestServer::new(backend).await;

        // Upload a node first
        let data = b"Test data for exists check";
        let hash = storage_primitives::hash_leaf(data);
        let hash_hex = format!("{hash:#x}");

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
}

common::backend_tests! {
    async fn test_commit_and_get_commitment(backend) {
        let server = TestServer::new(backend).await;

        // Upload a data root (leaf chunk)
        let data = b"Chunk data for commit test";
        let hash = storage_primitives::hash_leaf(data);
        let hash_hex = format!("{hash:#x}");

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
            "data_roots": [hash_hex],
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
}

common::backend_tests! {
    async fn test_upload_with_invalid_hash_fails(backend) {
        let server = TestServer::new(backend).await;

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
}

// Chunks and internal nodes are hashed with different prefixes, so a hash
// computed without the prefix for the node's kind is never accepted.
common::backend_tests! {
    async fn test_upload_rejects_untagged_chunk_and_node_hashes(backend) {
        let server = TestServer::new(backend).await;

        let data = b"untagged chunk";
        let response = server
            .client
            .put(server.url("/node"))
            .json(&json!({
                "bucket_id": 1,
                "hash": format!("{:#x}", storage_primitives::blake2_256(data)),
                "data": BASE64.encode(data),
                "children": null
            }))
            .send()
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::BAD_REQUEST);
        let body: Value = response.json().await.unwrap();
        assert_eq!(body["error"], "invalid_hash");

        let left = storage_primitives::hash_leaf(b"left");
        let right = storage_primitives::hash_leaf(b"right");
        let response = server
            .client
            .put(server.url("/node"))
            .json(&json!({
                "bucket_id": 1,
                "hash": format!("{:#x}", H256::zero()),
                "children": [
                    format!("{left:#x}"),
                    format!("{right:#x}"),
                ]
            }))
            .send()
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::BAD_REQUEST);
        let body: Value = response.json().await.unwrap();
        assert_eq!(body["error"], "invalid_hash");
    }
}

common::backend_tests! {
    async fn test_upload_internal_node_with_missing_children_fails(backend) {
        let server = TestServer::new(backend).await;

        let child1 = "0x0000000000000000000000000000000000000000000000000000000000000001";
        let child2 = "0x0000000000000000000000000000000000000000000000000000000000000002";

        let hash = storage_primitives::hash_children(
            child1.parse::<sp_core::H256>().unwrap(),
            child2.parse::<sp_core::H256>().unwrap(),
        );
        let hash_hex = format!("{hash:#x}");

        let response = server
            .client
            .put(server.url("/node"))
            .json(&json!({
                "bucket_id": 1,
                "hash": hash_hex,
                "children": [child1, child2]
            }))
            .send()
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::BAD_REQUEST);

        let body: Value = response.json().await.unwrap();
        assert_eq!(body["error"], "children_missing");
    }
}

common::backend_tests! {
    async fn test_upload_internal_node_rejects_children_that_are_not_a_pair(backend) {
        let server = TestServer::new(backend).await;

        // The child count is checked before anything is looked up, so the
        // child does not need to exist.
        let child_hash = H256::repeat_byte(0x11);
        let child_hex = format!("{child_hash:#x}");

        let hash = storage_primitives::hash_children(child_hash, child_hash);
        let response = server
            .client
            .put(server.url("/node"))
            .json(&json!({
                "bucket_id": 1,
                "hash": format!("{hash:#x}"),
                "children": [child_hex],
            }))
            .send()
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::BAD_REQUEST);

        let body: Value = response.json().await.unwrap();
        assert_eq!(body["error"], "invalid_child_count");
        assert_eq!(body["details"]["expected"], 2);
        assert_eq!(body["details"]["actual"], 1);
    }
}

common::backend_tests! {
    async fn test_upload_node_rejects_leaf_above_max_chunk_size(backend) {
        let server = TestServer::new(backend).await;
        let max = storage_primitives::DEFAULT_CHUNK_SIZE as usize;

        let put = |data: Vec<u8>| {
            let hash = storage_primitives::hash_leaf(&data);
            let request = server.client.put(server.url("/node")).json(&json!({
                "bucket_id": 1,
                "hash": format!("{hash:#x}"),
                "data": BASE64.encode(&data),
                "children": null,
            }));
            async move { request.send().await.unwrap() }
        };

        let response = put(vec![7u8; max]).await;
        assert_eq!(response.status(), StatusCode::OK);

        let response = put(vec![7u8; max + 1]).await;
        assert_eq!(response.status(), StatusCode::BAD_REQUEST);

        let body: Value = response.json().await.unwrap();
        assert_eq!(body["error"], "chunk_too_large");
        assert_eq!(body["details"]["size"], max + 1);
        assert_eq!(body["details"]["max"], max);
    }
}

common::backend_tests! {
    async fn test_upload_node_rejects_both_or_neither_of_data_and_children(backend) {
        let server = TestServer::new(backend).await;

        let (left, right) = (H256::from_low_u64_be(1), H256::from_low_u64_be(2));
        let hash = storage_primitives::hash_children(left, right);
        for request in [
            json!({
                "bucket_id": 1,
                "hash": format!("{hash:#x}"),
                "data": BASE64.encode(b"unrelated bytes"),
                "children": [format!("{left:#x}"), format!("{right:#x}")],
            }),
            json!({ "bucket_id": 1, "hash": format!("{hash:#x}") }),
        ] {
            let response = server
                .client
                .put(server.url("/node"))
                .json(&request)
                .send()
                .await
                .unwrap();
            assert_eq!(response.status(), StatusCode::BAD_REQUEST);
            let body: Value = response.json().await.unwrap();
            assert_eq!(body["error"], "invalid_node");
        }
    }
}

// An internal node is its two children: both download paths return them and
// no `data`.
common::backend_tests! {
    async fn test_download_internal_node_returns_its_children(backend) {
        let server = TestServer::new(backend).await;

        let mut child_hexes = Vec::new();
        for chunk in [b"left-chunk".as_slice(), b"right-chunk".as_slice()] {
            let chunk_hash = storage_primitives::hash_leaf(chunk);
            child_hexes.push(format!("{chunk_hash:#x}"));
            server
                .client
                .put(server.url("/node"))
                .json(&json!({
                    "bucket_id": 1,
                    "hash": format!("{chunk_hash:#x}"),
                    "data": BASE64.encode(chunk),
                }))
                .send()
                .await
                .unwrap();
        }

        let hash = storage_primitives::hash_children(
            storage_primitives::hash_leaf(b"left-chunk"),
            storage_primitives::hash_leaf(b"right-chunk"),
        );
        let hash_hex = format!("{hash:#x}");

        let response = server
            .client
            .put(server.url("/node"))
            .json(&json!({
                "bucket_id": 1,
                "hash": hash_hex,
                "children": child_hexes,
            }))
            .send()
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);

        let response = server
            .client
            .get(server.url(&format!("/node?bucket_id=1&hash={hash_hex}")))
            .send()
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        let body: Value = response.json().await.unwrap();
        assert_eq!(body["children"], json!(child_hexes));
        assert!(body.get("data").is_none());

        let response = server
            .client
            .post(server.url("/fetch_nodes"))
            .json(&json!({
                "bucket_id": 1,
                "hashes": [hash_hex],
            }))
            .send()
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        let body: Value = response.json().await.unwrap();
        let node = &body["nodes"].as_array().unwrap()[0];
        assert_eq!(node["children"], json!(child_hexes));
        assert!(node.get("data").is_none());
    }
}

common::backend_tests! {
    async fn test_full_upload_commit_read_flow(backend) {
        let server = TestServer::new(backend).await;

        // Step 1: Upload multiple chunks
        let chunks: Vec<&[u8]> = vec![b"Chunk 1 data", b"Chunk 2 data", b"Chunk 3 data"];
        let mut chunk_hashes = Vec::new();

        for chunk in &chunks {
            let hash = storage_primitives::hash_leaf(chunk);
            let hash_hex = format!("{hash:#x}");
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
            "data_roots": [data_root],
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
            .get(server.url(&format!("/read?bucket_id=1&data_root={data_root}&offset=0&length=100")))
            .send()
            .await
            .unwrap();

        assert_eq!(read_response.status(), StatusCode::OK);

        let read_body: Value = read_response.json().await.unwrap();
        assert!(read_body["chunks"].is_array());
    }
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
    let hash = storage_primitives::hash_leaf(data);
    let hash_hex = format!("{hash:#x}");

    let resp = server
        .client
        .request_bucket(Method::PUT, server.url("/node"), bucket_id)
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
        .request_bucket(Method::POST, server.url("/commit"), bucket_id)
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

/// `//Alice`'s sr25519 public key, used to verify signatures from the server.
fn alice_public() -> sr25519::Public {
    common::test_member_pair().public()
}

/// Decode a `provider_signature` field: 0x-prefixed hex of a SCALE-encoded
/// `MultiSignature`, as emitted by `ProviderState::sign`.
fn decode_provider_signature(sig_hex: &str) -> sp_runtime::MultiSignature {
    use codec::Decode;
    let bytes = array_bytes::hex2bytes(sig_hex).expect("signature hex parses");
    sp_runtime::MultiSignature::decode(&mut &bytes[..]).expect("valid SCALE MultiSignature")
}

/// Expect the signature to be the Sr25519 variant and unwrap it.
fn expect_sr25519(sig: sp_runtime::MultiSignature) -> sr25519::Signature {
    match sig {
        sp_runtime::MultiSignature::Sr25519(sig) => sig,
        other => panic!("expected an Sr25519 signature, got {other:?}"),
    }
}

common::backend_tests! {
    async fn commit_returns_valid_sr25519_signature_over_commitment_payload(backend) {
        // The whole point of the zero-byte-signing fix: when a key IS configured,
        // the server must produce a real sr25519 signature that verifies under the
        // provider's public key against the exact bytes the pallet will hash.
        let server = TestServer::new(backend).await;
        let bucket_id = 7;

        let (_chunk_hash, body) = upload_and_commit(&server, bucket_id).await;

        let mmr_root_hex = body["mmr_root"].as_str().expect("mmr_root present");
        let start_seq = body["start_seq"].as_u64().expect("start_seq present");
        let sig_hex = body["provider_signature"]
            .as_str()
            .expect("provider_signature present");

        // Defensive: if the zero-byte placeholder ever returns, this catches it.
        let sig = expect_sr25519(decode_provider_signature(sig_hex));
        assert_ne!(
            sig.0, [0u8; 64],
            "server returned zero-byte placeholder instead of a real signature"
        );

        // Reconstruct exactly what the handler signed: CommitmentPayload with
        // the real post-commit leaf_count (no longer hardcoded 0).
        let mmr_root: H256 = mmr_root_hex.parse().unwrap();
        let leaf_count = body["leaf_count"]
            .as_u64()
            .expect("leaf_count present in /commit response");
        let payload = CommitmentPayload::new(
            bucket_id,
            Commitment {
                mmr_root,
                start_seq,
                leaf_count,
            },
        );
        let encoded = payload.encode();

        assert!(
            sr25519::Pair::verify(&sig, &encoded, &alice_public()),
            "signature did not verify against //Alice over the expected commitment payload"
        );
    }
}

common::backend_tests! {
    async fn checkpoint_signature_verifies_with_real_leaf_count(backend) {
        // `/checkpoint-signature` signs the payload with the *real* leaf_count
        // (unlike /commit which uses 0). Verify both that a real signature comes
        // back and that it round-trips against the public key.
        let server = TestServer::new(backend).await;
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

        let mmr_root = body["mmr_root"].as_str().unwrap().parse::<H256>().unwrap();
        let start_seq = body["start_seq"].as_u64().unwrap();
        let leaf_count = body["leaf_count"].as_u64().unwrap();
        assert!(leaf_count > 0, "leaf_count must be the real on-disk value");

        let sig = expect_sr25519(decode_provider_signature(
            body["provider_signature"].as_str().unwrap(),
        ));
        assert_ne!(sig.0, [0u8; 64]);

        let payload = CommitmentPayload::new(
            bucket_id,
            Commitment {
                mmr_root,
                start_seq,
                leaf_count,
            },
        );
        assert!(sr25519::Pair::verify(
            &sig,
            payload.encode(),
            &alice_public()
        ));
    }
}

common::backend_tests! {
    async fn commitment_signature_does_not_verify_under_a_different_key(backend) {
        // Negative control: //Bob must not be able to take credit for //Alice's
        // signature. Catches accidental no-op verifiers.
        let server = TestServer::new(backend).await;
        let bucket_id = 9;

        let (_h, body) = upload_and_commit(&server, bucket_id).await;
        let mmr_root = body["mmr_root"].as_str().unwrap().parse::<H256>().unwrap();
        let start_seq = body["start_seq"].as_u64().unwrap();
        let sig = expect_sr25519(decode_provider_signature(
            body["provider_signature"].as_str().unwrap(),
        ));
        let encoded = CommitmentPayload::new(
            bucket_id,
            Commitment {
                mmr_root,
                start_seq,
                leaf_count: 0,
            },
        )
        .encode();

        let bob = sr25519::Pair::from_string("//Bob", None).unwrap().public();
        assert!(
            !sr25519::Pair::verify(&sig, &encoded, &bob),
            "Alice's signature wrongly verifies under Bob's key"
        );
    }
}

#[tokio::test]
async fn commit_signs_with_configured_ed25519_scheme() {
    // A node started with --key-scheme ed25519 must emit Ed25519-tagged
    // MultiSignatures that verify under the ed25519 key derived from the
    // same seed — the full multi-scheme wire path.
    use sp_core::ed25519;

    let server = TestServer::new_with_scheme(StorageBackendKind::RocksDb, KeyScheme::Ed25519).await;
    let bucket_id = 11;

    let (_h, body) = upload_and_commit(&server, bucket_id).await;
    let mmr_root = body["mmr_root"].as_str().unwrap().parse::<H256>().unwrap();
    let start_seq = body["start_seq"].as_u64().unwrap();
    let leaf_count = body["leaf_count"].as_u64().unwrap();

    let sig = match decode_provider_signature(body["provider_signature"].as_str().unwrap()) {
        sp_runtime::MultiSignature::Ed25519(sig) => sig,
        other => panic!("expected an Ed25519 signature, got {other:?}"),
    };

    let payload = CommitmentPayload::new(
        bucket_id,
        Commitment {
            mmr_root,
            start_seq,
            leaf_count,
        },
    );
    let alice_ed = ed25519::Pair::from_string(common::PROVIDER_SEED, None)
        .unwrap()
        .public();
    assert!(
        ed25519::Pair::verify(&sig, payload.encode(), &alice_ed),
        "Ed25519 signature did not verify under the ed25519 //Alice key"
    );
}

common::backend_tests! {
    async fn commit_returns_503_when_no_signing_key_configured(backend) {
        // The pre-fix behaviour silently returned 64 zero bytes here. Post-fix,
        // the handler must refuse with 503 Service Unavailable so callers do not
        // mistake a placeholder for a real provider commitment.
        let server = TestServer::new_unsigned(backend).await;

        let data = b"chunk-without-key";
        let hash = storage_primitives::hash_leaf(data);
        let hash_hex = format!("{hash:#x}");

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
}

common::backend_tests! {
    async fn commitment_endpoint_returns_503_when_no_signing_key(backend) {
        // GET /commitment also signs the response — it must also refuse rather
        // than emit zero bytes.
        let server = TestServer::new_unsigned(backend).await;

        // Seed the bucket via storage so the bucket-lookup gate passes and the
        // handler actually reaches the signing step. Calling /commit on the
        // unsigned server returns 503 but still mutates storage, which is enough
        // to make /commitment reach state.sign(...).
        let data = b"chunk-for-commitment-503";
        let hash = storage_primitives::hash_leaf(data);
        let hash_hex = format!("{hash:#x}");
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
}

common::backend_tests! {
    async fn commitment_endpoint_returns_503_when_registration_not_published(backend) {
        // A signing key alone is not enough: without a published on-chain
        // registration, the pallet has no key to verify against, so signing
        // must refuse rather than emit a signature that can never verify.
        let server = TestServer::new_unregistered(backend).await;

        // Seed the bucket via storage so the bucket-lookup gate passes and the
        // handler actually reaches the signing step. Calling /commit reaches
        // sign() and returns 503, but still mutates storage, which is enough
        // to make /commitment reach state.sign(...) too.
        let data = b"chunk-for-commitment-unregistered";
        let hash = storage_primitives::hash_leaf(data);
        let hash_hex = format!("{hash:#x}");
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
        assert_eq!(body["error"], "provider_info_unavailable");
    }
}

common::backend_tests! {
    async fn delete_endpoint_returns_503_when_no_signing_key(backend) {
        let server = TestServer::new_unsigned(backend).await;

        // Seed and commit so the delete handler can reach its sign() call.
        let data = b"chunk-for-delete-503";
        let hash = storage_primitives::hash_leaf(data);
        let hash_hex = format!("{hash:#x}");
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
}

// ─────────────────────────────────────────────────────────────────────────────
// Stats endpoint
// ─────────────────────────────────────────────────────────────────────────────

common::backend_tests! {
    async fn test_stats_endpoint_empty(backend) {
        let server = TestServer::new(backend).await;

        let resp = server
            .client
            .get(server.url("/stats"))
            .send()
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);

        let body: Value = resp.json().await.unwrap();
        assert_eq!(body["total_buckets"], 0);
        assert_eq!(body["total_nodes"], 0);
        assert_eq!(body["total_bytes"], 0);
        assert!(body.get("buckets").is_none());
    }
}

common::backend_tests! {
    async fn test_stats_endpoint(backend) {
        let server = TestServer::new(backend).await;

        // Upload data so stats are non-zero
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
        assert!(body.get("buckets").is_none());
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Chunk proof endpoint
// ─────────────────────────────────────────────────────────────────────────────

common::backend_tests! {
    async fn test_chunk_proof_endpoint(backend) {
        let server = TestServer::new(backend).await;
        let (hash_hex, _body) = upload_and_commit(&server, 1).await;

        let resp = server
            .client
            .get(server.url(&format!("/chunk_proof?bucket_id=1&data_root={hash_hex}&chunk_index=0")))
            .send()
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);

        let body: Value = resp.json().await.unwrap();
        assert!(body["chunk_hash"].is_string());
        assert!(body["chunk_data"].is_string()); // base64
        assert!(body["proof"].is_array());
    }
}

common::backend_tests! {
    async fn test_chunk_proof_past_the_last_chunk_is_out_of_range(backend) {
        let server = TestServer::new(backend).await;
        let (hash_hex, _body) = upload_and_commit(&server, 1).await;

        let resp = server
            .client
            .get(server.url(&format!("/chunk_proof?bucket_id=1&data_root={hash_hex}&chunk_index=1")))
            .send()
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::BAD_REQUEST);

        let body: Value = resp.json().await.unwrap();
        assert_eq!(body["error"], "chunk_index_out_of_range");
        assert_eq!(body["details"]["chunk_index"], 1);
        assert_eq!(body["details"]["chunk_count"], 1);
    }
}

common::backend_tests! {
    async fn test_chunk_proof_invalid_root(backend) {
        let server = TestServer::new(backend).await;

        let fake_root = "0x0000000000000000000000000000000000000000000000000000000000000001";
        let resp = server
            .client
            .get(server.url(&format!("/chunk_proof?bucket_id=1&data_root={fake_root}&chunk_index=0")))
            .send()
            .await
            .unwrap();

        assert!(resp.status().is_client_error() || resp.status().is_server_error());
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// MMR peaks and subtree
// ─────────────────────────────────────────────────────────────────────────────

common::backend_tests! {
    async fn test_mmr_proof_endpoint(backend) {
        let server = TestServer::new(backend).await;
        upload_and_commit(&server, 1).await;
        let (_, commit) = upload_and_commit(&server, 1).await;

        let resp = server
            .client
            .get(server.url("/mmr_proof?bucket_id=1&leaf_index=1"))
            .send()
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        let proof: storage_primitives::MmrProof = resp.json().await.unwrap();

        let h256 = |v: &Value| v.as_str().unwrap().parse::<H256>().unwrap();
        assert!(!proof.items.is_empty());
        let commitment = storage_primitives::Commitment {
            mmr_root: h256(&commit["mmr_root"]),
            start_seq: commit["start_seq"].as_u64().unwrap(),
            leaf_count: commit["leaf_count"].as_u64().unwrap(),
        };
        assert!(storage_primitives::verify_mmr_proof(proof, 1, &commitment));
    }
}

common::backend_tests! {
    async fn test_mmr_peaks_endpoint(backend) {
        let server = TestServer::new(backend).await;
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
        let peaks = body["peaks"].as_array().unwrap();
        assert!(!peaks.is_empty());
    }
}

common::backend_tests! {
    async fn test_mmr_peaks_nonexistent_bucket(backend) {
        let server = TestServer::new(backend).await;

        let resp = server
            .client
            .get(server.url("/mmr_peaks?bucket_id=999"))
            .send()
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::NOT_FOUND);

        let body: Value = resp.json().await.unwrap();
        assert_eq!(body["error"], "bucket_not_found");
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// MMR leaves
// ─────────────────────────────────────────────────────────────────────────────

/// Upload and commit `count` distinct single-chunk leaves to `bucket_id`, one
/// commit per leaf.
async fn commit_chunks(server: &TestServer, bucket_id: u64, count: u8) {
    for i in 0..count {
        let data = vec![i; 8];
        let hash = format!("{:#x}", storage_primitives::hash_leaf(&data));
        let resp = server
            .client
            .request_bucket(Method::PUT, server.url("/node"), bucket_id)
            .json(&json!({
                "bucket_id": bucket_id,
                "hash": hash,
                "data": BASE64.encode(&data),
                "children": null,
            }))
            .send()
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        let resp = server
            .client
            .request_bucket(Method::POST, server.url("/commit"), bucket_id)
            .json(&json!({ "bucket_id": bucket_id, "data_roots": [hash] }))
            .send()
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
    }
}

async fn get_leaves(server: &TestServer, query: &str) -> (StatusCode, Value) {
    let resp = server
        .client
        .get(server.url(&format!("/mmr_leaves?bucket_id=1&{query}")))
        .send()
        .await
        .unwrap();
    let status = resp.status();
    (status, resp.json().await.unwrap())
}

common::backend_tests! {
    async fn test_mmr_leaves_pages_through_the_range(backend) {
        let limits = PageLimits {
            max_items: 2,
            ..PageLimits::default()
        };
        let server = TestServer::new_with_page_limits(backend, limits).await;
        commit_chunks(&server, 1, 5).await;

        let mut from = 0;
        let mut pages = Vec::new();
        let mut leaves = Vec::new();
        while from < 5 {
            let (status, body) = get_leaves(&server, &format!("from_seq={from}&to_seq=5")).await;
            assert_eq!(status, StatusCode::OK);
            assert_eq!(body["bucket_id"], 1);
            assert_eq!(body["from_seq"], from);
            let next = body["next_seq"].as_u64().unwrap();
            leaves.extend(body["leaves"].as_array().unwrap().clone());
            pages.push(next);
            from = next;
        }
        assert_eq!(pages, [2, 4, 5]);
        assert_eq!(leaves.len(), 5);
        assert_eq!(leaves[0]["leaf_version"], 1);
        assert_eq!(leaves[0]["data_size"], 8);
        assert_eq!(leaves[4]["total_size"], 40);
        assert_eq!(
            leaves[3]["data_root"],
            format!("{:#x}", storage_primitives::hash_leaf(&[3u8; 8]))
        );
    }
}

common::backend_tests! {
    async fn test_mmr_leaves_conflicts_on_deleted_and_missing_ranges(backend) {
        let server = TestServer::new(backend).await;
        commit_chunks(&server, 1, 5).await;

        let (status, body) = get_leaves(&server, "from_seq=0&to_seq=6").await;
        assert_eq!(status, StatusCode::CONFLICT);
        assert_eq!(body["error"], "leaves_beyond_end");

        let resp = server
            .client
            .request_bucket(Method::POST, server.url("/delete"), 1)
            .json(&json!({ "bucket_id": 1, "new_start_seq": 2 }))
            .send()
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);

        let (status, body) = get_leaves(&server, "from_seq=1&to_seq=5").await;
        assert_eq!(status, StatusCode::CONFLICT);
        assert_eq!(body["error"], "leaves_deleted");

        let (status, body) = get_leaves(&server, "from_seq=2&to_seq=5").await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(body["leaves"].as_array().unwrap().len(), 3);
    }
}

common::backend_tests! {
    async fn test_mmr_leaves_nonexistent_bucket(backend) {
        let server = TestServer::new(backend).await;

        let resp = server
            .client
            .get(server.url("/mmr_leaves?bucket_id=999&from_seq=0&to_seq=0"))
            .send()
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::NOT_FOUND);
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Fetch nodes
// ─────────────────────────────────────────────────────────────────────────────

common::backend_tests! {
    async fn test_fetch_nodes_endpoint(backend) {
        let server = TestServer::new(backend).await;

        let data = b"fetch-nodes-test-data";
        let hash = storage_primitives::hash_leaf(data);
        let hash_hex = format!("{hash:#x}");

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
            .post(server.url("/fetch_nodes"))
            .json(&json!({
                "bucket_id": 1,
                "hashes": [hash_hex],
            }))
            .send()
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);

        let body: Value = resp.json().await.unwrap();
        let nodes = body["nodes"].as_array().unwrap();
        assert_eq!(nodes.len(), 1);
        assert_eq!(nodes[0]["hash"], hash_hex);
        // data should be base64-encoded original data
        let decoded = BASE64.decode(nodes[0]["data"].as_str().unwrap()).unwrap();
        assert_eq!(decoded, data);
    }
}

common::backend_tests! {
    async fn test_fetch_nodes_unknown_hashes(backend) {
        let server = TestServer::new(backend).await;
        upload_and_commit(&server, 1).await;

        let fake = "0x0000000000000000000000000000000000000000000000000000000000000042";
        let resp = server
            .client
            .post(server.url("/fetch_nodes"))
            .json(&json!({
                "bucket_id": 1,
                "hashes": [fake],
            }))
            .send()
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);

        let body: Value = resp.json().await.unwrap();
        assert!(body["nodes"].as_array().unwrap().is_empty());
        assert_eq!(body["missing"], json!([fake]));
        assert!(body["next"].as_array().unwrap().is_empty());
    }
}

/// Upload single-chunk nodes of `len` bytes each and return their hashes.
async fn upload_chunks(server: &TestServer, count: u8, len: usize) -> Vec<String> {
    let mut hashes = Vec::new();
    for i in 0..count {
        let data = vec![i; len];
        let hash = format!("{:#x}", storage_primitives::hash_leaf(&data));
        let resp = server
            .client
            .request_bucket(Method::PUT, server.url("/node"), 1)
            .json(&json!({
                "bucket_id": 1,
                "hash": hash,
                "data": BASE64.encode(&data),
                "children": null,
            }))
            .send()
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        hashes.push(hash);
    }
    hashes
}

async fn fetch_nodes(server: &TestServer, hashes: &[String]) -> Value {
    let resp = server
        .client
        .post(server.url("/fetch_nodes"))
        .json(&json!({ "bucket_id": 1, "hashes": hashes }))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    resp.json().await.unwrap()
}

/// Hashes of the `nodes` entries, in order.
fn node_hashes(body: &Value) -> Vec<Value> {
    body["nodes"]
        .as_array()
        .unwrap()
        .iter()
        .map(|node| node["hash"].clone())
        .collect()
}

common::backend_tests! {
    async fn test_fetch_nodes_stops_at_the_item_limit(backend) {
        let limits = PageLimits {
            max_items: 3,
            ..PageLimits::default()
        };
        let server = TestServer::new_with_page_limits(backend, limits).await;
        let stored = upload_chunks(&server, 4, 8).await;
        let fake = "0x0000000000000000000000000000000000000000000000000000000000000042".to_string();

        // The missing hash counts against the limit; the rest wait in `next`.
        let requested = [stored[0].clone(), fake.clone(), stored[1].clone(), stored[2].clone(), stored[3].clone()];
        let body = fetch_nodes(&server, &requested).await;
        assert_eq!(node_hashes(&body), [json!(stored[0]), json!(stored[1])]);
        assert_eq!(body["missing"], json!([fake]));
        assert_eq!(body["next"], json!([stored[2], stored[3]]));

        // Requesting `next` again finishes the work.
        let body = fetch_nodes(&server, &[stored[2].clone(), stored[3].clone()]).await;
        assert_eq!(node_hashes(&body), [json!(stored[2]), json!(stored[3])]);
        assert!(body["next"].as_array().unwrap().is_empty());
    }
}

common::backend_tests! {
    async fn test_fetch_nodes_stops_at_the_byte_limit(backend) {
        let limits = PageLimits {
            max_bytes: 20,
            ..PageLimits::default()
        };
        let server = TestServer::new_with_page_limits(backend, limits).await;
        let stored = upload_chunks(&server, 3, 8).await;

        // Two 8-byte chunks fit in 20 bytes; the third would not.
        let body = fetch_nodes(&server, &stored).await;
        assert_eq!(node_hashes(&body), [json!(stored[0]), json!(stored[1])]);
        assert_eq!(body["next"], json!([stored[2]]));
        assert!(body["missing"].as_array().unwrap().is_empty());
    }
}

common::backend_tests! {
    async fn test_fetch_nodes_returns_an_oversized_node_alone(backend) {
        let limits = PageLimits {
            max_bytes: 4,
            ..PageLimits::default()
        };
        let server = TestServer::new_with_page_limits(backend, limits).await;
        let stored = upload_chunks(&server, 2, 8).await;

        let body = fetch_nodes(&server, &stored).await;
        assert_eq!(node_hashes(&body), [json!(stored[0])]);
        assert_eq!(body["next"], json!([stored[1]]));
    }
}

common::backend_tests! {
    async fn test_fetch_nodes_lists_each_hash_once(backend) {
        let limits = PageLimits {
            max_items: 4,
            max_bytes: 20,
        };
        let server = TestServer::new_with_page_limits(backend, limits).await;
        let stored = upload_chunks(&server, 3, 8).await;
        let fake = "0x0000000000000000000000000000000000000000000000000000000000000042".to_string();
        let requested = [fake.clone(), stored[0].clone(), stored[1].clone(), stored[2].clone()];

        let body = fetch_nodes(&server, &requested).await;
        let mut seen: Vec<Value> = node_hashes(&body);
        for list in ["missing", "next"] {
            seen.extend(body[list].as_array().unwrap().clone());
        }
        let mut seen: Vec<String> = seen.iter().map(|v| v.as_str().unwrap().to_string()).collect();
        seen.sort();
        let mut expected = requested.to_vec();
        expected.sort();
        assert_eq!(seen, expected);
    }
}

common::backend_tests! {
    async fn test_hash_keyed_reads_reject_a_bucket_the_provider_does_not_store(backend) {
        let server = TestServer::new(backend).await;
        let (hash_hex, _body) = upload_and_commit(&server, 1).await;

        for request in [
            server
                .client
                .get(server.url(&format!("/node?bucket_id=2&hash={hash_hex}"))),
            server.client.get(server.url(&format!(
                "/read?bucket_id=2&data_root={hash_hex}&offset=0&length=100"
            ))),
            server.client.get(server.url(&format!(
                "/chunk_proof?bucket_id=2&data_root={hash_hex}&chunk_index=0"
            ))),
            server
                .client
                .post(server.url("/fetch_nodes"))
                .json(&json!({ "bucket_id": 2, "hashes": [hash_hex] })),
        ] {
            let resp = request.send().await.unwrap();
            assert_eq!(resp.status(), StatusCode::NOT_FOUND);
            let body: Value = resp.json().await.unwrap();
            assert_eq!(
                body["error"], "bucket_not_found",
                "the provider stores this hash only under bucket 1, so a read that names \
                 bucket 2 must fail before backends store buckets separately"
            );
        }
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Replica sync status endpoints
// ─────────────────────────────────────────────────────────────────────────────

common::backend_tests! {
    async fn test_historical_roots_endpoint(backend) {
        let server = TestServer::new(backend).await;
        upload_and_commit(&server, 1).await;

        let resp = server
            .client
            .get(server.url("/replica/historical_roots?bucket_id=1"))
            .send()
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);

        let body: Value = resp.json().await.unwrap();
        assert_eq!(body["bucket_id"], 1);
        // current_root should be a non-zero hash
        let current_root = body["current_root"].as_str().unwrap();
        assert_ne!(
            current_root,
            "0x0000000000000000000000000000000000000000000000000000000000000000"
        );
        // historical_roots is an array of 6 empty strings (provider doesn't track them)
        let hist = body["historical_roots"].as_array().unwrap();
        assert_eq!(hist.len(), 6);
    }
}

common::backend_tests! {
    async fn test_replica_sync_status_endpoint(backend) {
        let server = TestServer::new(backend).await;
        upload_and_commit(&server, 1).await;

        let resp = server
            .client
            .get(server.url("/replica/sync_status?bucket_id=1"))
            .send()
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);

        let body: Value = resp.json().await.unwrap();
        assert_eq!(body["bucket_id"], 1);
        assert!(body["local_mmr_root"].is_string());
        assert!(body["local_leaf_count"].as_u64().unwrap() > 0);
        assert_eq!(body["syncing"], false);
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Delete happy path
// ─────────────────────────────────────────────────────────────────────────────

common::backend_tests! {
    async fn test_delete_happy_path(backend) {
        let server = TestServer::new(backend).await;

        // Commit two data roots
        let data1 = b"chunk-for-delete-1";
        let hash1 = storage_primitives::hash_leaf(data1);
        let hash1_hex = format!("{hash1:#x}");

        let data2 = b"chunk-for-delete-2";
        let hash2 = storage_primitives::hash_leaf(data2);
        let hash2_hex = format!("{hash2:#x}");

        for (hash_hex, data) in [(&hash1_hex, &data1[..]), (&hash2_hex, &data2[..])] {
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
        .json(&json!({
            "bucket_id": 1,
            "data_roots": [hash1_hex, hash2_hex],
        }))
        .send()
        .await
        .unwrap();

    // Delete with new_start_seq=1 (removes first leaf)
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
    assert_eq!(resp.status(), StatusCode::OK);

        let body: Value = resp.json().await.unwrap();
        assert_eq!(body["start_seq"], 1);
        assert_eq!(body["leaf_count"], 1);

        // The deletion proof is the one signing path with no consumer in this
        // repo, so nothing else would catch it regressing. Verify it like the
        // /commit proofs: a real signature, in the SCALE MultiSignature wire
        // format, over the *post-delete* commitment. A zero placeholder or a
        // raw untagged signature both pass a `starts_with("0x")` check.
        let mmr_root = body["mmr_root"].as_str().unwrap().parse::<H256>().unwrap();
        let sig = expect_sr25519(decode_provider_signature(
            body["provider_signature"].as_str().unwrap(),
        ));
        assert_ne!(sig.0, [0u8; 64], "delete returned a zeroed placeholder");

        let payload = CommitmentPayload::new(
            1,
            Commitment {
                mmr_root,
                start_seq: 1,
                leaf_count: 1,
            },
        );
        assert!(
            sr25519::Pair::verify(&sig, payload.encode(), &alice_public()),
            "delete proof did not verify against //Alice over the post-delete commitment"
        );
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Error path tests: invalid hex, invalid base64, missing data
// ─────────────────────────────────────────────────────────────────────────────

common::backend_tests! {
    async fn test_get_node_invalid_hex(backend) {
        let server = TestServer::new(backend).await;

        let resp = server
            .client
            .get(server.url("/node?bucket_id=1&hash=not_hex_at_all"))
            .send()
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::BAD_REQUEST);

        let body: Value = resp.json().await.unwrap();
        assert_eq!(body["error"], "invalid_hash");
    }
}

common::backend_tests! {
    async fn test_get_node_hash_of_wrong_length(backend) {
        let server = TestServer::new(backend).await;

        let resp = server
            .client
            .get(server.url("/node?bucket_id=1&hash=0x01"))
            .send()
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::BAD_REQUEST);

        let body: Value = resp.json().await.unwrap();
        assert_eq!(body["error"], "invalid_hash");
    }
}

common::backend_tests! {
    async fn test_get_node_not_found(backend) {
        let server = TestServer::new(backend).await;
        upload_and_commit(&server, 1).await;

        let valid_but_absent = "0x0000000000000000000000000000000000000000000000000000000000000099";
        let resp = server
            .client
            .get(server.url(&format!("/node?bucket_id=1&hash={valid_but_absent}")))
            .send()
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::NOT_FOUND);

        let body: Value = resp.json().await.unwrap();
        assert_eq!(body["error"], "not_found");
    }
}

common::backend_tests! {
    async fn test_upload_node_invalid_base64(backend) {
        let server = TestServer::new(backend).await;

        let hash = "0x0000000000000000000000000000000000000000000000000000000000000001";
        let resp = server
            .client
            .put(server.url("/node"))
            .json(&json!({
                "bucket_id": 1,
                "hash": hash,
                "data": "!!!not_base64!!!",
                "children": null
            }))
            .send()
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::BAD_REQUEST);

        let body: Value = resp.json().await.unwrap();
        assert_eq!(body["error"], "serialization_error");
    }
}

common::backend_tests! {
    async fn test_upload_node_invalid_hex_children(backend) {
        let server = TestServer::new(backend).await;

        let resp = server
            .client
            .put(server.url("/node"))
            .json(&json!({
                "bucket_id": 1,
                "hash": format!("{:#x}", H256::zero()),
                "children": ["zzz_invalid_hex"]
            }))
            .send()
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::BAD_REQUEST);

        let body: Value = resp.json().await.unwrap();
        assert_eq!(body["error"], "invalid_hash");
    }
}

common::backend_tests! {
    async fn test_commit_invalid_hex_data_root(backend) {
        let server = TestServer::new(backend).await;

    let resp = server
        .client
        .post(server.url("/commit"))
        .json(&json!({
            "bucket_id": 1,
            "data_roots": ["not_hex"],
        }))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::BAD_REQUEST);

        let body: Value = resp.json().await.unwrap();
        assert_eq!(body["error"], "invalid_hash");
    }
}

common::backend_tests! {
    async fn test_commit_without_signing_key(backend) {
        let server = TestServer::new_unsigned(backend).await;

        // Upload a valid node first (upload doesn't need signing)
        let data = b"commit-no-key";
        let hash = storage_primitives::hash_leaf(data);
        let hash_hex = format!("{hash:#x}");

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
        .json(&json!({
            "bucket_id": 1,
            "data_roots": [hash_hex],
        }))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::SERVICE_UNAVAILABLE);

        let body: Value = resp.json().await.unwrap();
        assert_eq!(body["error"], "signing_unavailable");
    }
}

common::backend_tests! {
    async fn test_read_chunks_invalid_hex(backend) {
        let server = TestServer::new(backend).await;

        let resp = server
            .client
            .get(server.url("/read?bucket_id=1&data_root=not_hex&offset=0&length=100"))
            .send()
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::BAD_REQUEST);

        let body: Value = resp.json().await.unwrap();
        assert_eq!(body["error"], "invalid_hash");
    }
}

common::backend_tests! {
    async fn test_read_chunks_nonexistent_root(backend) {
        let server = TestServer::new(backend).await;
        upload_and_commit(&server, 1).await;

        let valid_hex = "0x0000000000000000000000000000000000000000000000000000000000000001";
        let resp = server
            .client
            .get(server.url(&format!("/read?bucket_id=1&data_root={valid_hex}&offset=0&length=100")))
            .send()
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);

        // No chunks found for nonexistent root → empty chunks array
        let body: Value = resp.json().await.unwrap();
        let chunks = body["chunks"].as_array().unwrap();
        assert!(chunks.is_empty());
    }
}

common::backend_tests! {
    async fn test_chunk_proof_invalid_hex(backend) {
        let server = TestServer::new(backend).await;

        let resp = server
            .client
            .get(server.url("/chunk_proof?bucket_id=1&data_root=not_hex&chunk_index=0"))
            .send()
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::BAD_REQUEST);

        let body: Value = resp.json().await.unwrap();
        assert_eq!(body["error"], "invalid_hash");
    }
}
