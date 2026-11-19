// SPDX-License-Identifier: Apache-2.0

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
use common::{current_timestamp, make_auth_header, test_member_pair};
use provider_auth::{
    AgreementRole, Authenticator, BucketAccess, Invalidation, LocalBucketAgreement,
    MembershipError, MembershipInvalidations, MembershipResolver, StaticMembershipResolver,
};
use provider_http::{ChainState, ProviderDeps, ProviderState};
use provider_storage::{temp_rocksdb, ChunkTreeNode, StorageBackend};
use reqwest::Client;
use serde_json::Value;
use sp_core::{sr25519, Pair};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;
use storage_primitives::{BucketId, Commitment, Role, Visibility};

type AccountId32 = sp_core::crypto::AccountId32;

// ─────────────────────────────────────────────────────────────────────────────
// Test server
// ─────────────────────────────────────────────────────────────────────────────

struct AuthTestServer {
    addr: std::net::SocketAddr,
    client: Client,
    /// The server's storage, for seeding data the way replica sync does.
    storage: Arc<dyn StorageBackend>,
    /// The node's view of the chain, for moving its anchor block.
    chain_state: Arc<ChainState>,
    /// This server's scratch directory; dropped with the server.
    _dir: tempfile::TempDir,
}

impl AuthTestServer {
    /// Start a server with auth enabled and Alice as the given role.
    /// Buckets resolve as `Private` (the static resolver's default).
    async fn with_role(alice_role: Role) -> Self {
        let alice_account = common::test_member_account();
        Self::with_resolver(StaticMembershipResolver::private(vec![(
            alice_account,
            alice_role,
        )
            .into()]))
        .await
    }

    /// Same, but every bucket resolves as `Public`.
    async fn public_with_role(alice_role: Role) -> Self {
        let alice_account = common::test_member_account();
        Self::with_resolver(StaticMembershipResolver::new(BucketAccess {
            members: vec![(alice_account, alice_role).into()],
            visibility: Visibility::Public,
            local_agreement: Some(LocalBucketAgreement {
                role: AgreementRole::Primary,
                max_bytes: u64::MAX,
                expires_at: u32::MAX,
            }),
        }))
        .await
    }

    async fn with_resolver(resolver: impl MembershipResolver + 'static) -> Self {
        Self::with_authenticator(Authenticator::new(resolver)).await
    }

    async fn with_authenticator(authenticator: Authenticator) -> Self {
        // The 300s skew keeps the default the `*_expired_timestamp` tests assume.
        let (storage, dir) = temp_rocksdb();
        let deps = ProviderDeps {
            storage: storage.clone(),
            auth: Arc::new(authenticator),
        };
        let state = ProviderState::with_seed(deps, "//Alice").expect("//Alice is valid");
        common::publish_matching_registration(&state);
        common::publish_anchor_block(&state);
        let chain_state = state.chain_state().clone();

        let (addr, _) = common::serve(state).await;

        Self {
            addr,
            client: Client::new(),
            storage,
            chain_state,
            _dir: dir,
        }
    }

