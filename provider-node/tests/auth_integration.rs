// SPDX-License-Identifier: GPL-3.0-only

//! Integration tests for auth-enabled HTTP endpoints.
//!
//! These tests spin up a real HTTP server whose membership is a fixed member
//! set with configurable roles per test account. All assertions go through
//! real HTTP requests — the auth middleware, signature verification,
//! membership cache lookup, and role check are exercised as a single
//! end-to-end path.

mod common;

use axum::http::StatusCode;
use base64::{engine::general_purpose::STANDARD as BASE64, Engine};
use common::{current_timestamp, make_auth_header};
use provider_auth::{
    Authenticator, BucketAccess, Member, MembershipError, MembershipResolver,
    StaticMembershipResolver,
};
use provider_storage::temp_rocksdb;
use reqwest::Client;
use serde_json::Value;
use sp_core::{sr25519, Pair};
use std::sync::Arc;
use std::time::Duration;
use storage_primitives::{BucketId, Role, Visibility};
use storage_provider_node::{create_router, ProviderDeps, ProviderState};
use tokio::net::TcpListener;

type AccountId32 = sp_core::crypto::AccountId32;

/// A resolver whose buckets are all `Public` — exercises the
/// visibility-aware Reader gate (anonymous reads allowed, writes still
/// authenticated).
struct PublicBucketResolver(Vec<Member>);

