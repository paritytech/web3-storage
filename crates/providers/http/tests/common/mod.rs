// SPDX-License-Identifier: Apache-2.0

//! Shared test helpers for the provider-node integration suites.
//!
//! Tests use [`SignedClient`] to sign every request as `//Alice`.

// Each integration suite compiles this module in its own test crate and uses only a
// subset of it, so per-crate analysis flags the rest.
#![allow(dead_code)]

use provider_auth::{build_auth_header, Authenticator, StaticMembershipResolver};
use provider_http::{create_router, ProviderDeps, ProviderState};
pub use provider_storage::StorageBackendSpec;
use reqwest::{Method, RequestBuilder};
use sp_core::{sr25519, Pair};
use std::net::SocketAddr;
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};
use storage_primitives::Role;
use tempfile::TempDir;

type AccountId32 = sp_core::crypto::AccountId32;

/// Seed the provider signs with when a suite wants a signing identity. Same
/// key as the client's: the suites only need the signatures to verify, not the
/// two sides to be distinguishable.
pub const PROVIDER_SEED: &str = TEST_MEMBER_SEED;

/// Provider serving on a random port, over its own throwaway backend.
pub struct TestServer {
    addr: SocketAddr,
    pub client: SignedClient,
    _dir: TempDir,
}

impl TestServer {
    /// `state` picks the identity: seeded (signing) or provider-id only.
    pub async fn start(
        backend: StorageBackendSpec,
        state: impl FnOnce(ProviderDeps) -> ProviderState,
    ) -> Self {
        let (dir, deps) = Self::deps(backend);
        let state = state(deps);
        publish_matching_registration(&state);
        Self::from_state(state, dir).await
    }

    /// Like [`start`](Self::start), but leaves the chain-state coordinator's
    /// registration unpublished — covers the window before its first refresh,
    /// or a provider the chain does not know about.
    pub async fn start_unregistered(
        backend: StorageBackendSpec,
        state: impl FnOnce(ProviderDeps) -> ProviderState,
    ) -> Self {
        let (dir, deps) = Self::deps(backend);
        Self::from_state(state(deps), dir).await
    }

    /// `backend` only selects the engine; its own `path` is discarded here in
    /// favour of a fresh temp dir, since every test server needs its own
    /// throwaway location regardless of what the caller passed in.
    fn deps(backend: StorageBackendSpec) -> (TempDir, ProviderDeps) {
        let dir = tempfile::Builder::new()
            .prefix(provider_storage::TEMP_DIR_PREFIX)
            .tempdir()
            .expect("temp dir");
        let backend = match backend {
            StorageBackendSpec::RocksDb { .. } => StorageBackendSpec::RocksDb {
                path: dir.path().to_path_buf(),
            },
        };
        let (storage, nonce_store) = backend.build().expect("backend opens");
        let deps = ProviderDeps {
            storage,
            nonce_store,
            auth: Arc::new(Authenticator::new(StaticMembershipResolver(vec![(
                test_member_account(),
                Role::Admin,
            )
                .into()]))),
        };
        (dir, deps)
    }

    async fn from_state(state: ProviderState, dir: TempDir) -> Self {
        let (addr, client) = serve(state).await;
        Self {
            addr,
            client,
            _dir: dir,
        }
    }

    pub fn url(&self, path: &str) -> String {
        format!("http://{}{}", self.addr, path)
    }
}

/// Publish a registration snapshot whose `public_key` matches this state's
/// own signing key, as the chain-state coordinator would once the provider
/// is registered on chain — signing endpoints refuse otherwise. Skipped for
/// a keyless state, which has no key to register.
pub fn publish_matching_registration(state: &ProviderState) {
    let Some(keypair) = state.keypair.as_ref() else {
        return;
    };
    state
        .chain_state
        .provider_info
        .write()
        .replace(provider_types::ProviderInfo {
            multiaddr: "/ip4/127.0.0.1/tcp/3333".to_string(),
            // Must match the server's own signing key — signing refuses when
            // the registered key differs.
            public_key: keypair.public_key_bytes(),
            stake: 1_000_000_000_000,
            committed_bytes: 0,
            settings: provider_types::ProviderSettings {
                min_duration: 10,
                max_duration: 100_000,
                price_per_byte: 1,
                accepting_primary: true,
                replica_sync_price: None,
                accepting_extensions: true,
                max_capacity: 0,
            },
            stats: Default::default(),
            deregister_at: None,
        });
}