    fn set_anchor_block(&self, block: u32) {
        self.chain_state
            .current_anchor_block
            .store(block, Ordering::Relaxed);
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
    let alice = test_member_pair();

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
    let alice = test_member_pair();
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
// Node and commit endpoint auth tests
// ─────────────────────────────────────────────────────────────────────────────

/// Build an `UploadNodeRequest` body for `bucket_id` storing `data`.
fn node_body(bucket_id: u64, data: &[u8]) -> Value {
    let hash = storage_primitives::hash_leaf(data);
    serde_json::json!({
        "bucket_id": bucket_id,
        "hash": format!("{hash:#x}"),
        "data": BASE64.encode(data),
    })
}

/// `PUT /node` storing `data` in bucket 1, with `auth` as the
/// `Authorization` header if given.
async fn put_node(server: &AuthTestServer, auth: Option<String>, data: &[u8]) -> reqwest::Response {
    let mut req = server
        .client
        .put(server.url("/node"))
        .json(&node_body(1, data));
    if let Some(header) = auth {
        req = req.header("Authorization", header);
    }
    req.send().await.unwrap()
}

/// Upload `data` as a single node to bucket 1 and commit it, both signed by
/// `signer` (which must hold Writer). Returns the node hash hex and the
/// commitment after the commit.
async fn upload_and_commit(
    server: &AuthTestServer,
    signer: &sr25519::Pair,
    data: &[u8],
) -> (String, Commitment) {
    let hash_hex = format!("{:#x}", storage_primitives::hash_leaf(data));
    let header = make_auth_header(signer, "PUT", 1, current_timestamp());
    assert_eq!(
        put_node(server, Some(header), data).await.status(),
        StatusCode::OK
    );

    let header = make_auth_header(signer, "POST", 1, current_timestamp());
    let resp = server
        .client
        .post(server.url("/commit"))
        .header("Authorization", &header)
        .json(&serde_json::json!({ "bucket_id": 1, "data_roots": [hash_hex] }))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK, "commit must succeed");
    let body: Value = resp.json().await.unwrap();
    assert!(body["provider_signature"].is_string());
    let commitment = Commitment {
        mmr_root: body["mmr_root"].as_str().unwrap().parse().unwrap(),
        start_seq: body["start_seq"].as_u64().unwrap(),
        leaf_count: body["leaf_count"].as_u64().unwrap(),
    };
    (hash_hex, commitment)
}

#[tokio::test]
async fn node_writer_can_upload() {
    let server = AuthTestServer::with_role(Role::Writer).await;
    let alice = test_member_pair();

    let header = make_auth_header(&alice, "PUT", 1, current_timestamp());
    let resp = put_node(&server, Some(header), b"writer node payload").await;

    assert_eq!(resp.status(), StatusCode::OK);
}

/// A member of a bucket this node has no agreement on cannot store anything.
#[tokio::test]
async fn upload_without_agreement_is_rejected() {
    let alice = sr25519::Pair::from_string("//Alice", None).unwrap();
    let server = AuthTestServer::with_resolver(
        StaticMembershipResolver::private(vec![
            (AccountId32::new(alice.public().0), Role::Admin).into()
        ])
        .without_agreement(),
    )
    .await;

    let header = make_auth_header(&alice, "PUT", 1, current_timestamp());
    let resp = put_node(&server, Some(header), b"no agreement").await;
    assert_eq!(resp.status(), StatusCode::FORBIDDEN);
    let body: Value = resp.json().await.unwrap();
    assert_eq!(body["error"], "no_agreement");
    assert_eq!(
        server
            .client
            .get(server.url("/stats"))
            .send()
            .await
            .unwrap()
            .json::<Value>()
            .await
            .unwrap()["total_bytes"],
        0,
        "nothing was written"
    );
}

/// A resolver whose answer a test replaces to stand in for a chain change,
/// counting how often the chain would be read.
struct SwitchableResolver {
    access: Arc<Mutex<BucketAccess>>,
    calls: Arc<AtomicUsize>,
}

#[async_trait::async_trait]
impl MembershipResolver for SwitchableResolver {
    async fn fetch_access(&self, _bucket_id: BucketId) -> Result<BucketAccess, MembershipError> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        Ok(self.access.lock().unwrap().clone())
    }
}

/// Buckets a test marks as changed on chain. The membership cache drains them
/// like the node's block-event feed, which turns each agreement and
/// membership event into its bucket id.
#[derive(Clone, Default)]
struct ChangedBuckets(Arc<Mutex<Vec<BucketId>>>);

impl ChangedBuckets {
    fn mark(&self, bucket_id: BucketId) {
        self.0.lock().unwrap().push(bucket_id);
    }
}

impl MembershipInvalidations for ChangedBuckets {
    fn drain(&self) -> Invalidation {
        let buckets = std::mem::take(&mut *self.0.lock().unwrap());
        if buckets.is_empty() {
            Invalidation::None
        } else {
            Invalidation::Buckets(buckets)
        }
    }
}

/// A server over a [`SwitchableResolver`] that starts with `access`, with the
/// membership cache fed by the returned [`ChangedBuckets`]. The 300s TTL is
/// long enough that only an invalidation can cause a re-resolve within a test.
async fn switchable_server(
    access: BucketAccess,
) -> (
    AuthTestServer,
    Arc<Mutex<BucketAccess>>,
    Arc<AtomicUsize>,
    ChangedBuckets,
) {
    let access = Arc::new(Mutex::new(access));
    let calls = Arc::new(AtomicUsize::new(0));
    let changed = ChangedBuckets::default();
    let auth = Authenticator::new(SwitchableResolver {
        access: access.clone(),
        calls: calls.clone(),
    })
    .with_ttl(Duration::from_secs(300))
    .with_invalidations(changed.clone());
    let server = AuthTestServer::with_authenticator(auth).await;
    (server, access, calls, changed)
}

/// The first upload to a bucket reads the chain once: the role check and the
/// quota lookup share one cached entry.
#[tokio::test]
async fn first_upload_reads_the_chain_once() {
    let alice = sr25519::Pair::from_string("//Alice", None).unwrap();
    let (server, _, calls, _) = switchable_server(
        StaticMembershipResolver::private(vec![
            (AccountId32::new(alice.public().0), Role::Writer).into()
        ])
        .access,
    )
    .await;

    let header = make_auth_header(&alice, "PUT", 1, current_timestamp());
    let resp = put_node(&server, Some(header), b"one chain read").await;

    assert_eq!(resp.status(), StatusCode::OK);
    assert_eq!(calls.load(Ordering::SeqCst), 1);
}

