// SPDX-License-Identifier: GPL-3.0-only

//! Integration tests for S3-compatible object storage endpoints.

mod common;

use axum::http::StatusCode;
use base64::{engine::general_purpose::STANDARD as BASE64, Engine};
use serde_json::Value;
use sp_core::H256;
use storage_provider_node::ProviderState;

use common::{StorageBackendKind, TestServer};

/// Decodes a `0x`-prefixed (or bare) hex hash into an `H256`.
fn h256_from_hex(s: &str) -> H256 {
    let bytes = hex::decode(s.strip_prefix("0x").unwrap_or(s)).unwrap();
    H256::from_slice(&bytes)
}

impl TestServer {
    async fn new(backend: StorageBackendKind) -> Self {
        Self::start(backend, |deps| {
            ProviderState::with_provider_id(deps, "0xtest_provider".to_string())
        })
        .await
    }
}

common::backend_tests! {
    async fn test_s3_put_and_get_object(backend) {
        let server = TestServer::new(backend).await;

        // PUT object
        let response = server
            .client
            .put(server.url("/s3/1/object?key=greeting.txt"))
            .header("content-type", "text/plain")
            .body("hello world")
            .send()
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::OK);
        let body: Value = response.json().await.unwrap();
        assert_eq!(body["size"], 11);
        assert!(body["etag"].is_string());
        assert!(body["data_root"].is_string());
        assert!(body["leaf_index"].is_number());

        // GET object
        let response = server
            .client
            .get(server.url("/s3/1/object?key=greeting.txt"))
            .send()
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(
            response.headers().get("content-type").unwrap(),
            "text/plain"
        );

        let data = response.text().await.unwrap();
        assert_eq!(data, "hello world");
    }
}

common::backend_tests! {
    async fn test_s3_head_object(backend) {
        let server = TestServer::new(backend).await;

        // PUT first
        server
            .client
            .put(server.url("/s3/1/object?key=test.txt"))
            .header("content-type", "text/plain")
            .header("x-amz-meta-author", "alice")
            .body("test data here")
            .send()
            .await
            .unwrap();

        // HEAD
        let response = server
            .client
            .head(server.url("/s3/1/object?key=test.txt"))
            .send()
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(
            response.headers().get("content-type").unwrap(),
            "text/plain"
        );
        assert_eq!(response.headers().get("content-length").unwrap(), "14");
        assert!(response.headers().get("etag").is_some());
        assert!(response.headers().get("x-amz-data-root").is_some());
        assert_eq!(
            response.headers().get("x-amz-meta-author").unwrap(),
            "alice"
        );
    }
}

common::backend_tests! {
    async fn test_s3_delete_object(backend) {
        let server = TestServer::new(backend).await;

        // PUT
        server
            .client
            .put(server.url("/s3/1/object?key=deleteme.txt"))
            .body("to be deleted")
            .send()
            .await
            .unwrap();

        // DELETE
        let response = server
            .client
            .delete(server.url("/s3/1/object?key=deleteme.txt"))
            .send()
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::OK);
        let body: Value = response.json().await.unwrap();
        assert_eq!(body["deleted"], true);

        // GET should return 404
        let response = server
            .client
            .get(server.url("/s3/1/object?key=deleteme.txt"))
            .send()
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::NOT_FOUND);
    }
}

common::backend_tests! {
    async fn test_s3_get_nonexistent_returns_404(backend) {
        let server = TestServer::new(backend).await;

        let response = server
            .client
            .get(server.url("/s3/1/object?key=nonexistent.txt"))
            .send()
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::NOT_FOUND);
    }
}

