// SPDX-License-Identifier: GPL-3.0-only

//! Integration tests for the off-chain term negotiation endpoint.
//!
//! These spin up a real HTTP server (served *with* `ConnectInfo`, matching
//! production in `command.rs`, since the `/negotiate` rate-limit middleware
//! extracts the peer `SocketAddr`) and drive `POST /negotiate` through its
//! happy path, validation failures, the signing/info prerequisites, and the
//! per-IP rate limiter.

use axum::http::StatusCode;
use reqwest::Client;
use serde_json::Value;
use sp_core::{sr25519, Pair};
use sp_runtime::{AccountId32, MultiSignature};
use std::net::SocketAddr;
use std::sync::Arc;
use storage_client::discovery::ProviderInfo;
use storage_primitives::ReplicaTerms;
use storage_provider_node::{create_router, NegotiateRequest, ProviderState, SignedTerms, Storage};
use tokio::net::TcpListener;

const PROVIDER_SEED: &str = "//Alice";

/// Test server for the negotiate endpoint. Serves with `ConnectInfo` so the
/// per-IP rate-limit middleware's `SocketAddr` extractor resolves.
struct TestServer {
    addr: SocketAddr,
    client: Client,
}

impl TestServer {
    /// `//Alice`-signed server whose state advertises `info` on-chain and has a
    /// nonce counter ready, i.e. every `/negotiate` prerequisite satisfied.
    async fn ready(info: ProviderInfo) -> Self {
        let state = ProviderState::with_seed(Arc::new(Storage::new()), PROVIDER_SEED)
            .expect("//Alice is a valid SURI");
        // Publish on-chain registration info and align the nonce counter with
        // the replay window (hsn 0), mirroring what the reconciler does once the
        // provider is registered — satisfying every `/negotiate` prerequisite.
        *state.provider_info.write().unwrap() = Some(info);
        state.nonce_counter.bootstrap_from_hsn(0);
        Self::serve(Arc::new(state)).await
    }

    async fn serve(state: Arc<ProviderState>) -> Self {
        let app = create_router(state);
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move {
            axum::serve(
                listener,
                app.into_make_service_with_connect_info::<SocketAddr>(),
            )
            .await
            .unwrap();
        });
        tokio::time::sleep(tokio::time::Duration::from_millis(10)).await;
        Self {
            addr,
            client: Client::new(),
        }
    }

    fn url(&self, path: &str) -> String {
        format!("http://{}{}", self.addr, path)
    }

    async fn negotiate(&self, req: &NegotiateRequest) -> reqwest::Response {
        self.client
            .post(self.url("/negotiate"))
            .json(req)
            .send()
            .await
            .unwrap()
    }
}

/// Provider settings that accept the [`primary_request`] below: open for
/// primary agreements, listed price 5, duration window [10, 100_000],
/// unlimited capacity.
fn provider_info() -> ProviderInfo {
    ProviderInfo {
        multiaddr: "/ip4/127.0.0.1/tcp/3333".to_string(),
        stake: 1_000_000_000_000,
        committed_bytes: 0,
        max_capacity: 0,
        min_duration: 10,
        max_duration: 100_000,
        price_per_byte: 5,
        accepting_primary: true,
        replica_sync_price: None,
        accepting_extensions: true,
        agreements_total: 0,
        challenges_failed: 0,
    }
}

fn primary_request() -> NegotiateRequest {
    NegotiateRequest {
        owner: AccountId32::new([7u8; 32]),
        max_bytes: 1024,
        duration: 50,
        price_per_byte: 5,
        bucket_id: None,
        replica_params: None,
    }
}

/// `//Alice`'s sr25519 public key — the negotiate handler signs with the same
/// keypair, so signatures must verify under this.
fn alice_public() -> sr25519::Public {
    sr25519::Pair::from_string(PROVIDER_SEED, None)
        .expect("//Alice is a valid SURI")
        .public()
}

#[tokio::test]
async fn negotiate_returns_signed_terms_with_valid_signature() {
    let server = TestServer::ready(provider_info()).await;

    let resp = server.negotiate(&primary_request()).await;
    assert_eq!(resp.status(), StatusCode::OK);

    let signed: SignedTerms = resp.json().await.unwrap();

    // The handler echoes the request but pins price to the provider's own
    // listed price (here equal) and binds a fresh nonce + the requested shape.
    assert_eq!(signed.terms.price_per_byte, 5);
    assert_eq!(signed.terms.max_bytes, 1024);
    assert_eq!(signed.terms.duration, 50);
    assert_eq!(signed.terms.bucket_id, None);
    assert!(signed.terms.replica_params.is_none());

    // The signature must verify under //Alice over blake2_256(signing_payload).
    let hash = sp_core::hashing::blake2_256(&signed.terms.signing_payload());
    let sig = match signed.signature {
        MultiSignature::Sr25519(s) => s,
        other => panic!("expected an sr25519 signature, got {other:?}"),
    };
    assert!(
        sr25519::Pair::verify(&sig, hash, &alice_public()),
        "negotiated terms signature did not verify under //Alice"
    );
}