/// This node's agreement is part of the cached access entry, so the
/// `StorageAgreementEstablished` event (decoded to `BucketMembershipChanged`)
/// must drop the entry: an upload refused for lack of an agreement succeeds
/// once the agreement exists on chain, without waiting out the TTL.
#[tokio::test]
async fn new_agreement_is_honoured_after_the_block_event() {
    let alice = sr25519::Pair::from_string("//Alice", None).unwrap();
    let member = || {
        StaticMembershipResolver::private(vec![
            (AccountId32::new(alice.public().0), Role::Writer).into()
        ])
    };
    let (server, access, calls, changed) =
        switchable_server(member().without_agreement().access).await;
    let put = |data: &'static [u8]| {
        put_node(
            &server,
            Some(make_auth_header(&alice, "PUT", 1, current_timestamp())),
            data,
        )
    };

    assert_eq!(put(&[1u8; 10]).await.status(), StatusCode::FORBIDDEN);
    assert_eq!(put(&[1u8; 10]).await.status(), StatusCode::FORBIDDEN);
    assert_eq!(calls.load(Ordering::SeqCst), 1, "the refusal is cached");

    *access.lock().unwrap() = member().with_max_bytes(10).access;
    changed.mark(1);

    assert_eq!(put(&[1u8; 10]).await.status(), StatusCode::OK);
    assert_eq!(calls.load(Ordering::SeqCst), 2);
    // The local bucket took the new agreement's quota: a second node passes it.
    assert_eq!(
        put(&[2u8; 10]).await.status(),
        StatusCode::INSUFFICIENT_STORAGE
    );
}

/// `AgreementEnded` (decoded to `BucketMembershipChanged`) must drop the
/// entry: once the agreement is gone from chain, a bucket the node holds no
/// data for is refused instead of being created from the cached agreement.
#[tokio::test]
async fn ended_agreement_is_refused_after_the_block_event() {
    let alice = sr25519::Pair::from_string("//Alice", None).unwrap();
    let member = || {
        StaticMembershipResolver::private(vec![
            (AccountId32::new(alice.public().0), Role::Admin).into()
        ])
    };
    let (server, access, calls, changed) = switchable_server(member().access).await;

    // A bucket-bound read caches the entry without creating the local bucket.
    let header = make_auth_header(&alice, "GET", 1, current_timestamp());
    let resp = server
        .client
        .get(server.url("/commitment?bucket_id=1"))
        .header("Authorization", &header)
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::NOT_FOUND);
    assert_eq!(calls.load(Ordering::SeqCst), 1);

    *access.lock().unwrap() = member().without_agreement().access;
    changed.mark(1);

    let header = make_auth_header(&alice, "PUT", 1, current_timestamp());
    let resp = put_node(&server, Some(header), b"after the agreement ended").await;
    assert_eq!(resp.status(), StatusCode::FORBIDDEN);
    let body: Value = resp.json().await.unwrap();
    assert_eq!(body["error"], "no_agreement");
    assert_eq!(calls.load(Ordering::SeqCst), 2);
    assert!(server.storage.get_bucket(1).unwrap().is_none());
}

/// `AgreementToppedUp` (decoded to `BucketMembershipChanged`) must drop the
/// entry, so a local bucket created after the top-up takes the new quota.
#[tokio::test]
async fn topped_up_quota_applies_after_the_block_event() {
    let alice = sr25519::Pair::from_string("//Alice", None).unwrap();
    let member = || {
        StaticMembershipResolver::private(vec![
            (AccountId32::new(alice.public().0), Role::Admin).into()
        ])
    };
    let (server, access, calls, changed) =
        switchable_server(member().with_max_bytes(10).access).await;

    let header = make_auth_header(&alice, "GET", 1, current_timestamp());
    let resp = server
        .client
        .get(server.url("/commitment?bucket_id=1"))
        .header("Authorization", &header)
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::NOT_FOUND);
    assert_eq!(calls.load(Ordering::SeqCst), 1);

    *access.lock().unwrap() = member().with_max_bytes(20).access;
    changed.mark(1);

    // 20 bytes fit the topped-up quota and not the cached one.
    let header = make_auth_header(&alice, "PUT", 1, current_timestamp());
    assert_eq!(
        put_node(&server, Some(header), &[1u8; 20]).await.status(),
        StatusCode::OK
    );
    assert_eq!(calls.load(Ordering::SeqCst), 2);
}

