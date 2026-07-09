// SPDX-License-Identifier: Apache-2.0

//! Integration tests for `StorageUserClient` request signing against an
//! auth-enabled provider node.
//!
//! Regression coverage for a bug where `StorageUserClient` never sent the
//! `Authorization` header the provider node requires by default (auth is
//! only disabled via `--disable-auth-i-know-what-i-am-doing`), so every
//! upload/commit against a real, non-test provider failed with
//! 401 Unauthorized.

#[path = "../common/mod.rs"]
mod common;

use async_trait::async_trait;
use sp_runtime::AccountId32;
use std::sync::Arc;
use std::time::Duration;
use storage_client::{ChunkingStrategy, ClientConfig, StorageUserClient};
use storage_primitives::Role;
use storage_provider_node::auth::{MembershipCache, MembershipResolver};
use storage_provider_node::{create_router, ProviderState, Storage};
use tokio::net::TcpListener;

/// A fixed membership list, standing in for the real chain-backed resolver.
struct FixedMembership(Vec<(AccountId32, Role)>);

#[async_trait]
impl MembershipResolver for FixedMembership {
    async fn fetch_members(&self, _bucket_id: u64) -> Result<Vec<(AccountId32, Role)>, String> {
        Ok(self.0.clone())
    }
}

/// Spawn an in-process provider with auth *enabled* and `member` granted
/// `role` on every bucket. Returns its URL.
async fn start_auth_enabled_provider(member: AccountId32, role: Role) -> String {
    let storage = Arc::new(Storage::new());
    let mut state = ProviderState::with_seed(storage, "//Alice").expect("//Alice is a valid SURI");
    state.set_auth_config(
        Arc::new(MembershipCache::new(
            Box::new(FixedMembership(vec![(member, role)])),
            Duration::from_secs(60),
        )),
        Duration::from_secs(300),
    );
    let app = create_router(Arc::new(state));

    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });
    tokio::time::sleep(tokio::time::Duration::from_millis(10)).await;
    format!("http://{addr}")
}

fn signed_client(url: String, signer_name: &str) -> StorageUserClient {
    StorageUserClient::new(ClientConfig {
        chain_ws_url: common::CHAIN_WS.to_string(),
        provider_urls: vec![url],
        timeout_secs: 10,
        enable_retries: false,
    })
    .expect("ClientConfig should be valid")
    .with_dev_signer(signer_name)
    .expect("dev signer name should be valid")
}

#[tokio::test]
async fn upload_without_signer_is_rejected() {
    let bob = common::dev_account("bob");
    let url = start_auth_enabled_provider(bob, Role::Writer).await;
    let client = common::make_client(url);

    let result = client
        .upload(1, b"unsigned upload", ChunkingStrategy::default())
        .await;
    assert!(
        result.is_err(),
        "unsigned upload should be rejected by an auth-enabled provider"
    );
}

#[tokio::test]
async fn upload_with_dev_signer_succeeds() {
    let bob = common::dev_account("bob");
    let url = start_auth_enabled_provider(bob, Role::Writer).await;
    let client = signed_client(url, "bob");

    let data = b"signed upload";
    let data_root = client
        .upload(1, data, ChunkingStrategy::default())
        .await
        .expect("signed upload by a Writer should succeed");
    let downloaded = client
        .download(&data_root, 0, data.len() as u64)
        .await
        .expect("download should succeed (reads are unauthenticated)");
    assert_eq!(downloaded, data);
}

#[tokio::test]
async fn upload_with_reader_role_is_rejected() {
    let bob = common::dev_account("bob");
    let url = start_auth_enabled_provider(bob, Role::Reader).await;
    let client = signed_client(url, "bob");

    let result = client
        .upload(1, b"reader upload", ChunkingStrategy::default())
        .await;
    assert!(
        result.is_err(),
        "a signer with only Reader role should not be able to write"
    );
}
