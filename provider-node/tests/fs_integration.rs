// SPDX-License-Identifier: GPL-3.0-only

//! Integration tests for the file system HTTP API endpoints.

mod common;

use axum::http::StatusCode;
use common::SignedClient;
use provider_auth::{Authenticator, StaticMembershipResolver};
use provider_storage::NullNonceStore;
use serde_json::Value;
use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Duration;
use storage_primitives::Role;
use storage_provider_node::{ProviderDeps, ProviderState};
use tempfile::TempDir;

struct TestServer {
    addr: SocketAddr,
    client: SignedClient,
    /// RocksDB scratch dir, `None` in-memory. Held so it outlives the server.
    _dir: Option<TempDir>,
}

impl TestServer {
    async fn new(backend: common::Backend) -> Self {
        let (storage, dir) = common::storage_for(backend);
        let deps = ProviderDeps {
            storage,
            nonce_store: Arc::new(NullNonceStore),
            auth: Arc::new(Authenticator::new(
                StaticMembershipResolver(vec![(common::test_member_account(), Role::Admin).into()]),
                Duration::from_secs(60),
                Duration::from_secs(300),
            )),
        };
        let (addr, client) = common::serve(ProviderState::with_provider_id(
            deps,
            "0xtest_provider".to_string(),
        ))
        .await;
        Self {
            addr,
            client,
            _dir: dir,
        }
    }