/// A top-up raises the quota of an existing bucket.
#[tokio::test]
async fn topped_up_quota_applies_to_an_existing_bucket() {
    let alice = sr25519::Pair::from_string("//Alice", None).unwrap();
    let member = || {
        StaticMembershipResolver::private(vec![
            (AccountId32::new(alice.public().0), Role::Writer).into()
        ])
    };
    let (server, access, calls, changed) =
        switchable_server(member().with_max_bytes(10).access).await;
    let put = |data: &'static [u8]| {
        put_node(
            &server,
            Some(make_auth_header(&alice, "PUT", 1, current_timestamp())),
            data,
        )
    };

    assert_eq!(put(&[1u8; 10]).await.status(), StatusCode::OK);
    assert_eq!(
        put(&[2u8; 10]).await.status(),
        StatusCode::INSUFFICIENT_STORAGE
    );

    *access.lock().unwrap() = member().with_max_bytes(20).access;
    changed.mark(1);

    assert_eq!(put(&[2u8; 10]).await.status(), StatusCode::OK);
    assert_eq!(calls.load(Ordering::SeqCst), 2);
    assert_eq!(
        put(&[3u8; 10]).await.status(),
        StatusCode::INSUFFICIENT_STORAGE
    );
}

/// An upload to an existing bucket is rejected once the agreement is gone.
#[tokio::test]
async fn upload_to_existing_bucket_without_agreement_is_rejected() {
    let alice = sr25519::Pair::from_string("//Alice", None).unwrap();
    let member = || {
        StaticMembershipResolver::private(vec![
            (AccountId32::new(alice.public().0), Role::Writer).into()
        ])
    };
    let (server, access, _, changed) = switchable_server(member().with_max_bytes(100).access).await;
    let put = |data: &'static [u8]| {
        put_node(
            &server,
            Some(make_auth_header(&alice, "PUT", 1, current_timestamp())),
            data,
        )
    };

    assert_eq!(put(&[1u8; 10]).await.status(), StatusCode::OK);
    let stats_before = server.storage.stats();

    *access.lock().unwrap() = member().without_agreement().access;
    changed.mark(1);

    let resp = put(&[2u8; 10]).await;
    assert_eq!(resp.status(), StatusCode::FORBIDDEN);
    let body: Value = resp.json().await.unwrap();
    assert_eq!(body["error"], "no_agreement");
    assert_eq!(server.storage.stats(), stats_before);
}

/// The agreement's `max_bytes` bounds what the bucket stores: a node that
/// would pass it is answered `507` and nothing is written.
#[tokio::test]
async fn upload_beyond_agreement_quota_is_rejected() {
    let alice = sr25519::Pair::from_string("//Alice", None).unwrap();
    let server = AuthTestServer::with_resolver(
        StaticMembershipResolver::private(vec![
            (AccountId32::new(alice.public().0), Role::Writer).into()
        ])
        .with_max_bytes(10),
    )
    .await;
    let header = make_auth_header(&alice, "PUT", 1, current_timestamp());

    assert_eq!(
        put_node(&server, Some(header.clone()), &[1u8; 10])
            .await
            .status(),
        StatusCode::OK
    );

    let resp = put_node(&server, Some(header), &[2u8; 10]).await;
    assert_eq!(resp.status(), StatusCode::INSUFFICIENT_STORAGE);
    let body: Value = resp.json().await.unwrap();
    assert_eq!(body["error"], "quota_exceeded");
}

#[tokio::test]
async fn node_reader_blocked() {
    let server = AuthTestServer::with_role(Role::Reader).await;
    let alice = test_member_pair();

    let header = make_auth_header(&alice, "PUT", 1, current_timestamp());
    let resp = put_node(&server, Some(header), b"reader cannot write").await;

    assert_eq!(resp.status(), StatusCode::FORBIDDEN);
    let body: Value = resp.json().await.unwrap();
    assert_eq!(body["error"], "insufficient_role");
}

#[tokio::test]
async fn node_missing_auth_returns_401() {
    let server = AuthTestServer::with_role(Role::Writer).await;

    let resp = put_node(&server, None, b"no auth header").await;

    assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
    let body: Value = resp.json().await.unwrap();
    assert_eq!(body["error"], "auth_required");
}

#[tokio::test]
async fn commit_writer_can_commit() {
    let server = AuthTestServer::with_role(Role::Writer).await;
    let alice = test_member_pair();

    upload_and_commit(&server, &alice, b"committed chunk").await;
}

