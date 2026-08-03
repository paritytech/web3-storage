// SPDX-License-Identifier: Apache-2.0

//! Shared test helpers for integration tests.

use provider_storage::{NullNonceStore, Storage};
use sp_core::crypto::Ss58Codec;
use sp_core::Pair;
use sp_runtime::AccountId32;
use std::sync::{Arc, OnceLock};
use std::time::Duration;
use storage_client::{
    sign_terms, AdminClient, AgreementTermsOf, ChallengerClient, ClientConfig, DiscoveryClient,
    ProviderClient, ProviderSettings, Signer, StorageUserClient,
};
use storage_primitives::{AgreementTerms, Role};
use storage_provider_node::auth::{MembershipCache, StaticMembershipResolver};
use storage_provider_node::{create_router, ProviderDeps, ProviderState};
use tokio::net::TcpListener;
use tokio::sync::{Mutex, MutexGuard};

// ─── Chain endpoint ───────────────────────────────────────────────────────────

/// Default parachain WebSocket endpoint used by `just start-chain`.
pub const CHAIN_WS: &str = "ws://127.0.0.1:2222";

// ─── Chain serialization lock ─────────────────────────────────────────────────

/// Acquire this lock before any test that submits chain extrinsics
/// to avoid error "Priority is too low"
///
#[allow(dead_code)]
pub async fn chain_guard() -> MutexGuard<'static, ()> {
    static LOCK: OnceLock<Mutex<()>> = OnceLock::new();
    // Initialize tracing once per process
    let _ = tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::from_default_env()
                .add_directive(tracing::Level::INFO.into()),
        )
        .with_test_writer()
        .try_init();
    LOCK.get_or_init(|| Mutex::new(())).lock().await
}

/// Minimum provider stake: 1000 tokens × 12 decimals.
const MIN_STAKE: u128 = 1_000 * 1_000_000_000_000u128;

// ─── Dev account helpers ──────────────────────────────────────────────────────

/// Derive the SS58 address for a dev account by name ("alice", "bob", …).
///
/// Avoids hardcoding addresses in tests. Derived from the same `//Name` seed the
/// signer uses, so the account and signer always match.
#[allow(dead_code)]
pub fn dev_ss58(name: &str) -> String {
    dev_account(name).to_ss58check()
}

/// The `//Name` secret URI for a dev account name ("alice" -> "//Alice").
fn dev_suri(name: &str) -> String {
    let mut chars = name.chars();
    match chars.next() {
        Some(first) => format!("//{}{}", first.to_ascii_uppercase(), chars.as_str()),
        None => "//Alice".to_string(),
    }
}

/// Derive the typed `AccountId32` for a dev account by name ("alice", "bob", …).
///
/// Same derivation as [`dev_ss58`], just without the SS58 round-trip — useful for
/// storage queries / parser APIs that take `&AccountId32` directly.
#[allow(dead_code)]
pub fn dev_account(name: &str) -> AccountId32 {
    let signer = Signer::from_seed(&dev_suri(name)).expect("valid dev seed");
    AccountId32::from(signer.keypair().public_key().0)
}

// ─── Chain client config ──────────────────────────────────────────────────────

fn chain_config() -> ClientConfig {
    ClientConfig {
        chain_ws_url: CHAIN_WS.to_string(),
        provider_urls: vec![],
        timeout_secs: 30,
        enable_retries: false,
    }
}

// ─── Common chain setup ───────────────────────────────────────────────────────

/// State created by [`chain_setup`] and shared across each test's lifecycle.
#[allow(dead_code)]
pub struct ChainSetup {
    /// Alice's SS58 address (she is the admin *and* the registered provider).
    pub alice_ss58: String,
    /// Bob's SS58 address (used for member management tests).
    pub bob_ss58: String,
    /// A freshly created bucket owned by Alice.
    pub bucket_id: u64,
}