common::backend_tests! {
    async fn test_s3_list_objects(backend) {
        let server = TestServer::new(backend).await;

        // Upload several objects
        for name in &["photos/cat.jpg", "photos/dog.jpg", "docs/readme.txt"] {
            server
                .client
                .put(server.url(&format!("/s3/1/object?key={name}")))
                .body("data")
                .send()
                .await
                .unwrap();
        }

        // List all
        let response = server
            .client
            .get(server.url("/s3/1/objects"))
            .send()
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::OK);
        let body: Value = response.json().await.unwrap();
        assert_eq!(body["key_count"], 3);

        // List with prefix
        let response = server
            .client
            .get(server.url("/s3/1/objects?prefix=photos/"))
            .send()
            .await
            .unwrap();

        let body: Value = response.json().await.unwrap();
        assert_eq!(body["key_count"], 2);

        // List with delimiter
        let response = server
            .client
            .get(server.url("/s3/1/objects?delimiter=/"))
            .send()
            .await
            .unwrap();

        let body: Value = response.json().await.unwrap();
        let common_prefixes = body["common_prefixes"].as_array().unwrap();
        assert_eq!(common_prefixes.len(), 2); // "docs/", "photos/"
    }
}

common::backend_tests! {
    async fn test_s3_list_pagination(backend) {
        let server = TestServer::new(backend).await;

        // Upload 5 objects
        for i in 0..5 {
            server
                .client
                .put(server.url(&format!("/s3/1/object?key=file_{i:03}.txt")))
                .body("data")
                .send()
                .await
                .unwrap();
        }

        // Page 1 (max_keys=2)
        let response = server
            .client
            .get(server.url("/s3/1/objects?max_keys=2"))
            .send()
            .await
            .unwrap();

        let body: Value = response.json().await.unwrap();
        assert_eq!(body["key_count"], 2);
        assert_eq!(body["is_truncated"], true);
        assert!(body["next_continuation_token"].is_string());

        // Page 2
        let token = body["next_continuation_token"].as_str().unwrap();
        let response = server
            .client
            .get(server.url(&format!(
                "/s3/1/objects?max_keys=2&continuation_token={token}"
            )))
            .send()
            .await
            .unwrap();

        let body: Value = response.json().await.unwrap();
        assert_eq!(body["key_count"], 2);
        assert_eq!(body["is_truncated"], true);
    }
}

common::backend_tests! {
    async fn test_s3_index_root(backend) {
        let server = TestServer::new(backend).await;

        // Empty bucket
        let response = server
            .client
            .get(server.url("/s3/1/index_root"))
            .send()
            .await
            .unwrap();

        let body: Value = response.json().await.unwrap();
        assert_eq!(body["object_count"], 0);
        assert_eq!(body["total_size"], 0);

        // Add objects
        server
            .client
            .put(server.url("/s3/1/object?key=a.txt"))
            .body("hello")
            .send()
            .await
            .unwrap();

        server
            .client
            .put(server.url("/s3/1/object?key=b.txt"))
            .body("world!")
            .send()
            .await
            .unwrap();

        let response = server
            .client
            .get(server.url("/s3/1/index_root"))
            .send()
            .await
            .unwrap();

        let body: Value = response.json().await.unwrap();
        assert_eq!(body["object_count"], 2);
        assert_eq!(body["total_size"], 11); // 5 + 6
        assert!(body["metadata_merkle_root"].is_string());
        // Non-zero root
        assert_ne!(
            body["metadata_merkle_root"],
            "0x0000000000000000000000000000000000000000000000000000000000000000"
        );
    }
}

common::backend_tests! {
    async fn test_s3_large_file_multi_chunk(backend) {
        let server = TestServer::new(backend).await;

        // Create data larger than one chunk (256 KiB = 262144 bytes)
        // Use 300 KB to get 2 chunks
        let data = vec![0x42u8; 300 * 1024];

        let response = server
            .client
            .put(server.url("/s3/1/object?key=large.bin"))
            .header("content-type", "application/octet-stream")
            .body(data.clone())
            .send()
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::OK);
        let body: Value = response.json().await.unwrap();
        assert_eq!(body["size"], 300 * 1024);

        // GET and verify round-trip
        let response = server
            .client
            .get(server.url("/s3/1/object?key=large.bin"))
            .send()
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::OK);
        let returned_data = response.bytes().await.unwrap();
        assert_eq!(returned_data.len(), 300 * 1024);
        assert_eq!(returned_data.as_ref(), data.as_slice());
    }
}