#[tokio::test]
async fn commit_reader_blocked() {
    let server = AuthTestServer::with_role(Role::Reader).await;
    let alice = test_member_pair();
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
/// must be rejected on the write path — a correct signature only proves
/// identity, not authorization.
#[tokio::test]
async fn node_non_member_returns_forbidden() {
    // Alice is the sole (Admin) member; Dave signs a genuine signature but is
    // not in the member set.
    let server = AuthTestServer::with_role(Role::Admin).await;
    let dave = sr25519::Pair::from_string("//Dave", None).unwrap();

    let header = make_auth_header(&dave, "PUT", 1, current_timestamp());
    let resp = put_node(&server, Some(header), b"non-member payload").await;

    assert_eq!(resp.status(), StatusCode::FORBIDDEN);
}

#[tokio::test]
async fn node_expired_timestamp_returns_401() {
    let server = AuthTestServer::with_role(Role::Admin).await;
    let alice = test_member_pair();

    // 10 minutes old; the allowed skew is 5 minutes.
    let header = make_auth_header(&alice, "PUT", 1, current_timestamp() - 600);
    let resp = put_node(&server, Some(header), b"stale").await;

    assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
    let body: Value = resp.json().await.unwrap();
    assert_eq!(body["error"], "auth_required");
}

#[tokio::test]
async fn node_wrong_signature_returns_401() {
    let server = AuthTestServer::with_role(Role::Admin).await;
    let alice = test_member_pair();

    // Signed for bucket 999 but sent for bucket 1, so verification fails.
    let header = make_auth_header(&alice, "PUT", 999, current_timestamp());
    let resp = put_node(&server, Some(header), b"wrong signature").await;

    assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
    let body: Value = resp.json().await.unwrap();
    assert_eq!(body["error"], "auth_required");
}

#[tokio::test]
async fn public_bucket_node_upload_still_requires_auth() {
    let server = AuthTestServer::public_with_role(Role::Writer).await;

    let resp = put_node(&server, None, b"anonymous write").await;

    assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
    let body: Value = resp.json().await.unwrap();
    assert_eq!(body["error"], "auth_required");
}

// ─────────────────────────────────────────────────────────────────────────────
// Read gates: bucket-bound reads are Reader-gated; hash-keyed
// reads stay open as capability lookups.
// ─────────────────────────────────────────────────────────────────────────────

/// `/mmr_proof` for leaf 0 of `commitment` in bucket 1.
fn mmr_proof_path(commitment: &Commitment) -> String {
    format!(
        "/mmr_proof?bucket_id=1&leaf_index=0&mmr_root={:#x}&start_seq={}&leaf_count={}",
        commitment.mmr_root, commitment.start_seq, commitment.leaf_count
    )
}

/// The bucket-bound `GET` reads gated at Reader, with `/mmr_proof` for
/// `commitment`. Add a newly gated endpoint here; both tests below walk this
/// list.
fn l0_gated_gets(commitment: &Commitment) -> Vec<String> {
    vec![
        "/mmr_peaks?bucket_id=1".into(),
        "/commitment?bucket_id=1".into(),
        "/checkpoint-signature?bucket_id=1".into(),
        mmr_proof_path(commitment),
        "/replica/historical_roots?bucket_id=1".into(),
        "/replica/sync_status?bucket_id=1".into(),
    ]
}

/// Every bucket-bound read answers 401 to an anonymous request on a
/// private bucket.
#[tokio::test]
async fn l0_reads_require_auth_on_private_bucket() {
    let server = AuthTestServer::with_role(Role::Admin).await;

    for path in l0_gated_gets(&Commitment::default()) {
        let resp = server.client.get(server.url(&path)).send().await.unwrap();
        assert_eq!(
            resp.status(),
            StatusCode::UNAUTHORIZED,
            "anonymous GET {path} must be refused"
        );
    }

    let resp = server
        .client
        .post(server.url("/exists"))
        .json(&serde_json::json!({ "bucket_id": 1, "hashes": [] }))
        .send()
        .await
        .unwrap();
    assert_eq!(
        resp.status(),
        StatusCode::UNAUTHORIZED,
        "anonymous POST /exists must be refused"
    );
}

/// A member holding Reader (here: Admin, which subsumes it) passes the same
/// gates with a signed header.
#[tokio::test]
async fn l0_member_reads_private_bucket() {
    let server = AuthTestServer::with_role(Role::Admin).await;
    let alice = test_member_pair();
    let (_, commitment) = upload_and_commit(&server, &alice, b"read-gate-test-data").await;

    for path in l0_gated_gets(&commitment) {
        let header = make_auth_header(&alice, "GET", 1, current_timestamp());
        let resp = server
            .client
            .get(server.url(&path))
            .header("Authorization", &header)
            .send()
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK, "signed GET {path} must pass");
    }

    let header = make_auth_header(&alice, "POST", 1, current_timestamp());
    let resp = server
        .client
        .post(server.url("/exists"))
        .header("Authorization", &header)
        .json(&serde_json::json!({ "bucket_id": 1, "hashes": [] }))
        .send()
        .await
        .unwrap();
    assert_eq!(
        resp.status(),
        StatusCode::OK,
        "signed POST /exists must pass"
    );
}

/// Public buckets keep serving the bucket-bound reads anonymously.
#[tokio::test]
async fn l0_reads_open_on_public_bucket() {
    let server = AuthTestServer::public_with_role(Role::Writer).await;
    let alice = test_member_pair();
    upload_and_commit(&server, &alice, b"read-gate-test-data").await;

    let resp = server
        .client
        .get(server.url("/mmr_peaks?bucket_id=1"))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
}