#[tokio::test]
async fn negotiate_pins_listed_price_when_client_overpays() {
    let server = TestServer::ready(provider_info()).await;

    // Client proposes more than the listed price; the provider must sign its
    // own (lower) listed price, never the client's inflated number.
    let mut req = primary_request();
    req.price_per_byte = 100;

    let resp = server.negotiate(&req).await;
    assert_eq!(resp.status(), StatusCode::OK);
    let signed: SignedTerms = resp.json().await.unwrap();
    assert_eq!(signed.terms.price_per_byte, 5);
}

#[tokio::test]
async fn negotiate_allocates_distinct_monotonic_nonces() {
    let server = TestServer::ready(provider_info()).await;

    let first: SignedTerms = server
        .negotiate(&primary_request())
        .await
        .json()
        .await
        .unwrap();
    let second: SignedTerms = server
        .negotiate(&primary_request())
        .await
        .json()
        .await
        .unwrap();

    // Counter bootstrapped at 1; each call consumes one.
    assert_eq!(first.terms.nonce, 1);
    assert_eq!(second.terms.nonce, 2);
}

#[tokio::test]
async fn negotiate_accepts_replica_when_sync_price_configured() {
    let mut info = provider_info();
    info.accepting_primary = false; // closed for primary…
    info.replica_sync_price = Some(7); // …but open for replicas.
    let server = TestServer::ready(info).await;

    let mut req = primary_request();
    req.bucket_id = Some(42);
    req.replica_params = Some(ReplicaTerms {
        sync_balance: 1_000,
        min_sync_interval: 10,
        sync_price: 10,
    });

    let resp = server.negotiate(&req).await;
    assert_eq!(resp.status(), StatusCode::OK);
    let signed: SignedTerms = resp.json().await.unwrap();
    assert_eq!(signed.terms.bucket_id, Some(42));
    assert!(signed.terms.replica_params.is_some());
}

#[tokio::test]
async fn negotiate_503_when_no_signing_key() {
    // No keypair configured → the handler refuses before doing any work.
    let server = TestServer::serve(Arc::new(ProviderState::with_provider_id(
        Arc::new(Storage::new()),
        "0xtest_provider".to_string(),
    )))
    .await;

    let resp = server.negotiate(&primary_request()).await;
    assert_eq!(resp.status(), StatusCode::SERVICE_UNAVAILABLE);
    let body: Value = resp.json().await.unwrap();
    assert_eq!(body["error"], "signing_unavailable");
}

#[tokio::test]
async fn negotiate_503_when_provider_info_unavailable() {
    // Keypair present but no on-chain registration info loaded (the reconciler
    // never ran): the node cannot validate terms it would be bound to, so it
    // must refuse. `provider_info` defaults to `None`.
    let state = ProviderState::with_seed(Arc::new(Storage::new()), PROVIDER_SEED).unwrap();
    let server = TestServer::serve(Arc::new(state)).await;

    let resp = server.negotiate(&primary_request()).await;
    assert_eq!(resp.status(), StatusCode::SERVICE_UNAVAILABLE);
    let body: Value = resp.json().await.unwrap();
    assert_eq!(body["error"], "provider_info_unavailable");
}

#[tokio::test]
async fn negotiate_503_when_nonce_counter_not_bootstrapped() {
    // Registered (provider_info loaded) but the nonce counter has not been
    // aligned with the chain's replay window yet. The handler must refuse
    // rather than sign a nonce not derived from on-chain state.
    let state = ProviderState::with_seed(Arc::new(Storage::new()), PROVIDER_SEED).unwrap();
    *state.provider_info.write().unwrap() = Some(provider_info());
    // nonce_counter intentionally left un-bootstrapped.
    let server = TestServer::serve(Arc::new(state)).await;

    let resp = server.negotiate(&primary_request()).await;
    assert_eq!(resp.status(), StatusCode::SERVICE_UNAVAILABLE);
    let body: Value = resp.json().await.unwrap();
    assert_eq!(body["error"], "nonce_counter_unavailable");
}

