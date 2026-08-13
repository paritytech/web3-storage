// SPDX-License-Identifier: GPL-3.0-only

//! Integration tests for the off-chain term negotiation endpoint.
//!
//! These spin up a real HTTP server (served *with* `ConnectInfo`, matching
//! production in `command.rs`, since the `/negotiate` rate-limit middleware
//! extracts the peer `SocketAddr`) and drive `POST /negotiate` through its
//! happy path, validation failures, the signing/info prerequisites, and the
//! per-IP rate limiter.

use axum::http::StatusCode;
use provider_auth::{Authenticator, StaticMembershipResolver};
use provider_storage::{DiskStorage, NonceStore, NullNonceStore, Storage};
use reqwest::Client;
use serde_json::Value;
use sp_core::{sr25519, Pair};
use sp_runtime::{AccountId32, MultiSignature};
use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Duration;
use storage_primitives::ReplicaTerms;
use storage_provider_node::ProviderInfo;
use storage_provider_node::{
    create_router, NegotiateRequest, NonceCounter, PalletConstants, ProviderDeps, ProviderState,
    SignedTerms,
};
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
        let deps = ProviderDeps {
            storage: Arc::new(Storage::new()),
            nonce_store: Arc::new(NullNonceStore),
            auth: Arc::new(Authenticator::new(
                StaticMembershipResolver(vec![]),
                Duration::from_secs(60),
                Duration::from_secs(300),
            )),
        };
        let state = ProviderState::with_seed(deps, PROVIDER_SEED).expect("//Alice is a valid SURI");
        // Simulate what the coordinator does once registration lands: publish
        // constants, bootstrap the nonce counter, then publish provider_info.
        // Together these satisfy every `/negotiate` prerequisite.
        state
            .chain_state
            .current_anchor_block
            .store(100, std::sync::atomic::Ordering::Relaxed);
        *state.chain_state.constants.write() = Some(PalletConstants {
            request_timeout: 200,
        });
        let counter = std::sync::Arc::new(NonceCounter::new(1));
        counter.bootstrap_from_hsn(0);
        *state.chain_state.nonce_counter.write() = Some(counter);
        *state.chain_state.provider_info.write() = Some(info);
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

    async fn info(&self) -> Value {
        self.client
            .get(self.url("/info"))
            .send()
            .await
            .unwrap()
            .json()
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
        deregister_at: None,
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
async fn negotiate_valid_until_is_anchor_block_plus_request_timeout() {
    // `current_anchor_block` is the pallet's anchor clock — the block the pallet
    // checks `valid_until` against. Seed a Paseo-scale value to prove the
    // validity window is anchored to it (a parachain-height-based window
    // would be rejected on-chain as already expired).
    let deps = ProviderDeps {
        storage: Arc::new(Storage::new()),
        nonce_store: Arc::new(NullNonceStore),
        auth: Arc::new(Authenticator::new(
            StaticMembershipResolver(vec![]),
            Duration::from_secs(60),
            Duration::from_secs(300),
        )),
    };
    let state = ProviderState::with_seed(deps, PROVIDER_SEED).unwrap();
    state
        .chain_state
        .current_anchor_block
        .store(29_123_456, std::sync::atomic::Ordering::Relaxed);
    *state.chain_state.constants.write() = Some(PalletConstants {
        request_timeout: 3_600,
    });
    let counter = std::sync::Arc::new(NonceCounter::new(1));
    counter.bootstrap_from_hsn(0);
    *state.chain_state.nonce_counter.write() = Some(counter);
    *state.chain_state.provider_info.write() = Some(provider_info());
    let server = TestServer::serve(Arc::new(state)).await;

    let resp = server.negotiate(&primary_request()).await;
    assert_eq!(resp.status(), StatusCode::OK);
    let signed: SignedTerms = resp.json().await.unwrap();
    assert_eq!(signed.terms.valid_until, 29_123_456 + 3_600);
}

#[tokio::test]
async fn negotiate_503_when_anchor_block_unknown() {
    // Everything ready except the anchor clock (`current_anchor_block == 0`,
    // i.e. the chain-state coordinator has not processed a finalized block
    // yet): signing would emit terms whose `valid_until` is meaningless on
    // the pallet's clock, so the handler must refuse.
    let deps = ProviderDeps {
        storage: Arc::new(Storage::new()),
        nonce_store: Arc::new(NullNonceStore),
        auth: Arc::new(Authenticator::new(
            StaticMembershipResolver(vec![]),
            Duration::from_secs(60),
            Duration::from_secs(300),
        )),
    };
    let state = ProviderState::with_seed(deps, PROVIDER_SEED).unwrap();
    *state.chain_state.constants.write() = Some(PalletConstants {
        request_timeout: 200,
    });
    let counter = std::sync::Arc::new(NonceCounter::new(1));
    counter.bootstrap_from_hsn(0);
    *state.chain_state.nonce_counter.write() = Some(counter);
    *state.chain_state.provider_info.write() = Some(provider_info());
    let server = TestServer::serve(Arc::new(state)).await;

    let resp = server.negotiate(&primary_request()).await;
    assert_eq!(resp.status(), StatusCode::SERVICE_UNAVAILABLE);
    let body: Value = resp.json().await.unwrap();
    assert_eq!(body["error"], "chain_state_not_ready");
}

#[tokio::test]
async fn negotiate_503_when_request_timeout_unknown() {
    // The mirror case: clock known but the RequestTimeout constant not yet
    // fetched — an unbounded validity window must not be signed either.
    let deps = ProviderDeps {
        storage: Arc::new(Storage::new()),
        nonce_store: Arc::new(NullNonceStore),
        auth: Arc::new(Authenticator::new(
            StaticMembershipResolver(vec![]),
            Duration::from_secs(60),
            Duration::from_secs(300),
        )),
    };
    let state = ProviderState::with_seed(deps, PROVIDER_SEED).unwrap();
    state
        .chain_state
        .current_anchor_block
        .store(100, std::sync::atomic::Ordering::Relaxed);
    let counter = std::sync::Arc::new(NonceCounter::new(1));
    counter.bootstrap_from_hsn(0);
    *state.chain_state.nonce_counter.write() = Some(counter);
    *state.chain_state.provider_info.write() = Some(provider_info());
    let server = TestServer::serve(Arc::new(state)).await;

    let resp = server.negotiate(&primary_request()).await;
    assert_eq!(resp.status(), StatusCode::SERVICE_UNAVAILABLE);
    let body: Value = resp.json().await.unwrap();
    assert_eq!(body["error"], "chain_state_not_ready");
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
    let deps = ProviderDeps {
        storage: Arc::new(Storage::new()),
        nonce_store: Arc::new(NullNonceStore),
        auth: Arc::new(Authenticator::new(
            StaticMembershipResolver(vec![]),
            Duration::from_secs(60),
            Duration::from_secs(300),
        )),
    };
    let server = TestServer::serve(Arc::new(ProviderState::with_provider_id(
        deps,
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
    // Keypair present and chain state ready, but no on-chain registration info
    // loaded (the reconciler never published it): the node cannot validate terms
    // it would be bound to, so it must refuse. `provider_info` defaults to `None`.
    let deps = ProviderDeps {
        storage: Arc::new(Storage::new()),
        nonce_store: Arc::new(NullNonceStore),
        auth: Arc::new(Authenticator::new(
            StaticMembershipResolver(vec![]),
            Duration::from_secs(60),
            Duration::from_secs(300),
        )),
    };
    let state = ProviderState::with_seed(deps, PROVIDER_SEED).unwrap();
    state
        .chain_state
        .current_anchor_block
        .store(100, std::sync::atomic::Ordering::Relaxed);
    *state.chain_state.constants.write() = Some(PalletConstants {
        request_timeout: 200,
    });
    // provider_info and nonce_counter intentionally left None.
    let state = Arc::new(state);
    let server = TestServer::serve(state.clone()).await;

    let resp = server.negotiate(&primary_request()).await;
    assert_eq!(resp.status(), StatusCode::SERVICE_UNAVAILABLE);
    let body: Value = resp.json().await.unwrap();
    assert_eq!(body["error"], "provider_info_unavailable");

    // Once on-chain info lands (mirroring the coordinator: bootstrap nonce
    // counter, then publish provider_info), negotiation succeeds.
    let counter = std::sync::Arc::new(NonceCounter::new(1));
    counter.bootstrap_from_hsn(0);
    *state.chain_state.nonce_counter.write() = Some(counter);
    *state.chain_state.provider_info.write() = Some(provider_info());
    let resp = server.negotiate(&primary_request()).await;
    assert_eq!(resp.status(), StatusCode::OK);
}

#[tokio::test]
async fn negotiate_503_when_provider_deregistering() {
    // Every prerequisite satisfied, but the provider has announced deregistration:
    // it is winding down and must refuse to sign new terms.
    let mut info = provider_info();
    info.deregister_at = Some(150);
    let server = TestServer::ready(info).await;

    let resp = server.negotiate(&primary_request()).await;
    assert_eq!(resp.status(), StatusCode::SERVICE_UNAVAILABLE);
    let body: Value = resp.json().await.unwrap();
    assert_eq!(body["error"], "provider_deregistering");
}

// ─── /info readiness flag tests ──────────────────────────────────────────────

#[tokio::test]
async fn info_deregistering_false_for_active_provider() {
    // A fully-ready provider with no pending deregistration shows
    // `readiness.deregistering: false`.
    let server = TestServer::ready(provider_info()).await;
    let body = server.info().await;

    assert_eq!(body["readiness"]["deregistering"], false);
    assert_eq!(body["readiness"]["provider_info_loaded"], true);
    assert_eq!(body["readiness"]["signing_configured"], true);
    assert_eq!(body["readiness"]["nonce_counter_ready"], true);
    // deregister_at absent from provider_registration_info.
    assert!(body["provider_registration_info"]["deregister_at"].is_null());
}

#[tokio::test]
async fn info_deregistering_true_and_block_surfaced_when_announced() {
    // After DeregisterAnnounced, the coordinator re-fetches and publishes an
    // info where deregister_at = Some(150). The /info response must reflect
    // this in both the readiness flag and the raw provider_registration_info.
    let mut info = provider_info();
    info.deregister_at = Some(150);
    let server = TestServer::ready(info).await;

    let body = server.info().await;

    assert_eq!(body["readiness"]["deregistering"], true);
    // Every other readiness flag is still true — the node is still up.
    assert_eq!(body["readiness"]["provider_info_loaded"], true);
    assert_eq!(body["readiness"]["signing_configured"], true);
    assert_eq!(body["readiness"]["nonce_counter_ready"], true);
    // The raw block number surfaces so operators can see when deregistration
    // becomes finalisable.
    assert_eq!(body["provider_registration_info"]["deregister_at"], 150);
}

#[tokio::test]
async fn negotiate_transitions_to_info_unavailable_after_complete_deregister() {
    // Lifecycle: announced → negotiate is blocked → complete_deregister fires
    // → coordinator gets ProviderDeregistered → re-fetches storage which now
    // returns None → clears provider_info. Subsequent negotiate calls should
    // return provider_info_unavailable, not provider_deregistering (the info is
    // just gone at that point).
    let deps = ProviderDeps {
        storage: Arc::new(Storage::new()),
        nonce_store: Arc::new(NullNonceStore),
        auth: Arc::new(Authenticator::new(
            StaticMembershipResolver(vec![]),
            Duration::from_secs(60),
            Duration::from_secs(300),
        )),
    };
    let state = ProviderState::with_seed(deps, PROVIDER_SEED).unwrap();
    state
        .chain_state
        .current_anchor_block
        .store(100, std::sync::atomic::Ordering::Relaxed);
    *state.chain_state.constants.write() = Some(PalletConstants {
        request_timeout: 200,
    });
    let counter = std::sync::Arc::new(NonceCounter::new(1));
    counter.bootstrap_from_hsn(0);
    *state.chain_state.nonce_counter.write() = Some(counter);
    // Phase 1: deregistration announced.
    let mut deregistering = provider_info();
    deregistering.deregister_at = Some(150);
    *state.chain_state.provider_info.write() = Some(deregistering);
    let state = Arc::new(state);
    let server = TestServer::serve(state.clone()).await;

    let resp = server.negotiate(&primary_request()).await;
    assert_eq!(resp.status(), StatusCode::SERVICE_UNAVAILABLE);
    assert_eq!(
        resp.json::<Value>().await.unwrap()["error"],
        "provider_deregistering"
    );

    // Phase 2: complete_deregister — coordinator clears provider_info and
    // nonce_counter (same as when the storage query returns None).
    *state.chain_state.provider_info.write() = None;
    *state.chain_state.nonce_counter.write() = None;

    let resp = server.negotiate(&primary_request()).await;
    assert_eq!(resp.status(), StatusCode::SERVICE_UNAVAILABLE);
    assert_eq!(
        resp.json::<Value>().await.unwrap()["error"],
        "provider_info_unavailable"
    );

    // /info reflects both cleared flags.
    let body = server.info().await;
    assert_eq!(body["readiness"]["deregistering"], false);
    assert_eq!(body["readiness"]["provider_info_loaded"], false);
    assert!(body["provider_registration_info"].is_null());
}

#[tokio::test]
async fn negotiate_recovers_after_deregister_cancelled() {
    // Lifecycle: announced → negotiate blocked → DeregisterCancelled fires →
    // coordinator re-fetches storage which now reports deregister_at = None →
    // negotiate signs again. Mirrors the coordinator clearing the deregistering
    // state when a provider backs out of winding down.
    let deps = ProviderDeps {
        storage: Arc::new(Storage::new()),
        nonce_store: Arc::new(NullNonceStore),
        auth: Arc::new(Authenticator::new(
            StaticMembershipResolver(vec![]),
            Duration::from_secs(60),
            Duration::from_secs(300),
        )),
    };
    let state = ProviderState::with_seed(deps, PROVIDER_SEED).unwrap();
    state
        .chain_state
        .current_anchor_block
        .store(100, std::sync::atomic::Ordering::Relaxed);
    *state.chain_state.constants.write() = Some(PalletConstants {
        request_timeout: 200,
    });
    let counter = std::sync::Arc::new(NonceCounter::new(1));
    counter.bootstrap_from_hsn(0);
    *state.chain_state.nonce_counter.write() = Some(counter);
    let mut deregistering = provider_info();
    deregistering.deregister_at = Some(150);
    *state.chain_state.provider_info.write() = Some(deregistering);
    let state = Arc::new(state);
    let server = TestServer::serve(state.clone()).await;

    // Announced: refused.
    let resp = server.negotiate(&primary_request()).await;
    assert_eq!(resp.status(), StatusCode::SERVICE_UNAVAILABLE);
    assert_eq!(
        resp.json::<Value>().await.unwrap()["error"],
        "provider_deregistering"
    );

    // Cancelled: coordinator re-publishes info with deregister_at back to None.
    *state.chain_state.provider_info.write() = Some(provider_info());

    let resp = server.negotiate(&primary_request()).await;
    assert_eq!(resp.status(), StatusCode::OK);

    // /info confirms the flag cleared while the provider stays registered.
    let body = server.info().await;
    assert_eq!(body["readiness"]["deregistering"], false);
    assert_eq!(body["readiness"]["provider_info_loaded"], true);
    assert!(body["provider_registration_info"]["deregister_at"].is_null());
}

#[tokio::test]
async fn negotiate_503_when_nonce_counter_absent() {
    // Registered (provider_info loaded) but the coordinator has not yet
    // published any nonce counter (nonce_counter == None). The handler must
    // refuse so we never sign a nonce not derived from on-chain state.
    let deps = ProviderDeps {
        storage: Arc::new(Storage::new()),
        nonce_store: Arc::new(NullNonceStore),
        auth: Arc::new(Authenticator::new(
            StaticMembershipResolver(vec![]),
            Duration::from_secs(60),
            Duration::from_secs(300),
        )),
    };
    let state = ProviderState::with_seed(deps, PROVIDER_SEED).unwrap();
    state
        .chain_state
        .current_anchor_block
        .store(100, std::sync::atomic::Ordering::Relaxed);
    *state.chain_state.constants.write() = Some(PalletConstants {
        request_timeout: 200,
    });
    *state.chain_state.provider_info.write() = Some(provider_info());
    // nonce_counter intentionally left None.
    let server = TestServer::serve(Arc::new(state)).await;

    let resp = server.negotiate(&primary_request()).await;
    assert_eq!(resp.status(), StatusCode::SERVICE_UNAVAILABLE);
    let body: Value = resp.json().await.unwrap();
    assert_eq!(body["error"], "nonce_counter_unavailable");
}

#[tokio::test]
async fn negotiate_503_when_nonce_counter_present_but_not_bootstrapped() {
    // Exercises the transient window where the coordinator has published a
    // Some counter but bootstrap_from_hsn has not yet been called (e.g. the
    // chain returned the provider info but replay state was not yet visible).
    // The handler must refuse until is_bootstrapped() is true.
    let deps = ProviderDeps {
        storage: Arc::new(Storage::new()),
        nonce_store: Arc::new(NullNonceStore),
        auth: Arc::new(Authenticator::new(
            StaticMembershipResolver(vec![]),
            Duration::from_secs(60),
            Duration::from_secs(300),
        )),
    };
    let state = ProviderState::with_seed(deps, PROVIDER_SEED).unwrap();
    state
        .chain_state
        .current_anchor_block
        .store(100, std::sync::atomic::Ordering::Relaxed);
    *state.chain_state.constants.write() = Some(PalletConstants {
        request_timeout: 200,
    });
    *state.chain_state.provider_info.write() = Some(provider_info());
    // Counter is Some but not bootstrapped (no bootstrap_from_hsn call).
    let counter = std::sync::Arc::new(NonceCounter::new(1));
    *state.chain_state.nonce_counter.write() = Some(counter);
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

// ─── NonceCounter ─────────────────────────────────────────────────────────────
//
// `NonceCounter` is the nonce source `/negotiate` allocates from. The
// chain-alignment advance and bootstrap semantics are exercised here directly
// against the public type.

#[test]
fn nonce_counter_is_unbootstrapped_until_aligned() {
    let counter = NonceCounter::new(1);
    // Fresh counter has not been reconciled with the chain's replay window.
    assert!(!counter.is_bootstrapped());
    counter.bootstrap_from_hsn(0);
    assert!(counter.is_bootstrapped());
}

#[test]
fn bootstrap_from_hsn_advances_to_hsn_plus_one() {
    // Counter starts at 1 but the chain's replay head is already at 10, so the
    // node must resume at 11 — never reissue a nonce the chain has seen.
    let counter = NonceCounter::new(1);
    counter.bootstrap_from_hsn(10);
    assert!(counter.is_bootstrapped());
    assert_eq!(counter.next(), 11);
    assert_eq!(counter.next(), 12);
}

#[test]
fn bootstrap_from_hsn_never_rewinds() {
    // A stale/lower hsn (e.g. an out-of-order poll) must not pull the counter
    // back below nonces it may already have issued.
    let counter = NonceCounter::new(1);
    counter.bootstrap_from_hsn(10); // now at 11
    assert_eq!(counter.next(), 11); // -> 12
    counter.bootstrap_from_hsn(3); // lower head: no-op
    assert_eq!(counter.next(), 12);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn concurrent_next_allocates_distinct_nonces() {
    use std::collections::HashSet;
    use std::sync::Arc;

    // Hammer the atomic from many tasks: every allocation must be unique and the
    // count exact (exercises the `compare_exchange_weak` retry path under load).
    let counter = Arc::new(NonceCounter::new(1));
    counter.bootstrap_from_hsn(0);

    let mut handles = Vec::new();
    for _ in 0..16 {
        let c = counter.clone();
        handles.push(tokio::spawn(async move {
            (0..256).map(|_| c.next()).collect::<Vec<_>>()
        }));
    }

    let mut seen = HashSet::new();
    for h in handles {
        for nonce in h.await.unwrap() {
            assert!(seen.insert(nonce), "nonce {nonce} was allocated twice");
        }
    }
    assert_eq!(seen.len(), 16 * 256);
}

// ─── NonceCounter persistence (with_store) ────────────────────────────────────

#[test]
fn with_store_counter_persists_on_next() {
    // A counter backed by a DiskNonceStore persists each allocation so a fresh
    // counter seeded from the store resumes above the last issued nonce.
    let dir = tempfile::TempDir::new().unwrap();
    let storage = DiskStorage::new(dir.path()).unwrap();
    let store = storage.nonce_store();

    let counter = NonceCounter::with_store(1, store.clone());
    counter.bootstrap_from_hsn(0); // counter now at 1
    assert_eq!(counter.next(), 1); // persist(1) → next starts at 2
    assert_eq!(counter.next(), 2); // persist(2)

    // A fresh counter seeded from the stored value resumes above the issued nonces.
    let new_counter = NonceCounter::with_store(store.load().unwrap_or(1), store.clone());
    new_counter.bootstrap_from_hsn(0); // chain hsn=0, so floor=1, but stored=2 wins
    assert!(
        new_counter.next() > 2,
        "restarted counter must resume above the last issued nonce"
    );
}

#[test]
fn new_counter_uses_null_store_so_existing_tests_compile() {
    // Regression: NonceCounter::new must still work with no store argument.
    let counter = NonceCounter::new(1);
    counter.bootstrap_from_hsn(5);
    assert_eq!(counter.next(), 6);
    // NullNonceStore persists nothing — load returns None.
    let null: NullNonceStore = Default::default();
    assert!(null.load().is_none());
}
