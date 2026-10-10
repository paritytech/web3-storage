// SPDX-License-Identifier: GPL-3.0-only

//! Integration tests for auth-enabled HTTP endpoints.
//!
//! These tests spin up a real HTTP server whose membership is a fixed member
//! set with configurable roles per test account. All assertions go through
//! real HTTP requests — the auth middleware, signature verification,
//! membership cache lookup, and role check are exercised as a single
//! end-to-end path.
//!
//! Only the Layer 0 write endpoints (`PUT /node`, `POST /commit`,
//! `POST /delete`) check roles. Layer 0 reads are unauthenticated, so there
//! are no Reader-read or bucket-visibility read tests here (#383, #396).

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

/// A resolver whose buckets are all `Public`: writes must still be
/// authenticated.
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
        let (storage, dir) = temp_rocksdb();
        let deps = ProviderDeps {
            storage,
            auth: Arc::new(Authenticator::new(resolver)),
        };
        let state = ProviderState::with_seed(deps, "//Alice").expect("//Alice is valid");
        common::publish_matching_registration(&state);

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
// Delete endpoint auth tests (admin-only)
// ─────────────────────────────────────────────────────────────────────────────

#[tokio::test]
async fn delete_admin_can_prune() {
    let server = AuthTestServer::with_role(Role::Admin).await;
    let alice = sr25519::Pair::from_string("//Alice", None).unwrap();

    // Create bucket 1 by uploading and committing a node (Admin satisfies the
    // Writer requirement).
    upload_and_commit(&server, &alice, b"prune me").await;

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

/// Upload `data` as a single node to bucket 1 and commit it, both signed by
/// `signer`. Returns the `/commit` response.
async fn upload_and_commit(
    server: &AuthTestServer,
    signer: &sr25519::Pair,
    data: &[u8],
) -> reqwest::Response {
    let hash_hex = format!(
        "0x{}",
        hex::encode(storage_primitives::blake2_256(data).as_bytes())
    );
    let header = make_auth_header(signer, "PUT", 1, current_timestamp());
    let resp = server
        .client
        .put(server.url("/node"))
        .header("Authorization", &header)
        .json(&node_body(1, data))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);

    let header = make_auth_header(signer, "POST", 1, current_timestamp());
    server
        .client
        .post(server.url("/commit"))
        .header("Authorization", &header)
        .json(&serde_json::json!({ "bucket_id": 1, "data_roots": [hash_hex] }))
        .send()
        .await
        .unwrap()
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
    let body: Value = resp.json().await.unwrap();
    assert_eq!(body["error"], "insufficient_role");
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
    let body: Value = resp.json().await.unwrap();
    assert_eq!(body["error"], "auth_required");
}

#[tokio::test]
async fn commit_writer_can_commit() {
    let server = AuthTestServer::with_role(Role::Writer).await;
    let alice = sr25519::Pair::from_string("//Alice", None).unwrap();

    let resp = upload_and_commit(&server, &alice, b"committed chunk").await;

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
/// identity, not authorization.
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

#[tokio::test]
async fn node_expired_timestamp_returns_401() {
    let server = AuthTestServer::with_role(Role::Admin).await;
    let alice = sr25519::Pair::from_string("//Alice", None).unwrap();

    // 10 minutes old; the allowed skew is 5 minutes.
    let header = make_auth_header(&alice, "PUT", 1, current_timestamp() - 600);
    let resp = server
        .client
        .put(server.url("/node"))
        .header("Authorization", &header)
        .json(&node_body(1, b"stale"))
        .send()
        .await
        .unwrap();

    assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
    let body: Value = resp.json().await.unwrap();
    assert_eq!(body["error"], "auth_required");
}

#[tokio::test]
async fn node_wrong_signature_returns_401() {
    let server = AuthTestServer::with_role(Role::Admin).await;
    let alice = sr25519::Pair::from_string("//Alice", None).unwrap();

    // Signed for bucket 999 but sent for bucket 1, so verification fails.
    let header = make_auth_header(&alice, "PUT", 999, current_timestamp());
    let resp = server
        .client
        .put(server.url("/node"))
        .header("Authorization", &header)
        .json(&node_body(1, b"wrong signature"))
        .send()
        .await
        .unwrap();

    assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
    let body: Value = resp.json().await.unwrap();
    assert_eq!(body["error"], "auth_required");
}

#[tokio::test]
async fn public_bucket_node_upload_still_requires_auth() {
    let server = AuthTestServer::public_with_role(Role::Writer).await;

    let resp = server
        .client
        .put(server.url("/node"))
        .json(&node_body(1, b"anonymous write"))
        .send()
        .await
        .unwrap();

    assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
    let body: Value = resp.json().await.unwrap();
    assert_eq!(body["error"], "auth_required");
}
