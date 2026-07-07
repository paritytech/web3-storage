// SPDX-License-Identifier: GPL-3.0-only

//! Shared test helpers for the provider-node integration suites.
//!
//! Tests run with `//Alice` as a bucket `Admin`
//! ([`with_admin_member`]) and use [`SignedClient`] to sign every request as
//! `//Alice`.


use reqwest::{Method, RequestBuilder};
use sp_core::{sr25519, Pair};
use std::net::SocketAddr;
use std::sync::Arc;
use std::time::{Duration, SystemTime, UNIX_EPOCH};
use storage_primitives::{build_auth_header, Role};
use storage_provider_node::auth::{MembershipCache, StaticMembershipResolver};
use storage_provider_node::{create_router, ProviderState};

type AccountId32 = sp_core::crypto::AccountId32;

/// The account every test signs as.
pub const TEST_MEMBER_SEED: &str = "//Alice";

pub fn test_member_pair() -> sr25519::Pair {
    sr25519::Pair::from_string(TEST_MEMBER_SEED, None).expect("//Alice is a valid SURI")
}

pub fn test_member_account() -> AccountId32 {
    AccountId32::new(test_member_pair().public().0)
}

/// Membership cache over a fixed member set (returned for every bucket).
pub fn membership_cache(members: Vec<(AccountId32, Role)>) -> Arc<MembershipCache> {
    Arc::new(MembershipCache::new(
        Box::new(StaticMembershipResolver(members)),
        Duration::from_secs(60),
    ))
}

/// Enforce auth with the test member as `Admin` on every bucket.
pub fn with_admin_member(mut state: ProviderState) -> ProviderState {
    let cache = membership_cache(vec![(test_member_account(), Role::Admin)]);
    state.set_auth_config(cache, Duration::from_secs(300));
    state
}

/// Spawn the provider on a random port with the test member as `Admin`, and
/// return its address plus a [`SignedClient`].
pub async fn serve(state: ProviderState) -> (SocketAddr, SignedClient) {
    let app = create_router(Arc::new(with_admin_member(state)));
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
