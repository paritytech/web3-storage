//! Shared test helpers for integration tests.

use std::sync::{Arc, OnceLock};
use storage_client::{
    AdminClient, ChallengerClient, ClientConfig, DiscoveryClient, ProviderClient, ProviderSettings,
    StorageUserClient,
};
use storage_provider_node::{create_router, ProviderState, Storage};
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
/// Avoids hardcoding addresses in tests. Uses the same derivation path that
/// `set_dev_signer` uses internally, so the account and signer always match.
#[allow(dead_code)]
pub fn dev_ss58(name: &str) -> String {
    use sp_core::crypto::Ss58Codec as _;
    use sp_runtime::AccountId32;
    use subxt_signer::sr25519::dev;

    let keypair = match name {
        "alice" => dev::alice(),
        "bob" => dev::bob(),
        "charlie" => dev::charlie(),
        "dave" => dev::dave(),
        "eve" => dev::eve(),
        "ferdie" => dev::ferdie(),
        other => panic!("unknown dev account: {other}"),
    };
    let bytes: [u8; 32] = keypair.public_key().0;
    AccountId32::from(bytes).to_ss58check()
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
/// 1. Registers Alice as a storage provider (idempotent — already-registered
///    errors are silently ignored).
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
    let mut provider = ProviderClient::new(chain_config(), alice_ss58.clone()).ok()?;
    if provider.connect().await.is_err() {
        return None;
    }
    provider.set_dev_signer("alice").ok()?;

    // Idempotent: ignore "already registered" errors so tests survive reruns.
    let _ = provider
        .register(
            "/ip4/127.0.0.1/tcp/3333".to_string(),
            vec![0u8; 32],
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

    // ── Create a fresh bucket as Alice ────────────────────────────────────────
    let mut admin = AdminClient::new(chain_config(), alice_ss58.clone()).ok()?;
    if admin.connect().await.is_err() {
        return None;
    }
    admin.set_dev_signer("alice").ok()?;
    let bucket_id = admin.create_bucket(1).await.ok()?;

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
    let mut client = AdminClient::new(chain_config(), dev_ss58("alice")).ok()?;
    if client.connect().await.is_err() {
        return None;
    }
    client.set_dev_signer("alice").ok()?;
    Some(client)
}

/// Build a `ChallengerClient` signed by Alice. Returns `None` if the chain is down.
#[allow(dead_code)]
pub async fn alice_challenger() -> Option<ChallengerClient> {
    let mut client = ChallengerClient::new(chain_config(), dev_ss58("alice")).ok()?;
    if client.connect().await.is_err() {
        return None;
    }
    client.set_dev_signer("alice").ok()?;
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
pub async fn start_test_provider() -> String {
    let storage = Arc::new(Storage::new());
    let state = Arc::new(ProviderState::new(storage, "0xtest_provider".to_string()));
    let app = create_router(state);

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
#[allow(dead_code)]
pub fn make_client(provider_url: String) -> StorageUserClient {
    StorageUserClient::new(ClientConfig {
        chain_ws_url: CHAIN_WS.to_string(),
        provider_urls: vec![provider_url],
        timeout_secs: 10,
        enable_retries: false,
    })
    .expect("ClientConfig should be valid")
}