#[tokio::test]
async fn negotiate_422_price_below_listed() {
    let server = TestServer::ready(provider_info()).await;

    let mut req = primary_request();
    req.price_per_byte = 1; // below the listed 5

    let resp = server.negotiate(&req).await;
    assert_eq!(resp.status(), StatusCode::UNPROCESSABLE_ENTITY);
    let body: Value = resp.json().await.unwrap();
    assert_eq!(body["error"], "price_below_listed");
    // u128 fields are serialized as strings.
    assert_eq!(body["details"]["proposed"], "1");
    assert_eq!(body["details"]["listed"], "5");
}

#[tokio::test]
async fn negotiate_422_duration_out_of_bounds() {
    let server = TestServer::ready(provider_info()).await;

    let mut req = primary_request();
    req.duration = 5; // below min_duration 10

    let resp = server.negotiate(&req).await;
    assert_eq!(resp.status(), StatusCode::UNPROCESSABLE_ENTITY);
    let body: Value = resp.json().await.unwrap();
    assert_eq!(body["error"], "duration_out_of_bounds");
    assert_eq!(body["details"]["min"], 10);
    assert_eq!(body["details"]["max"], 100_000);
}

#[tokio::test]
async fn negotiate_422_capacity_exceeded() {
    let mut info = provider_info();
    info.max_capacity = 2048;
    info.committed_bytes = 1536; // only 512 bytes free
    let server = TestServer::ready(info).await;

    let resp = server.negotiate(&primary_request()).await; // wants 1024
    assert_eq!(resp.status(), StatusCode::UNPROCESSABLE_ENTITY);
    let body: Value = resp.json().await.unwrap();
    assert_eq!(body["error"], "capacity_exceeded");
    assert_eq!(body["details"]["requested"], 1024);
}

#[tokio::test]
async fn negotiate_422_zero_bytes() {
    let server = TestServer::ready(provider_info()).await;

    let mut req = primary_request();
    req.max_bytes = 0;

    let resp = server.negotiate(&req).await;
    assert_eq!(resp.status(), StatusCode::UNPROCESSABLE_ENTITY);
    let body: Value = resp.json().await.unwrap();
    assert_eq!(body["error"], "capacity_exceeded");
}

#[tokio::test]
async fn negotiate_422_not_accepting_primary() {
    let mut info = provider_info();
    info.accepting_primary = false;
    let server = TestServer::ready(info).await;

    let resp = server.negotiate(&primary_request()).await;
    assert_eq!(resp.status(), StatusCode::UNPROCESSABLE_ENTITY);
    let body: Value = resp.json().await.unwrap();
    assert_eq!(body["error"], "not_accepting_primary");
}

#[tokio::test]
async fn negotiate_422_not_accepting_replicas() {
    // replica_sync_price is None by default → replica requests are rejected.
    let server = TestServer::ready(provider_info()).await;

    let mut req = primary_request();
    req.bucket_id = Some(1);
    req.replica_params = Some(ReplicaTerms {
        sync_balance: 1_000,
        min_sync_interval: 10,
        sync_price: 10,
    });

    let resp = server.negotiate(&req).await;
    assert_eq!(resp.status(), StatusCode::UNPROCESSABLE_ENTITY);
    let body: Value = resp.json().await.unwrap();
    assert_eq!(body["error"], "not_accepting_replicas");
}

#[tokio::test]
async fn negotiate_rate_limited_after_burst() {
    let server = TestServer::ready(provider_info()).await;

    // Burst capacity is 16 with a 5/s refill. Fire well past the burst
    // concurrently from the same (loopback) IP so the shared per-IP bucket
    // drains and at least one request is throttled with 429.
    let mut handles = Vec::new();
    for _ in 0..40 {
        let client = server.client.clone();
        let url = server.url("/negotiate");
        let req = primary_request();
        handles.push(tokio::spawn(async move {
            client.post(url).json(&req).send().await.unwrap().status()
        }));
    }

    let mut ok = 0;
    let mut limited = 0;
    for h in handles {
        match h.await.unwrap() {
            StatusCode::OK => ok += 1,
            StatusCode::TOO_MANY_REQUESTS => limited += 1,
            other => panic!("unexpected status from /negotiate under load: {other}"),
        }
    }

    assert!(ok > 0, "expected some requests to succeed");
    assert!(
        limited > 0,
        "expected the per-IP rate limiter to reject part of a 40-request burst"
    );
}
