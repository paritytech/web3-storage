// SPDX-License-Identifier: GPL-3.0-only

//! Shared test helpers for the provider-node integration suites.
//!
//! Tests use [`SignedClient`] to sign every request as `//Alice`.

// Each integration suite compiles this module in its own test crate and exercises only
// a subset of these helpers, so per-crate dead-code analysis flags the rest.
#![allow(dead_code)]

use provider_auth::{build_auth_header, Authenticator, StaticMembershipResolver};
use provider_storage::StorageBackendSpec;
use reqwest::{Method, RequestBuilder};
use sp_core::{sr25519, Pair};
use std::net::SocketAddr;
use std::sync::Arc;
use std::time::{Duration, SystemTime, UNIX_EPOCH};
use storage_primitives::Role;
pub use storage_provider_node::cli::StorageBackendKind;
use storage_provider_node::{create_router, ProviderDeps, ProviderState};
use tempfile::TempDir;

type AccountId32 = sp_core::crypto::AccountId32;

/// Seed the provider signs with when a suite wants a signing identity.
pub const PROVIDER_SEED: &str = "//Alice";

/// Provider serving on a random port, over its own throwaway backend.
pub struct TestServer {
    addr: SocketAddr,
    pub client: SignedClient,
    _dir: TempDir,
}

impl TestServer {
    /// `state` picks the identity: seeded (signing) or provider-id only.
    pub async fn start(
        backend: StorageBackendKind,
        state: impl FnOnce(ProviderDeps) -> ProviderState,
    ) -> Self {
        let dir = TempDir::new().expect("temp dir");
        let spec = match backend {
            StorageBackendKind::RocksDb => StorageBackendSpec::RocksDb {
                path: dir.path().to_path_buf(),
            },
        };
        let (storage, nonce_store) = spec.build().expect("backend opens");
        let deps = ProviderDeps {
            storage,
            nonce_store,
            auth: Arc::new(Authenticator::new(
                StaticMembershipResolver(vec![(test_member_account(), Role::Admin).into()]),
                Duration::from_secs(60),
                Duration::from_secs(300),
            )),
        };
        let (addr, client) = serve(state(deps)).await;
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
macro_rules! backend_tests {
    ($(async fn $name:ident($backend:ident) $body:block)*) => {
        $(
            async fn $name($backend: common::StorageBackendKind) $body

            mod $name {
                #[tokio::test]
                async fn rocksdb() {
                    super::$name(super::common::StorageBackendKind::RocksDb).await
                }
            }
        )*
    };
}

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