/// Hash-keyed reads (`GET /node`, `POST /fetch_nodes`) are capability
/// lookups: they stay open even on a private bucket, because a blake2-256
/// hash is unguessable and the enumeration surfaces above are gated. See
/// "Bucket Visibility & Access" in the design doc.
#[tokio::test]
async fn hash_keyed_reads_stay_open_on_private_bucket() {
    let server = AuthTestServer::with_role(Role::Admin).await;
    let alice = test_member_pair();
    let (hash_hex, _) = upload_and_commit(&server, &alice, b"read-gate-test-data").await;

    let resp = server
        .client
        .get(server.url(&format!("/node?bucket_id=1&hash={hash_hex}")))
        .send()
        .await
        .unwrap();
    assert_eq!(
        resp.status(),
        StatusCode::OK,
        "GET /node is a capability read"
    );

    let resp = server
        .client
        .post(server.url("/fetch_nodes"))
        .json(&serde_json::json!({ "bucket_id": 1, "hashes": [hash_hex] }))
        .send()
        .await
        .unwrap();
    assert_eq!(
        resp.status(),
        StatusCode::OK,
        "POST /fetch_nodes is a capability read"
    );
}

/// The node's own provider account passes Reader gates on buckets it is not
/// a member of — the challenged provider's tooling (dashboard) fetches
/// `/mmr_proof` this way, since provider accounts are never members.
#[tokio::test]
async fn provider_account_reads_private_bucket_proofs() {
    // Membership: Bob (Admin) only. The server's identity is //Alice (the
    // harness seeds ProviderState with it), and the authenticator is told so.
    let bob = sr25519::Pair::from_string("//Bob", None).unwrap();
    let alice = sr25519::Pair::from_string("//Alice", None).unwrap();
    let server = AuthTestServer::with_authenticator(
        Authenticator::new(StaticMembershipResolver::private(vec![(
            AccountId32::new(bob.public().0),
            Role::Admin,
        )
            .into()]))
        .with_provider_account(AccountId32::new(alice.public().0)),
    )
    .await;
    let (_, commitment) = upload_and_commit(&server, &bob, b"read-gate-test-data").await;

    // Alice is not a member, but is the node's own account: Reader passes.
    let header = make_auth_header(&alice, "GET", 1, current_timestamp());
    let resp = server
        .client
        .get(server.url(&mmr_proof_path(&commitment)))
        .header("Authorization", &header)
        .send()
        .await
        .unwrap();
    assert_eq!(
        resp.status(),
        StatusCode::OK,
        "self-account Reader must pass"
    );

    // Self-auth stops at Reader: Alice still cannot write.
    let header = make_auth_header(&alice, "PUT", 1, current_timestamp());
    let resp = put_node(&server, Some(header), b"self-cannot-write").await;
    assert_eq!(resp.status(), StatusCode::FORBIDDEN);
}

/// A private bucket this node holds as a *replica* serves reads to everyone
/// — the design's "replicas always serve everyone" rule (visibility gates
/// primaries only), and what lets replicas seed further replicas.
#[tokio::test]
async fn replica_served_private_bucket_reads_anonymously() {
    let alice = test_member_pair();
    let server = AuthTestServer::with_resolver(
        StaticMembershipResolver::private(vec![
            (AccountId32::new(alice.public().0), Role::Writer).into()
        ])
        .with_serves_as_replica(),
    )
    .await;
    seed_bucket_1(&server, b"read-gate-test-data");

    let resp = server
        .client
        .get(server.url("/mmr_peaks?bucket_id=1"))
        .send()
        .await
        .unwrap();
    assert_eq!(
        resp.status(),
        StatusCode::OK,
        "a replica serves private-bucket reads to anyone"
    );
}

/// Store `data` as a node of bucket 1 and commit it directly in storage.
fn seed_bucket_1(server: &AuthTestServer, data: &[u8]) {
    let hash = storage_primitives::hash_leaf(data);
    server.storage.init_bucket(1, u64::MAX).unwrap();
    server
        .storage
        .store_node(1, hash, ChunkTreeNode::Chunk(data.to_vec()))
        .unwrap();
    server.storage.commit(1, vec![hash]).unwrap();
}

/// A server whose node holds every bucket as a replica, with Alice as `role`.
async fn replica_server(role: Role) -> AuthTestServer {
    AuthTestServer::with_resolver(
        StaticMembershipResolver::private(vec![(
            AccountId32::new(test_member_pair().public().0),
            role,
        )
            .into()])
        .with_serves_as_replica(),
    )
    .await
}

/// The response is a 403 `not_primary`.
async fn assert_not_primary(resp: reqwest::Response) {
    assert_eq!(resp.status(), StatusCode::FORBIDDEN);
    let body: Value = resp.json().await.unwrap();
    assert_eq!(body["error"], "not_primary");
}