    fn url(&self, path: &str) -> String {
        format!("http://{}{}", self.addr, path)
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// PUT + GET round-trip
// ─────────────────────────────────────────────────────────────────────────────

common::backend_tests! {
    async fn test_fs_put_and_get_file(backend) {
        let server = TestServer::new(backend).await;

        // PUT
        let resp = server
            .client
            .put(server.url("/fs/1/file?path=/hello.txt"))
            .header("content-type", "text/plain")
            .body("hello world")
            .send()
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);

        let body: Value = resp.json().await.unwrap();
        assert_eq!(body["size"], 11);
        assert!(body["data_root"].is_string());
        assert!(body["leaf_index"].is_number());

        // GET
        let resp = server
            .client
            .get(server.url("/fs/1/file?path=/hello.txt"))
            .send()
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        assert_eq!(resp.headers().get("content-type").unwrap(), "text/plain");

        let data = resp.text().await.unwrap();
        assert_eq!(data, "hello world");
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// GET nonexistent → 404
// ─────────────────────────────────────────────────────────────────────────────

common::backend_tests! {
    async fn test_fs_get_nonexistent_file(backend) {
        let server = TestServer::new(backend).await;

        let resp = server
            .client
            .get(server.url("/fs/1/file?path=/nope.txt"))
            .send()
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::NOT_FOUND);

        let body: Value = resp.json().await.unwrap();
        assert_eq!(body["error"], "file_not_found");
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// DELETE file
// ─────────────────────────────────────────────────────────────────────────────

common::backend_tests! {
    async fn test_fs_delete_file(backend) {
        let server = TestServer::new(backend).await;

        // PUT
        server
            .client
            .put(server.url("/fs/1/file?path=/to_delete.txt"))
            .body("bye")
            .send()
            .await
            .unwrap();

        // DELETE
        let resp = server
            .client
            .delete(server.url("/fs/1/file?path=/to_delete.txt"))
            .send()
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        let body: Value = resp.json().await.unwrap();
        assert_eq!(body["deleted"], true);

        // GET should be 404
        let resp = server
            .client
            .get(server.url("/fs/1/file?path=/to_delete.txt"))
            .send()
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::NOT_FOUND);
    }
}

common::backend_tests! {
    async fn test_fs_delete_nonexistent(backend) {
        let server = TestServer::new(backend).await;

        let resp = server
            .client
            .delete(server.url("/fs/1/file?path=/never_existed.txt"))
            .send()
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);

        let body: Value = resp.json().await.unwrap();
        assert_eq!(body["deleted"], false);
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// mkdir
// ─────────────────────────────────────────────────────────────────────────────

common::backend_tests! {
    async fn test_fs_mkdir(backend) {
        let server = TestServer::new(backend).await;

        let resp = server
            .client
            .post(server.url("/fs/1/mkdir?path=/docs"))
            .send()
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);

        let body: Value = resp.json().await.unwrap();
        assert_eq!(body["path"], "/docs");
        assert_eq!(body["created"], true);
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// ls (list directory)
// ─────────────────────────────────────────────────────────────────────────────

common::backend_tests! {
    async fn test_fs_list_dir(backend) {
        let server = TestServer::new(backend).await;

        // Create a directory and a file
        server
            .client
            .post(server.url("/fs/1/mkdir?path=/docs"))
            .send()
            .await
            .unwrap();

        server
            .client
            .put(server.url("/fs/1/file?path=/readme.txt"))
            .body("read me")
            .send()
            .await
            .unwrap();

        // List root
        let resp = server
            .client
            .get(server.url("/fs/1/ls?path=/"))
            .send()
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);

        let body: Value = resp.json().await.unwrap();
        let entries = body["entries"].as_array().unwrap();
        assert_eq!(entries.len(), 2);
    }
}

common::backend_tests! {
    async fn test_fs_list_dir_recursive(backend) {
        let server = TestServer::new(backend).await;

        // Create nested structure
        server
            .client
            .post(server.url("/fs/1/mkdir?path=/a"))
            .send()
            .await
            .unwrap();
        server
            .client
            .put(server.url("/fs/1/file?path=/a/one.txt"))
            .body("1")
            .send()
            .await
            .unwrap();
        server
            .client
            .put(server.url("/fs/1/file?path=/top.txt"))
            .body("t")
            .send()
            .await
            .unwrap();

        let resp = server
            .client
            .get(server.url("/fs/1/ls?path=/&recursive=true"))
            .send()
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);

        let body: Value = resp.json().await.unwrap();
        let entries = body["entries"].as_array().unwrap();
        // Should include /a, /a/one.txt, /top.txt
        assert!(entries.len() >= 3);
    }
}

common::backend_tests! {
    async fn test_fs_list_dir_empty(backend) {
        let server = TestServer::new(backend).await;

        let resp = server
            .client
            .get(server.url("/fs/1/ls?path=/"))
            .send()
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);

        let body: Value = resp.json().await.unwrap();
        let entries = body["entries"].as_array().unwrap();
        assert!(entries.is_empty());
        assert_eq!(body["file_count"], 0);
        assert_eq!(body["dir_count"], 0);
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// index_root
// ─────────────────────────────────────────────────────────────────────────────

common::backend_tests! {
    async fn test_fs_index_root_empty(backend) {
        let server = TestServer::new(backend).await;

        let resp = server
            .client
            .get(server.url("/fs/1/index_root"))
            .send()
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);

        let body: Value = resp.json().await.unwrap();
        assert_eq!(body["file_count"], 0);
        assert_eq!(body["dir_count"], 0);
        assert_eq!(body["total_size"], 0);
    }
}

common::backend_tests! {
    async fn test_fs_index_root_with_files(backend) {
        let server = TestServer::new(backend).await;

        server
            .client
            .put(server.url("/fs/1/file?path=/a.txt"))
            .body("aaa")
            .send()
            .await
            .unwrap();
        server
            .client
            .put(server.url("/fs/1/file?path=/b.txt"))
            .body("bbb")
            .send()
            .await
            .unwrap();

        let resp = server
            .client
            .get(server.url("/fs/1/index_root"))
            .send()
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);

        let body: Value = resp.json().await.unwrap();
        assert_eq!(body["file_count"], 2);
        assert!(body["metadata_merkle_root"].is_string());
        assert_ne!(
            body["metadata_merkle_root"],
            "0x0000000000000000000000000000000000000000000000000000000000000000"
        );
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Invalid path → 400
// ─────────────────────────────────────────────────────────────────────────────

common::backend_tests! {
    async fn test_fs_put_invalid_path(backend) {
        let server = TestServer::new(backend).await;

        let resp = server
            .client
            .put(server.url("/fs/1/file?path=no_slash"))
            .body("data")
            .send()
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::BAD_REQUEST);

        let body: Value = resp.json().await.unwrap();
        assert_eq!(body["error"], "invalid_path");
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Path traversal (`..`) rejected on every fs endpoint → 400
// ─────────────────────────────────────────────────────────────────────────────

common::backend_tests! {
    async fn test_fs_rejects_path_traversal(backend) {
        let server = TestServer::new(backend).await;

        // Each tuple is (HTTP method, URL) for a path containing `..`.
        let put = server
            .client
            .put(server.url("/fs/1/file?path=/../../etc/passwd"));
        let get = server.client.get(server.url("/fs/1/file?path=/../secret"));
        let del = server
            .client
            .delete(server.url("/fs/1/file?path=/a/../../b"));
        let mkdir = server.client.post(server.url("/fs/1/mkdir?path=/x/../y"));
        let ls = server.client.get(server.url("/fs/1/ls?path=/../"));

        for req in [put, get, del, mkdir, ls] {
            let resp = req.body("data").send().await.unwrap();
            assert_eq!(
                resp.status(),
                StatusCode::BAD_REQUEST,
                "path containing '..' must be rejected"
            );
            let body: Value = resp.json().await.unwrap();
            assert_eq!(body["error"], "invalid_path");
        }
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Overwrite
// ─────────────────────────────────────────────────────────────────────────────

common::backend_tests! {
    async fn test_fs_put_overwrite(backend) {
        let server = TestServer::new(backend).await;

        // PUT v1
        server
            .client
            .put(server.url("/fs/1/file?path=/config.txt"))
            .header("content-type", "text/plain")
            .body("version 1")
            .send()
            .await
            .unwrap();

        // PUT v2 (overwrite)
        server
            .client
            .put(server.url("/fs/1/file?path=/config.txt"))
            .header("content-type", "text/plain")
            .body("version 2")
            .send()
            .await
            .unwrap();

        // GET should return v2
        let resp = server
            .client
            .get(server.url("/fs/1/file?path=/config.txt"))
            .send()
            .await
            .unwrap();
        let data = resp.text().await.unwrap();
        assert_eq!(data, "version 2");

        // Index should show only 1 file
        let resp = server
            .client
            .get(server.url("/fs/1/index_root"))
            .send()
            .await
            .unwrap();
        let body: Value = resp.json().await.unwrap();
        assert_eq!(body["file_count"], 1);
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Large file (multi-chunk)
// ─────────────────────────────────────────────────────────────────────────────

common::backend_tests! {
    async fn test_fs_large_file(backend) {
        let server = TestServer::new(backend).await;

        // 300 KB → 2 chunks (256 KiB threshold)
        let data = vec![0x42u8; 300 * 1024];

        let resp = server
            .client
            .put(server.url("/fs/1/file?path=/large.bin"))
            .header("content-type", "application/octet-stream")
            .body(data.clone())
            .send()
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);

        let body: Value = resp.json().await.unwrap();
        assert_eq!(body["size"], 300 * 1024);

        // GET and verify round-trip
        let resp = server
            .client
            .get(server.url("/fs/1/file?path=/large.bin"))
            .send()
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);

        let returned = resp.bytes().await.unwrap();
        assert_eq!(returned.len(), 300 * 1024);
        assert_eq!(returned.as_ref(), data.as_slice());
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// GET directory → not_a_file error
// ─────────────────────────────────────────────────────────────────────────────

common::backend_tests! {
    async fn test_fs_get_directory_returns_error(backend) {
        let server = TestServer::new(backend).await;

        // Create directory
        server
            .client
            .post(server.url("/fs/1/mkdir?path=/docs"))
            .send()
            .await
            .unwrap();

        // Try to GET the directory as a file
        let resp = server
            .client
            .get(server.url("/fs/1/file?path=/docs"))
            .send()
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::BAD_REQUEST);

        let body: Value = resp.json().await.unwrap();
        assert_eq!(body["error"], "not_a_file");
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Edge cases: empty path, empty body, mkdir validation
// ─────────────────────────────────────────────────────────────────────────────

common::backend_tests! {
    async fn test_fs_put_empty_body(backend) {
        let server = TestServer::new(backend).await;

        // PUT with empty body should succeed (zero-size file)
        let resp = server
            .client
            .put(server.url("/fs/1/file?path=/empty.bin"))
            .body("")
            .send()
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);

        let body: Value = resp.json().await.unwrap();
        assert_eq!(body["size"], 0);

        // GET should return empty body
        let resp = server
            .client
            .get(server.url("/fs/1/file?path=/empty.bin"))
            .send()
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        let data = resp.bytes().await.unwrap();
        assert!(data.is_empty());
    }
}

common::backend_tests! {
    async fn test_fs_mkdir_no_leading_slash(backend) {
        let server = TestServer::new(backend).await;

        let resp = server
            .client
            .post(server.url("/fs/1/mkdir?path=noslash"))
            .send()
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::BAD_REQUEST);

        let body: Value = resp.json().await.unwrap();
        assert_eq!(body["error"], "invalid_path");
    }
}

common::backend_tests! {
    async fn test_fs_delete_file_then_list(backend) {
        let server = TestServer::new(backend).await;

        // Upload two files
        server
            .client
            .put(server.url("/fs/1/file?path=/a.txt"))
            .body("aaa")
            .send()
            .await
            .unwrap();
        server
            .client
            .put(server.url("/fs/1/file?path=/b.txt"))
            .body("bbb")
            .send()
            .await
            .unwrap();

        // Delete one
        let resp = server
            .client
            .delete(server.url("/fs/1/file?path=/a.txt"))
            .send()
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);

        // List should show only 1 file
        let resp = server
            .client
            .get(server.url("/fs/1/ls?path=/"))
            .send()
            .await
            .unwrap();
        let body: Value = resp.json().await.unwrap();
        assert_eq!(body["file_count"], 1);
    }
}

common::backend_tests! {
    async fn test_fs_nested_mkdir_and_file(backend) {
        let server = TestServer::new(backend).await;

        // Create nested dir
        server
            .client
            .post(server.url("/fs/1/mkdir?path=/a"))
            .send()
            .await
            .unwrap();
        server
            .client
            .post(server.url("/fs/1/mkdir?path=/a/b"))
            .send()
            .await
            .unwrap();

        // Put file in nested dir
        let resp = server
            .client
            .put(server.url("/fs/1/file?path=/a/b/deep.txt"))
            .body("deep")
            .send()
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);

        // GET from nested path
        let resp = server
            .client
            .get(server.url("/fs/1/file?path=/a/b/deep.txt"))
            .send()
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        assert_eq!(resp.text().await.unwrap(), "deep");
    }
}