#[async_trait::async_trait]
impl MembershipResolver for PublicBucketResolver {
    async fn fetch_access(&self, _bucket_id: BucketId) -> Result<BucketAccess, MembershipError> {
        Ok(BucketAccess {
            members: self.0.clone(),
            visibility: Visibility::Public,
        })
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Test server
// ─────────────────────────────────────────────────────────────────────────────

struct AuthTestServer {
    addr: std::net::SocketAddr,
    client: Client,
    /// This server's scratch directory; dropped with the server.
    _dir: tempfile::TempDir,
}

impl AuthTestServer {
    /// Start a server with auth enabled and Alice as the given role.
    /// Buckets resolve as `Private` (the static resolver's default).
    async fn with_role(alice_role: Role) -> Self {
        let alice_kp = sr25519::Pair::from_string("//Alice", None).unwrap();
        let alice_account = AccountId32::new(alice_kp.public().0);
        Self::with_resolver(StaticMembershipResolver(vec![
            (alice_account, alice_role).into()
        ]))
        .await
    }

    /// Same, but every bucket resolves as `Public`.
    async fn public_with_role(alice_role: Role) -> Self {
        let alice_kp = sr25519::Pair::from_string("//Alice", None).unwrap();
        let alice_account = AccountId32::new(alice_kp.public().0);
        Self::with_resolver(PublicBucketResolver(vec![
            (alice_account, alice_role).into()
        ]))
        .await
    }

    async fn with_resolver(resolver: impl MembershipResolver + 'static) -> Self {
        // The 300s skew keeps the default the `*_expired_timestamp` tests assume.
        let (storage, nonce_store, dir) = temp_rocksdb();
        let deps = ProviderDeps {
            storage,
            nonce_store,
            auth: Arc::new(Authenticator::new(resolver)),
        };
        let state = ProviderState::with_seed(deps, "//Alice").expect("//Alice is valid");

        let app = create_router(Arc::new(state));
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
        tokio::time::sleep(Duration::from_millis(10)).await;

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

// ─────────────────────────────────────────────────────────────────────────────
// S3 endpoint auth tests
// ─────────────────────────────────────────────────────────────────────────────

#[tokio::test]
async fn s3_writer_can_put_object() {
    let server = AuthTestServer::with_role(Role::Writer).await;
    let alice = sr25519::Pair::from_string("//Alice", None).unwrap();
    let ts = current_timestamp();
    let header = make_auth_header(&alice, "PUT", 1, ts);

    let resp = server
        .client
        .put(server.url("/s3/1/object?key=hello.txt"))
        .header("Authorization", &header)
        .body(b"hello world".to_vec())
        .send()
        .await
        .unwrap();

    assert_eq!(resp.status(), StatusCode::OK);
    let body: Value = resp.json().await.unwrap();
    assert!(body["etag"].is_string());
    assert!(body["data_root"].is_string());
}

#[tokio::test]
async fn s3_reader_blocked_from_put() {
    let server = AuthTestServer::with_role(Role::Reader).await;
    let alice = sr25519::Pair::from_string("//Alice", None).unwrap();
    let ts = current_timestamp();
    let header = make_auth_header(&alice, "PUT", 1, ts);

    let resp = server
        .client
        .put(server.url("/s3/1/object?key=hello.txt"))
        .header("Authorization", &header)
        .body(b"hello world".to_vec())
        .send()
        .await
        .unwrap();

    assert_eq!(resp.status(), StatusCode::FORBIDDEN);
    let body: Value = resp.json().await.unwrap();
    assert_eq!(body["error"], "insufficient_role");
}

#[tokio::test]
async fn s3_reader_can_get_object() {
    let server = AuthTestServer::with_role(Role::Writer).await;
    let alice = sr25519::Pair::from_string("//Alice", None).unwrap();

    // First PUT (as Writer)
    let ts = current_timestamp();
    let header = make_auth_header(&alice, "PUT", 1, ts);
    server
        .client
        .put(server.url("/s3/1/object?key=read-me.txt"))
        .header("Authorization", &header)
        .body(b"readable data".to_vec())
        .send()
        .await
        .unwrap();

    // GET (Reader level is sufficient)
    let ts = current_timestamp();
    let header = make_auth_header(&alice, "GET", 1, ts);
    let resp = server
        .client
        .get(server.url("/s3/1/object?key=read-me.txt"))
        .header("Authorization", &header)
        .send()
        .await
        .unwrap();

    assert_eq!(resp.status(), StatusCode::OK);
    assert_eq!(resp.bytes().await.unwrap().as_ref(), b"readable data");
}

#[tokio::test]
async fn public_bucket_s3_get_served_without_auth() {
    let server = AuthTestServer::public_with_role(Role::Writer).await;
    let alice = sr25519::Pair::from_string("//Alice", None).unwrap();

    // Seed an object (writes stay authenticated even on public buckets).
    let ts = current_timestamp();
    let header = make_auth_header(&alice, "PUT", 1, ts);
    server
        .client
        .put(server.url("/s3/1/object?key=open.txt"))
        .header("Authorization", &header)
        .body(b"open data".to_vec())
        .send()
        .await
        .unwrap();

    // Anonymous GET: an honest primary serves public-bucket reads to anyone.
    let resp = server
        .client
        .get(server.url("/s3/1/object?key=open.txt"))
        .send()
        .await
        .unwrap();

    assert_eq!(resp.status(), StatusCode::OK);
    assert_eq!(resp.bytes().await.unwrap().as_ref(), b"open data");
}

#[tokio::test]
async fn public_bucket_put_still_requires_auth() {
    let server = AuthTestServer::public_with_role(Role::Writer).await;

    let resp = server
        .client
        .put(server.url("/s3/1/object?key=nope.txt"))
        .body(b"data".to_vec())
        .send()
        .await
        .unwrap();

    assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
    let body: Value = resp.json().await.unwrap();
    assert_eq!(body["error"], "auth_required");
}

#[tokio::test]
async fn private_bucket_get_requires_auth() {
    let server = AuthTestServer::with_role(Role::Writer).await;

    // Anonymous GET on a private bucket is rejected before any storage work.
    let resp = server
        .client
        .get(server.url("/s3/1/object?key=secret.txt"))
        .send()
        .await
        .unwrap();

    assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
    let body: Value = resp.json().await.unwrap();
    assert_eq!(body["error"], "auth_required");
}

#[tokio::test]
async fn s3_missing_auth_header_returns_401() {
    let server = AuthTestServer::with_role(Role::Admin).await;

    // No Authorization header at all
    let resp = server
        .client
        .put(server.url("/s3/1/object?key=no-auth.txt"))
        .body(b"data".to_vec())
        .send()
        .await
        .unwrap();

    assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
    let body: Value = resp.json().await.unwrap();
    assert_eq!(body["error"], "auth_required");
}

#[tokio::test]
async fn s3_expired_timestamp_returns_401() {
    let server = AuthTestServer::with_role(Role::Admin).await;
    let alice = sr25519::Pair::from_string("//Alice", None).unwrap();

    // Use a timestamp from 10 minutes ago (max_skew is 5 min)
    let old_ts = current_timestamp() - 600;
    let header = make_auth_header(&alice, "PUT", 1, old_ts);

    let resp = server
        .client
        .put(server.url("/s3/1/object?key=old.txt"))
        .header("Authorization", &header)
        .body(b"stale".to_vec())
        .send()
        .await
        .unwrap();

    assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
    let body: Value = resp.json().await.unwrap();
    assert_eq!(body["error"], "auth_required");
}

#[tokio::test]
async fn s3_wrong_signature_returns_401() {
    let server = AuthTestServer::with_role(Role::Admin).await;
    let alice = sr25519::Pair::from_string("//Alice", None).unwrap();

    // Sign for bucket 1, but send to bucket 2 — method/bucket mismatch
    let ts = current_timestamp();
    let header = make_auth_header(&alice, "PUT", 999, ts);

    let resp = server
        .client
        .put(server.url("/s3/1/object?key=wrong-sig.txt"))
        .header("Authorization", &header)
        .body(b"data".to_vec())
        .send()
        .await
        .unwrap();

    // Signature doesn't match bucket_id=1, so verification fails
    assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn s3_admin_can_delete_object() {
    let server = AuthTestServer::with_role(Role::Admin).await;
    let alice = sr25519::Pair::from_string("//Alice", None).unwrap();

    // PUT first
    let ts = current_timestamp();
    let header = make_auth_header(&alice, "PUT", 1, ts);
    server
        .client
        .put(server.url("/s3/1/object?key=delete-me.txt"))
        .header("Authorization", &header)
        .body(b"delete this".to_vec())
        .send()
        .await
        .unwrap();

    // DELETE
    let ts = current_timestamp();
    let header = make_auth_header(&alice, "DELETE", 1, ts);
    let resp = server
        .client
        .delete(server.url("/s3/1/object?key=delete-me.txt"))
        .header("Authorization", &header)
        .send()
        .await
        .unwrap();

    assert_eq!(resp.status(), StatusCode::OK);
}

// ─────────────────────────────────────────────────────────────────────────────
// FS endpoint auth tests
// ─────────────────────────────────────────────────────────────────────────────

#[tokio::test]
async fn fs_writer_can_put_file() {
    let server = AuthTestServer::with_role(Role::Writer).await;
    let alice = sr25519::Pair::from_string("//Alice", None).unwrap();
    let ts = current_timestamp();
    let header = make_auth_header(&alice, "PUT", 1, ts);

    let resp = server
        .client
        .put(server.url("/fs/1/file?path=/hello.txt"))
        .header("Authorization", &header)
        .body(b"fs content".to_vec())
        .send()
        .await
        .unwrap();

    assert_eq!(resp.status(), StatusCode::OK);
}

#[tokio::test]
async fn fs_reader_blocked_from_put() {
    let server = AuthTestServer::with_role(Role::Reader).await;
    let alice = sr25519::Pair::from_string("//Alice", None).unwrap();
    let ts = current_timestamp();
    let header = make_auth_header(&alice, "PUT", 1, ts);

    let resp = server
        .client
        .put(server.url("/fs/1/file?path=/blocked.txt"))
        .header("Authorization", &header)
        .body(b"denied".to_vec())
        .send()
        .await
        .unwrap();

    assert_eq!(resp.status(), StatusCode::FORBIDDEN);
}

#[tokio::test]
async fn fs_reader_can_list_dir() {
    let server = AuthTestServer::with_role(Role::Reader).await;
    let alice = sr25519::Pair::from_string("//Alice", None).unwrap();
    let ts = current_timestamp();
    let header = make_auth_header(&alice, "GET", 1, ts);

    let resp = server
        .client
        .get(server.url("/fs/1/ls?path=/"))
        .header("Authorization", &header)
        .send()
        .await
        .unwrap();

    assert_eq!(resp.status(), StatusCode::OK);
}

#[tokio::test]
async fn fs_reader_blocked_from_mkdir() {
    let server = AuthTestServer::with_role(Role::Reader).await;
    let alice = sr25519::Pair::from_string("//Alice", None).unwrap();
    let ts = current_timestamp();
    let header = make_auth_header(&alice, "POST", 1, ts);

    let resp = server
        .client
        .post(server.url("/fs/1/mkdir?path=/new-dir"))
        .header("Authorization", &header)
        .send()
        .await
        .unwrap();

    assert_eq!(resp.status(), StatusCode::FORBIDDEN);
}

#[tokio::test]
async fn fs_unknown_account_returns_forbidden() {
    // Server only knows Alice as Admin; Bob is not a member at all
    let server = AuthTestServer::with_role(Role::Admin).await;
    let bob = sr25519::Pair::from_string("//Bob", None).unwrap();
    let ts = current_timestamp();
    let header = make_auth_header(&bob, "PUT", 1, ts);

    let resp = server
        .client
        .put(server.url("/fs/1/file?path=/intruder.txt"))
        .header("Authorization", &header)
        .body(b"nope".to_vec())
        .send()
        .await
        .unwrap();

    assert_eq!(resp.status(), StatusCode::FORBIDDEN);
}

#[tokio::test]
async fn s3_list_with_auth() {
    let server = AuthTestServer::with_role(Role::Writer).await;
    let alice = sr25519::Pair::from_string("//Alice", None).unwrap();

    // PUT an object first
    let ts = current_timestamp();
    let header = make_auth_header(&alice, "PUT", 1, ts);
    server
        .client
        .put(server.url("/s3/1/object?key=listed.txt"))
        .header("Authorization", &header)
        .body(b"list me".to_vec())
        .send()
        .await
        .unwrap();

    // LIST
    let ts = current_timestamp();
    let header = make_auth_header(&alice, "GET", 1, ts);
    let resp = server
        .client
        .get(server.url("/s3/1/objects"))
        .header("Authorization", &header)
        .send()
        .await
        .unwrap();

    assert_eq!(resp.status(), StatusCode::OK);
    let body: Value = resp.json().await.unwrap();
    let contents = body["contents"].as_array().unwrap();
    assert!(contents.iter().any(|o| o["key"] == "listed.txt"));
}

#[tokio::test]
async fn s3_head_with_auth() {
    let server = AuthTestServer::with_role(Role::Writer).await;
    let alice = sr25519::Pair::from_string("//Alice", None).unwrap();

    // PUT
    let ts = current_timestamp();
    let header = make_auth_header(&alice, "PUT", 1, ts);
    server
        .client
        .put(server.url("/s3/1/object?key=head-me.txt"))
        .header("Authorization", &header)
        .body(b"head data".to_vec())
        .send()
        .await
        .unwrap();

    // HEAD
    let ts = current_timestamp();
    let header = make_auth_header(&alice, "HEAD", 1, ts);
    let resp = server
        .client
        .head(server.url("/s3/1/object?key=head-me.txt"))
        .header("Authorization", &header)
        .send()
        .await
        .unwrap();

    assert_eq!(resp.status(), StatusCode::OK);
}

// ─────────────────────────────────────────────────────────────────────────────
// Delete endpoint auth tests (admin-only)
// ─────────────────────────────────────────────────────────────────────────────

#[tokio::test]
async fn delete_admin_can_prune() {
    let server = AuthTestServer::with_role(Role::Admin).await;
    let alice = sr25519::Pair::from_string("//Alice", None).unwrap();

    // Create bucket 1 by uploading a file (Admin satisfies the Writer requirement).
    let ts = current_timestamp();
    let header = make_auth_header(&alice, "PUT", 1, ts);
    server
        .client
        .put(server.url("/fs/1/file?path=/data.txt"))
        .header("Authorization", &header)
        .body(b"prune me".to_vec())
        .send()
        .await
        .unwrap();

    // Admin-signed delete succeeds.
    let ts = current_timestamp();
    let header = make_auth_header(&alice, "POST", 1, ts);
    let resp = server
        .client
        .post(server.url("/delete"))
        .header("Authorization", &header)
        .json(&serde_json::json!({ "bucket_id": 1, "new_start_seq": 0 }))
        .send()
        .await
        .unwrap();

    assert_eq!(resp.status(), StatusCode::OK);
    let body: Value = resp.json().await.unwrap();
    assert!(body["provider_signature"].is_string());
}

#[tokio::test]
async fn delete_writer_blocked() {
    let server = AuthTestServer::with_role(Role::Writer).await;
    let alice = sr25519::Pair::from_string("//Alice", None).unwrap();
    let ts = current_timestamp();
    let header = make_auth_header(&alice, "POST", 1, ts);

    let resp = server
        .client
        .post(server.url("/delete"))
        .header("Authorization", &header)
        .json(&serde_json::json!({ "bucket_id": 1, "new_start_seq": 0 }))
        .send()
        .await
        .unwrap();

    assert_eq!(resp.status(), StatusCode::FORBIDDEN);
}

#[tokio::test]
async fn delete_missing_auth_returns_401() {
    let server = AuthTestServer::with_role(Role::Admin).await;

    let resp = server
        .client
        .post(server.url("/delete"))
        .json(&serde_json::json!({ "bucket_id": 1, "new_start_seq": 0 }))
        .send()
        .await
        .unwrap();

    assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
}

// ─────────────────────────────────────────────────────────────────────────────
// L0 node / commit endpoint auth tests
// ─────────────────────────────────────────────────────────────────────────────

/// Build an `UploadNodeRequest` body for `bucket_id` storing `data`.
fn node_body(bucket_id: u64, data: &[u8]) -> Value {
    let hash = storage_primitives::blake2_256(data);
    serde_json::json!({
        "bucket_id": bucket_id,
        "hash": format!("0x{}", hex::encode(hash.as_bytes())),
        "data": BASE64.encode(data),
    })
}

#[tokio::test]
async fn node_writer_can_upload() {
    let server = AuthTestServer::with_role(Role::Writer).await;
    let alice = sr25519::Pair::from_string("//Alice", None).unwrap();

    let ts = current_timestamp();
    let header = make_auth_header(&alice, "PUT", 1, ts);
    let resp = server
        .client
        .put(server.url("/node"))
        .header("Authorization", &header)
        .json(&node_body(1, b"writer node payload"))
        .send()
        .await
        .unwrap();

    assert_eq!(resp.status(), StatusCode::OK);
}

#[tokio::test]
async fn node_reader_blocked() {
    let server = AuthTestServer::with_role(Role::Reader).await;
    let alice = sr25519::Pair::from_string("//Alice", None).unwrap();

    let ts = current_timestamp();
    let header = make_auth_header(&alice, "PUT", 1, ts);
    let resp = server
        .client
        .put(server.url("/node"))
        .header("Authorization", &header)
        .json(&node_body(1, b"reader cannot write"))
        .send()
        .await
        .unwrap();

    assert_eq!(resp.status(), StatusCode::FORBIDDEN);
}

#[tokio::test]
async fn node_missing_auth_returns_401() {
    let server = AuthTestServer::with_role(Role::Writer).await;

    let resp = server
        .client
        .put(server.url("/node"))
        .json(&node_body(1, b"no auth header"))
        .send()
        .await
        .unwrap();

    assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn commit_writer_can_commit() {
    let server = AuthTestServer::with_role(Role::Writer).await;
    let alice = sr25519::Pair::from_string("//Alice", None).unwrap();

    // Upload a node first (Writer), then commit it.
    let data = b"committed chunk";
    let hash_hex = format!(
        "0x{}",
        hex::encode(storage_primitives::blake2_256(data).as_bytes())
    );
    let ts = current_timestamp();
    let header = make_auth_header(&alice, "PUT", 1, ts);
    server
        .client
        .put(server.url("/node"))
        .header("Authorization", &header)
        .json(&node_body(1, data))
        .send()
        .await
        .unwrap();

    let ts = current_timestamp();
    let header = make_auth_header(&alice, "POST", 1, ts);
    let resp = server
        .client
        .post(server.url("/commit"))
        .header("Authorization", &header)
        .json(&serde_json::json!({ "bucket_id": 1, "data_roots": [hash_hex] }))
        .send()
        .await
        .unwrap();

    assert_eq!(resp.status(), StatusCode::OK);
    let body: Value = resp.json().await.unwrap();
    assert!(body["provider_signature"].is_string());
}

#[tokio::test]
async fn commit_reader_blocked() {
    let server = AuthTestServer::with_role(Role::Reader).await;
    let alice = sr25519::Pair::from_string("//Alice", None).unwrap();
    let ts = current_timestamp();
    let header = make_auth_header(&alice, "POST", 1, ts);

    let resp = server
        .client
        .post(server.url("/commit"))
        .header("Authorization", &header)
        .json(&serde_json::json!({ "bucket_id": 1, "data_roots": [] }))
        .send()
        .await
        .unwrap();

    assert_eq!(resp.status(), StatusCode::FORBIDDEN);
}

/// A validly-signed request from an account that is not a member of the bucket
/// must be rejected on the L0 write path — a correct signature only proves
/// identity, not authorization. (The FS path has `fs_unknown_account_*`; this
/// closes the same gap for `/node`.)
#[tokio::test]
async fn node_non_member_returns_forbidden() {
    // Alice is the sole (Admin) member; Dave signs a genuine signature but is
    // not in the member set.
    let server = AuthTestServer::with_role(Role::Admin).await;
    let dave = sr25519::Pair::from_string("//Dave", None).unwrap();

    let ts = current_timestamp();
    let header = make_auth_header(&dave, "PUT", 1, ts);
    let resp = server
        .client
        .put(server.url("/node"))
        .header("Authorization", &header)
        .json(&node_body(1, b"non-member payload"))
        .send()
        .await
        .unwrap();

    assert_eq!(resp.status(), StatusCode::FORBIDDEN);
}