/// A bucket writer cannot push a node to, or commit on, a node that holds
/// the bucket as a replica.
#[tokio::test]
async fn replica_rejects_node_upload_and_commit() {
    let alice = test_member_pair();
    let server = replica_server(Role::Writer).await;
    let data = b"pushed to a replica";

    let header = make_auth_header(&alice, "PUT", 1, current_timestamp());
    assert_not_primary(put_node(&server, Some(header), data).await).await;

    let header = make_auth_header(&alice, "POST", 1, current_timestamp());
    let hash_hex = format!("{:#x}", storage_primitives::hash_leaf(data));
    let resp = server
        .client
        .post(server.url("/commit"))
        .header("Authorization", &header)
        .json(&serde_json::json!({ "bucket_id": 1, "data_roots": [hash_hex] }))
        .send()
        .await
        .unwrap();
    assert_not_primary(resp).await;

    assert!(
        server.storage.get_bucket(1).unwrap().is_none(),
        "a rejected write must not create the bucket"
    );
}

/// A bucket admin cannot prune a bucket on a node that holds it as a replica.
#[tokio::test]
async fn replica_rejects_delete() {
    let alice = test_member_pair();
    let server = replica_server(Role::Admin).await;
    seed_bucket_1(&server, b"replicated data");

    let header = make_auth_header(&alice, "POST", 1, current_timestamp());
    let resp = server
        .client
        .post(server.url("/delete"))
        .header("Authorization", &header)
        .json(&serde_json::json!({ "bucket_id": 1, "new_start_seq": 0 }))
        .send()
        .await
        .unwrap();
    assert_not_primary(resp).await;
}

// ─────────────────────────────────────────────────────────────────────────────
// Agreement gate
// ─────────────────────────────────────────────────────────────────────────────

/// A request to bucket 1 for each bucket-scoped read endpoint, over the
/// single node `data_root` committed to it.
fn bucket_reads(
    data_root: &str,
    commitment: &Commitment,
) -> Vec<(&'static str, String, Option<Value>)> {
    let hashes = serde_json::json!({ "bucket_id": 1, "hashes": [data_root] });
    vec![
        ("GET", format!("/node?bucket_id=1&hash={data_root}"), None),
        (
            "GET",
            format!("/read?bucket_id=1&data_root={data_root}&offset=0&length=1"),
            None,
        ),
        (
            "GET",
            format!("/chunk_proof?bucket_id=1&data_root={data_root}&chunk_index=0"),
            None,
        ),
        ("POST", "/fetch_nodes".into(), Some(hashes.clone())),
        ("POST", "/exists".into(), Some(hashes)),
        ("GET", "/commitment?bucket_id=1".into(), None),
        ("GET", "/checkpoint-signature?bucket_id=1".into(), None),
        (
            "GET",
            format!(
                "/mmr_proof?bucket_id=1&leaf_index=0&mmr_root={:#x}&start_seq={}&leaf_count={}",
                commitment.mmr_root, commitment.start_seq, commitment.leaf_count
            ),
            None,
        ),
        ("GET", "/mmr_peaks?bucket_id=1".into(), None),
        ("GET", "/replica/historical_roots?bucket_id=1".into(), None),
        ("GET", "/replica/sync_status?bucket_id=1".into(), None),
    ]
}

/// A request to bucket 1 for each endpoint that changes it.
fn bucket_writes(data_root: &str) -> Vec<(&'static str, String, Option<Value>)> {
    vec![
        ("PUT", "/node".into(), Some(node_body(1, b"another node"))),
        (
            "POST",
            "/commit".into(),
            Some(serde_json::json!({ "bucket_id": 1, "data_roots": [data_root] })),
        ),
        (
            "POST",
            "/delete".into(),
            Some(serde_json::json!({ "bucket_id": 1, "new_start_seq": 0 })),
        ),
    ]
}