common::backend_tests! {
    async fn test_s3_put_overwrite(backend) {
        let server = TestServer::new(backend).await;

        // PUT v1
        server
            .client
            .put(server.url("/s3/1/object?key=config.json"))
            .header("content-type", "application/json")
            .body(r#"{"v":1}"#)
            .send()
            .await
            .unwrap();

        // PUT v2 (overwrite)
        server
            .client
            .put(server.url("/s3/1/object?key=config.json"))
            .header("content-type", "application/json")
            .body(r#"{"v":2}"#)
            .send()
            .await
            .unwrap();

        // GET should return v2
        let response = server
            .client
            .get(server.url("/s3/1/object?key=config.json"))
            .send()
            .await
            .unwrap();

        let data = response.text().await.unwrap();
        assert_eq!(data, r#"{"v":2}"#);

        // Index should show only 1 object
        let response = server
            .client
            .get(server.url("/s3/1/index_root"))
            .send()
            .await
            .unwrap();

        let body: Value = response.json().await.unwrap();
        assert_eq!(body["object_count"], 1);
    }
}

common::backend_tests! {
    async fn test_s3_mmr_proof_works_with_s3_data(backend) {
        let server = TestServer::new(backend).await;

        // Upload via S3 API
        let put_response = server
            .client
            .put(server.url("/s3/1/object?key=proof-test.txt"))
            .body("data for proof test")
            .send()
            .await
            .unwrap();

        let put_body: Value = put_response.json().await.unwrap();
        let leaf_index = put_body["leaf_index"].as_u64().unwrap();

        // Verify MMR proof still works
        let proof_response = server
            .client
            .get(server.url(&format!("/mmr_proof?bucket_id=1&leaf_index={leaf_index}")))
            .send()
            .await
            .unwrap();

        assert_eq!(proof_response.status(), StatusCode::OK);

        let body: Value = proof_response.json().await.unwrap();
        assert!(body["leaf"]["data_root"].is_string());
        assert!(body["proof"]["peaks"].is_array());
    }
}

common::backend_tests! {
    async fn test_s3_chunk_proofs_verify_across_multiple_chunks(backend) {
        let server = TestServer::new(backend).await;

        // Two full chunks plus a partial one, so the object spans more than
        // two chunks and the padded proof tree is not itself a power of two.
        let chunk_size = storage_primitives::DEFAULT_CHUNK_SIZE as usize;
        let data: Vec<u8> = (0..2 * chunk_size + 50_000)
            .map(|i| (i * 37 + 11) as u8)
            .collect();
        let chunk_count = data.len().div_ceil(chunk_size);
        assert!(chunk_count > 2, "test data must span more than two chunks");

        let put_response = server
            .client
            .put(server.url("/s3/1/object?key=multi-chunk.bin"))
            .body(data.clone())
            .send()
            .await
            .unwrap();
        assert_eq!(put_response.status(), StatusCode::OK);
        let put_body: Value = put_response.json().await.unwrap();
        let leaf_index = put_body["leaf_index"].as_u64().unwrap();

        // Resolve the data_root the object landed under.
        let proof_response = server
            .client
            .get(server.url(&format!("/mmr_proof?bucket_id=1&leaf_index={leaf_index}")))
            .send()
            .await
            .unwrap();
        assert_eq!(proof_response.status(), StatusCode::OK);
        let proof_body: Value = proof_response.json().await.unwrap();
        let data_root_hex = proof_body["leaf"]["data_root"]
            .as_str()
            .unwrap()
            .to_string();
        let data_root = h256_from_hex(&data_root_hex);

        for chunk_index in 0..chunk_count {
            let resp = server
                .client
                .get(server.url(&format!(
                    "/chunk_proof?data_root={data_root_hex}&chunk_index={chunk_index}"
                )))
                .send()
                .await
                .unwrap();
            assert_eq!(resp.status(), StatusCode::OK);
            let body: Value = resp.json().await.unwrap();

            let expected_start = chunk_index * chunk_size;
            let expected_end = (expected_start + chunk_size).min(data.len());
            let expected_chunk = &data[expected_start..expected_end];

            let chunk_data = BASE64
                .decode(body["chunk_data"].as_str().unwrap())
                .unwrap();
            assert_eq!(
                chunk_data.as_slice(),
                expected_chunk,
                "chunk {chunk_index} data does not match the uploaded slice"
            );

            let chunk_hash = h256_from_hex(body["chunk_hash"].as_str().unwrap());
            assert_eq!(chunk_hash, storage_primitives::blake2_256(expected_chunk));

            let siblings: Vec<H256> = body["proof"]["siblings"]
                .as_array()
                .unwrap()
                .iter()
                .map(|s| h256_from_hex(s.as_str().unwrap()))
                .collect();
            let path: Vec<bool> = body["proof"]["path"]
                .as_array()
                .unwrap()
                .iter()
                .map(|b| b.as_bool().unwrap())
                .collect();

            let proof = storage_primitives::MerkleProof { siblings, path };
            assert!(
                storage_primitives::verify_merkle_proof(
                    chunk_hash,
                    chunk_index as u64,
                    &proof,
                    &data_root
                ),
                "chunk {chunk_index} proof did not verify against the data root"
            );
        }
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Edge cases: empty key, nonexistent HEAD/DELETE, empty body, metadata on GET
// ─────────────────────────────────────────────────────────────────────────────

common::backend_tests! {
    async fn test_s3_head_nonexistent(backend) {
        let server = TestServer::new(backend).await;

        let response = server
            .client
            .head(server.url("/s3/1/object?key=nope.txt"))
            .send()
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::NOT_FOUND);
    }
}

common::backend_tests! {
    async fn test_s3_delete_nonexistent(backend) {
        let server = TestServer::new(backend).await;

        let response = server
            .client
            .delete(server.url("/s3/1/object?key=nope.txt"))
            .send()
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);

        let body: Value = response.json().await.unwrap();
        assert_eq!(body["deleted"], false);
    }
}

common::backend_tests! {
    async fn test_s3_put_empty_body(backend) {
        let server = TestServer::new(backend).await;

        let response = server
            .client
            .put(server.url("/s3/1/object?key=empty.bin"))
            .body("")
            .send()
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);

        let body: Value = response.json().await.unwrap();
        assert_eq!(body["size"], 0);

        // GET should return empty body
        let response = server
            .client
            .get(server.url("/s3/1/object?key=empty.bin"))
            .send()
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        let data = response.bytes().await.unwrap();
        assert!(data.is_empty());
    }
}

common::backend_tests! {
    async fn test_s3_metadata_roundtrip_on_get(backend) {
        let server = TestServer::new(backend).await;

        // PUT with custom metadata
        server
            .client
            .put(server.url("/s3/1/object?key=meta.txt"))
            .header("content-type", "text/plain")
            .header("x-amz-meta-project", "web3-storage")
            .header("x-amz-meta-env", "test")
            .body("metadata test")
            .send()
            .await
            .unwrap();

        // GET should include metadata in response headers
        let response = server
            .client
            .get(server.url("/s3/1/object?key=meta.txt"))
            .send()
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);

        assert_eq!(
            response.headers().get("x-amz-meta-project").unwrap(),
            "web3-storage"
        );
        assert_eq!(response.headers().get("x-amz-meta-env").unwrap(), "test");
        assert_eq!(
            response.headers().get("content-type").unwrap(),
            "text/plain"
        );
        assert_eq!(response.text().await.unwrap(), "metadata test");
    }
}

common::backend_tests! {
    async fn test_s3_list_empty_bucket(backend) {
        let server = TestServer::new(backend).await;

        let response = server
            .client
            .get(server.url("/s3/1/objects"))
            .send()
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);

        let body: Value = response.json().await.unwrap();
        assert_eq!(body["key_count"], 0);
        assert_eq!(body["is_truncated"], false);
        assert!(body["contents"].as_array().unwrap().is_empty());
    }
}