/// Declare tests that run once per backend.
///
/// ```ignore
/// common::backend_tests! {
///     async fn health(backend) {
///         let server = TestServer::new(backend).await;
///         // ...
///     }
/// }
/// ```
///
/// expands to one test per backend, named after it (`health::rocksdb`), so a
/// failure says which one it happened on.
///
/// Allowed as unused: the suites that never parameterize by backend (auth,
/// negotiate, chain-state) compile this module too.
#[allow(unused_macros)]
macro_rules! backend_tests {
    ($(async fn $name:ident($backend:ident) $body:block)*) => {
        $(
            async fn $name($backend: common::StorageBackendSpec) $body

            mod $name {
                #[tokio::test]
                async fn rocksdb() {
                    // The path is discarded and replaced with a fresh temp dir
                    // by `TestServer::deps`; only the engine choice matters here.
                    super::$name(super::common::StorageBackendSpec::RocksDb {
                        path: std::path::PathBuf::new(),
                    })
                    .await
                }
            }
        )*
    };
}

#[allow(unused_imports)]
pub(crate) use backend_tests;

/// The account every test signs as.
pub const TEST_MEMBER_SEED: &str = "//Alice";

/// Derived once per test thread: `//Alice` is a dev *phrase*, so each call
/// would otherwise run PBKDF2 2048 times — tens of ms in a debug build, and
/// this is called several times per test case.
pub fn test_member_pair() -> sr25519::Pair {
    thread_local! {
        static PAIR: sr25519::Pair =
            sr25519::Pair::from_string(TEST_MEMBER_SEED, None).expect("//Alice is a valid SURI");
    }
    PAIR.with(|pair| pair.clone())
}

pub fn test_member_account() -> AccountId32 {
    AccountId32::new(test_member_pair().public().0)
}

/// Spawn the provider on a random port and return its address plus a
/// [`SignedClient`].
pub async fn serve(state: ProviderState) -> (SocketAddr, SignedClient) {
    let app = create_router(Arc::new(state));
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    while tokio::net::TcpStream::connect(addr).await.is_err() {
        tokio::task::yield_now().await;
    }
    (addr, SignedClient::new())
}

pub fn current_timestamp() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("system clock is after the Unix epoch")
        .as_secs()
}

/// `Authorization` value signed by `keypair` over
/// `web3storage:<method>:<bucket_id>:<timestamp>`.
pub fn make_auth_header(
    keypair: &sr25519::Pair,
    method: &str,
    bucket_id: u64,
    timestamp: u64,
) -> String {
    build_auth_header(&keypair.public().0, method, bucket_id, timestamp, |msg| {
        keypair.sign(msg).0
    })
}

/// Bucket id from the URL path (`/s3/{id}/`, `/fs/{id}/`) or a `?bucket_id=`
/// query param. Matched per-component so a key containing a marker can't mislead.
fn parse_bucket_id(url: &str) -> Option<u64> {
    let (path, query) = url.split_once('?').unwrap_or((url, ""));
    let rest = path
        .split_once("/s3/")
        .or_else(|| path.split_once("/fs/"))
        .or_else(|| query.split_once("bucket_id="))
        .map(|(_, rest)| rest)?;
    rest.split(|c: char| !c.is_ascii_digit())
        .find(|t| !t.is_empty())?
        .parse()
        .ok()
}

/// `reqwest::Client` that signs every request as the test member.
///
/// Verb methods infer the bucket from the URL, defaulting to `1`. For an
/// endpoint (`/node`, `/commit`, `/delete`) targeting a bucket other than `1`,
/// sign explicitly with [`SignedClient::request_bucket`].
pub struct SignedClient {
    inner: reqwest::Client,
    keypair: sr25519::Pair,
}

impl Default for SignedClient {
    fn default() -> Self {
        Self::new()
    }
}

impl SignedClient {
    pub fn new() -> Self {
        Self {
            inner: reqwest::Client::new(),
            keypair: test_member_pair(),
        }
    }

    /// Sign for an explicit bucket id (L0 endpoints whose bucket is in the body).
    pub fn request_bucket(&self, method: Method, url: String, bucket_id: u64) -> RequestBuilder {
        let header = make_auth_header(
            &self.keypair,
            method.as_str(),
            bucket_id,
            current_timestamp(),
        );
        self.inner
            .request(method, url)
            .header(reqwest::header::AUTHORIZATION, header)
    }

    fn auto(&self, method: Method, url: String) -> RequestBuilder {
        let bucket_id = parse_bucket_id(&url).unwrap_or(1);
        self.request_bucket(method, url, bucket_id)
    }

    pub fn get(&self, url: String) -> RequestBuilder {
        self.auto(Method::GET, url)
    }

    pub fn put(&self, url: String) -> RequestBuilder {
        self.auto(Method::PUT, url)
    }

    pub fn post(&self, url: String) -> RequestBuilder {
        self.auto(Method::POST, url)
    }

    pub fn delete(&self, url: String) -> RequestBuilder {
        self.auto(Method::DELETE, url)
    }

    pub fn head(&self, url: String) -> RequestBuilder {
        self.auto(Method::HEAD, url)
    }
}