/// Set up the minimum on-chain state needed to exercise admin, challenger, and
/// discovery clients against an otherwise empty dev chain:
///
/// 1. Registers Alice as a storage provider (skipped when she is already
///    registered — avoids burning an extrinsic on every test rerun).
/// 2. Updates Alice's settings so she is accepting primary agreements.
/// 3. Creates a fresh bucket owned by Alice.
///
/// Returns `None` when the chain is not reachable, allowing callers to skip
/// the test gracefully.
#[allow(dead_code)]
pub async fn chain_setup() -> Option<ChainSetup> {
    let alice_ss58 = dev_ss58("alice");
    let bob_ss58 = dev_ss58("bob");

    // ── Register Alice as provider ────────────────────────────────────────────
    let mut provider =
        ProviderClient::new(chain_config(), Signer::from_seed("//Alice").ok()?).ok()?;
    if provider.connect().await.is_err() {
        return None;
    }

    let already_registered = matches!(
        provider.get_provider_info(&dev_account("alice")).await,
        Ok(Some(_))
    );

    let alice_keypair = sp_core::sr25519::Pair::from_string("//Alice", None).unwrap();
    let alice_pubkey = alice_keypair.public().0.to_vec();

    if !already_registered {
        // Idempotent: ignore "already registered" errors so tests survive races.
        let _ = provider
            .register(
                "/ip4/127.0.0.1/tcp/3333".to_string(),
                alice_pubkey,
                MIN_STAKE,
            )
            .await;

        let _ = provider
            .update_settings(ProviderSettings {
                price_per_byte: 1_000_000,
                min_duration: 1,
                max_duration: 1_000_000,
                accepting_primary: true,
                replica_sync_price: None,
                accepting_extensions: true,
                max_capacity: 10 * 1024 * 1024 * 1024, // 10 GiB
            })
            .await;
    }

    // ── Create a fresh bucket as Alice via signed-terms flow ──────────────────
    // Alice is both the provider (signs terms) and the bucket owner (redeems).
    // Nonces must be unique per provider, so we pick one based on the current
    // block-ish counter — using time-since-epoch nanos so reruns don't collide.
    let mut admin = AdminClient::new(chain_config(), Signer::from_seed("//Alice").ok()?).ok()?;
    if admin.connect().await.is_err() {
        return None;
    }

    let nonce = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos() as u64)
        .unwrap_or(0);

    let terms: AgreementTermsOf = AgreementTerms {
        owner: dev_account("alice"),
        max_bytes: 1_000_000,
        duration: 100,
        price_per_byte: 1,
        valid_until: u32::MAX,
        nonce,
        replica_params: None,
        bucket_id: None,
    };
    let signed_terms = sign_terms(&alice_keypair, terms);
    let bucket_id = admin
        .establish_storage_agreement(alice_ss58.clone(), signed_terms)
        .await
        .ok()?;

    Some(ChainSetup {
        alice_ss58,
        bob_ss58,
        bucket_id,
    })
}

// ─── Per-client factories ─────────────────────────────────────────────────────

/// Build an `AdminClient` signed by Alice. Returns `None` if the chain is down.
#[allow(dead_code)]
pub async fn alice_admin() -> Option<AdminClient> {
    let mut client = AdminClient::new(chain_config(), Signer::from_seed("//Alice").ok()?).ok()?;
    if client.connect().await.is_err() {
        return None;
    }
    Some(client)
}

/// Build a `ChallengerClient` signed by Alice. Returns `None` if the chain is down.
#[allow(dead_code)]
pub async fn alice_challenger() -> Option<ChallengerClient> {
    let mut client =
        ChallengerClient::new(chain_config(), Signer::from_seed("//Alice").ok()?).ok()?;
    if client.connect().await.is_err() {
        return None;
    }
    Some(client)
}

/// Build a `ProviderClient` signed by Alice. Returns `None` if the chain is down.
#[allow(dead_code)]
pub async fn alice_provider() -> Option<ProviderClient> {
    let mut client =
        ProviderClient::new(chain_config(), Signer::from_seed("//Alice").ok()?).ok()?;
    if client.connect().await.is_err() {
        return None;
    }
    Some(client)
}

/// Build a read-only `DiscoveryClient`. Returns `None` if the chain is down.
#[allow(dead_code)]
pub async fn dev_discovery() -> Option<DiscoveryClient> {
    let mut client = DiscoveryClient::new(chain_config()).ok()?;
    if client.connect().await.is_err() {
        return None;
    }
    Some(client)
}

// ─── In-process provider node (for StorageUserClient tests) ──────────────────

/// Spawn an in-process provider node on a random port and return its URL.
///
/// Uses `//Alice` as the signing key so endpoints that sign commitments
/// (`/commit`, `/commitment`, `/checkpoint-signature`, `/delete`) work end-to-end.
/// `//Alice` is granted `Admin` on every bucket.
pub async fn start_test_provider() -> String {
    let deps = ProviderDeps {
        storage: Arc::new(Storage::new()),
        nonce_store: Arc::new(NullNonceStore),
        membership: Arc::new(MembershipCache::new(
            Box::new(StaticMembershipResolver(vec![(
                dev_account("alice"),
                Role::Admin,
            )])),
            Duration::from_secs(60),
        )),
        auth_max_skew: Duration::from_secs(300),
    };
    let state = ProviderState::with_seed(deps, "//Alice").expect("//Alice is a valid SURI");
    let app = create_router(Arc::new(state));

    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();

    tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });

    tokio::time::sleep(tokio::time::Duration::from_millis(10)).await;
    format!("http://{addr}")
}

/// Spawn `n` independent in-process provider nodes.
#[allow(dead_code)]
pub async fn start_providers(n: usize) -> Vec<String> {
    let mut urls = Vec::with_capacity(n);
    for _ in 0..n {
        urls.push(start_test_provider().await);
    }
    urls
}

/// Build a `StorageUserClient` pointed at the given provider URL.
/// The chain WS URL is a placeholder — these integration tests never touch the chain.
///
/// Signs provider requests as `//Alice`, matching the `Admin` member granted by
/// [`start_test_provider`], so uploads/commits are authorized.
#[allow(dead_code)]
pub fn make_client(provider_url: String) -> StorageUserClient {
    StorageUserClient::new(
        ClientConfig {
            chain_ws_url: CHAIN_WS.to_string(),
            provider_urls: vec![provider_url],
            timeout_secs: 10,
            enable_retries: false,
        },
        subxt_signer::sr25519::dev::alice().into(),
    )
    .expect("ClientConfig should be valid")
}