/// Send `(method, path, body)` to bucket 1, signed by `signer`.
async fn send_to_bucket(
    server: &AuthTestServer,
    signer: &sr25519::Pair,
    (method, path, body): &(&'static str, String, Option<Value>),
) -> reqwest::Response {
    let header = make_auth_header(signer, method, 1, current_timestamp());
    let url = server.url(path);
    let request = match *method {
        "GET" => server.client.get(url),
        "PUT" => server.client.put(url),
        _ => server.client.post(url),
    }
    .header("Authorization", header);
    match body {
        Some(body) => request.json(body),
        None => request,
    }
    .send()
    .await
    .unwrap()
}

async fn assert_error(response: reqwest::Response, status: StatusCode, error: &str, what: &str) {
    assert_eq!(response.status(), status, "{what}");
    let body: Value = response.json().await.unwrap();
    assert_eq!(body["error"], error, "{what}");
}

/// Once the node's agreement is gone from chain, every bucket-scoped request
/// is refused, including the hash-keyed reads that carry no authorization,
/// on a bucket that still holds data. The `AgreementEnded` event drops the
/// cached entry, so this does not wait for the TTL.
#[tokio::test]
async fn ended_agreement_rejects_every_request_to_a_bucket_with_data() {
    let alice = sr25519::Pair::from_string("//Alice", None).unwrap();
    let member = || {
        StaticMembershipResolver::private(vec![
            (AccountId32::new(alice.public().0), Role::Admin).into()
        ])
    };
    let (server, access, _, changed) = switchable_server(member().access).await;
    let (data_root, commitment) = upload_and_commit(&server, &alice, b"kept after the end").await;
    let requests: Vec<_> = bucket_reads(&data_root, &commitment)
        .into_iter()
        .chain(bucket_writes(&data_root))
        .collect();

    for request in &requests {
        let response = send_to_bucket(&server, &alice, request).await;
        let ok = response.status().is_success();
        assert!(ok, "{} {} before the end", request.0, request.1);
    }

    *access.lock().unwrap() = member().without_agreement().access;
    changed.mark(1);

    for request in &requests {
        let response = send_to_bucket(&server, &alice, request).await;
        let what = format!("{} {} after the end", request.0, request.1);
        assert_error(response, StatusCode::FORBIDDEN, "no_agreement", &what).await;
    }
}

/// From the agreement's `expires_at`, writes are refused and reads keep
/// working.
#[tokio::test]
async fn expired_agreement_rejects_writes_and_serves_reads() {
    let alice = sr25519::Pair::from_string("//Alice", None).unwrap();
    let server = AuthTestServer::with_resolver(
        StaticMembershipResolver::private(vec![
            (AccountId32::new(alice.public().0), Role::Admin).into()
        ])
        .with_expires_at(100),
    )
    .await;
    server.set_anchor_block(99);
    let (data_root, commitment) = upload_and_commit(&server, &alice, b"stored before expiry").await;

    server.set_anchor_block(100);

    for request in &bucket_writes(&data_root) {
        let response = send_to_bucket(&server, &alice, request).await;
        let what = format!("{} {} at expires_at", request.0, request.1);
        assert_error(response, StatusCode::FORBIDDEN, "agreement_expired", &what).await;
    }
    for request in &bucket_reads(&data_root, &commitment) {
        let response = send_to_bucket(&server, &alice, request).await;
        let status = response.status();
        assert_eq!(
            status,
            StatusCode::OK,
            "{} {} at expires_at",
            request.0,
            request.1
        );
    }
}

/// Until the node has read the anchor block from the chain it cannot tell
/// whether an agreement has expired, so writes answer
/// `503 chain_state_not_ready` instead of passing, and reads keep working.
#[tokio::test]
async fn writes_wait_until_the_anchor_block_is_known() {
    let alice = sr25519::Pair::from_string("//Alice", None).unwrap();
    let server = AuthTestServer::with_resolver(StaticMembershipResolver::private(vec![(
        AccountId32::new(alice.public().0),
        Role::Admin,
    )
        .into()]))
    .await;
    let (data_root, commitment) = upload_and_commit(&server, &alice, b"stored while synced").await;

    server.set_anchor_block(0);

    for request in &bucket_writes(&data_root) {
        let response = send_to_bucket(&server, &alice, request).await;
        let what = format!("{} {} with no anchor block", request.0, request.1);
        assert_error(
            response,
            StatusCode::SERVICE_UNAVAILABLE,
            "chain_state_not_ready",
            &what,
        )
        .await;
    }
    for request in &bucket_reads(&data_root, &commitment) {
        let response = send_to_bucket(&server, &alice, request).await;
        let status = response.status();
        assert_eq!(
            status,
            StatusCode::OK,
            "{} {} with no anchor block",
            request.0,
            request.1
        );
    }
}

/// An extension moves `expires_at` past the current anchor block. The node
/// drops the cached entry on `AgreementExtended`, so writes resume without
/// waiting for the TTL; without the drop the cached `expires_at` keeps
/// rejecting them.
#[tokio::test]
async fn extended_agreement_accepts_writes_again() {
    let alice = sr25519::Pair::from_string("//Alice", None).unwrap();
    let member = || {
        StaticMembershipResolver::private(vec![
            (AccountId32::new(alice.public().0), Role::Writer).into()
        ])
    };
    let (server, access, _, changed) =
        switchable_server(member().with_expires_at(100).access).await;
    server.set_anchor_block(100);
    let put = || {
        put_node(
            &server,
            Some(make_auth_header(&alice, "PUT", 1, current_timestamp())),
            b"after the extension",
        )
    };

    let before = put().await;
    assert_error(
        before,
        StatusCode::FORBIDDEN,
        "agreement_expired",
        "before the extension",
    )
    .await;

    *access.lock().unwrap() = member().with_expires_at(200).access;
    changed.mark(1);

    assert_eq!(put().await.status(), StatusCode::OK);
}
