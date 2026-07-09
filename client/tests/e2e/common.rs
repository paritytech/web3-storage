// SPDX-License-Identifier: Apache-2.0

//! Shared helpers for the happy-case e2e workflow tests, mirroring
//! `examples/papi/e2e/helpers.ts`.
//!
//! Unlike `tests/integration/`, these tests require a **live provider node**
//! registered on-chain (`just start-chain` + `just start-provider`), not an
//! in-process one - matching/checkpoint flows need a real on-chain multiaddr.
//! Every helper here returns `None` when the chain or the provider is
//! unreachable, so callers can skip the test gracefully.

#[path = "../common/mod.rs"]
pub mod common;

use common::{dev_account, dev_ss58, CHAIN_WS, MIN_STAKE};
use sp_runtime::AccountId32;
use storage_client::substrate::SubstrateClient;
use storage_client::{
    AdminClient, ClientConfig, NegotiateRequest, ProviderClient, ProviderSettings,
};
use storage_primitives::BucketId;
use subxt::ext::scale_value::At;

/// Default provider HTTP endpoint used by `just start-provider`.
pub const PROVIDER_URL: &str = "http://127.0.0.1:3333";

fn chain_config() -> ClientConfig {
    ClientConfig {
        chain_ws_url: CHAIN_WS.to_string(),
        provider_urls: vec![PROVIDER_URL.to_string()],
        timeout_secs: 30,
        enable_retries: false,
    }
}

/// Check that the provider node at [`PROVIDER_URL`] is reachable via `/health`.
async fn provider_reachable() -> bool {
    reqwest::Client::new()
        .get(format!("{PROVIDER_URL}/health"))
        .timeout(std::time::Duration::from_secs(3))
        .send()
        .await
        .map(|r| r.status().is_success())
        .unwrap_or(false)
}

// Not every workflow file uses every helper below - each test binary
// compiles its own copy of this module via `#[path]`, so unused ones would
// otherwise warn per-binary; `#[allow(dead_code)]` on each is intentional.

/// Ensure `name`'s dev account is registered on-chain as a storage provider
/// accepting primary agreements at `price_per_byte`, and return a connected,
/// signed [`ProviderClient`] for it.
///
/// Returns `None` when the chain or the provider node at [`PROVIDER_URL`] is
/// unreachable - callers should skip the test in that case.
#[allow(dead_code)]
pub async fn ensure_provider_registered(
    name: &str,
    price_per_byte: u128,
) -> Option<ProviderClient> {
    if !provider_reachable().await {
        return None;
    }

    let ss58 = dev_ss58(name);
    let mut provider = ProviderClient::new(chain_config(), ss58).ok()?;
    if provider.connect().await.is_err() {
        return None;
    }
    provider.set_dev_signer(name).ok()?;

    let already_registered = matches!(
        provider.get_provider_info(&dev_account(name)).await,
        Ok(Some(_))
    );

    if !already_registered {
        // A dev AccountId32 IS the sr25519 public key, so no keypair lookup needed.
        let public_key = (dev_account(name).as_ref() as &[u8]).to_vec();
        provider
            .register(PROVIDER_URL.to_string(), public_key, MIN_STAKE)
            .await
            .ok()?;
    }

    provider
        .update_settings(ProviderSettings {
            price_per_byte,
            min_duration: 10,
            max_duration: 100_000,
            accepting_primary: true,
            replica_sync_price: None,
            accepting_extensions: true,
            max_capacity: 0,
        })
        .await
        .ok()?;

    Some(provider)
}

/// Build a connected, signed [`AdminClient`] for `name`'s dev account.
///
/// Returns `None` when the chain is unreachable.
#[allow(dead_code)]
pub async fn admin_for(name: &str) -> Option<AdminClient> {
    let mut admin = AdminClient::new(chain_config(), dev_ss58(name)).ok()?;
    if admin.connect().await.is_err() {
        return None;
    }
    admin.set_dev_signer(name).ok()?;
    Some(admin)
}

/// Negotiate provider-signed terms for `owner_name` against the provider
/// node at [`PROVIDER_URL`], then redeem them via `establish_storage_agreement`.
/// Returns the new bucket id.
///
/// Returns `None` when the chain is unreachable - callers should skip the
/// test in that case.
#[allow(dead_code)]
pub async fn negotiate_and_establish(
    owner_name: &str,
    provider_ss58: &str,
    max_bytes: u64,
    duration: u32,
    price_per_byte: u128,
) -> Option<BucketId> {
    let admin = admin_for(owner_name).await?;

    let signed = ProviderClient::negotiate_terms(
        PROVIDER_URL,
        &NegotiateRequest {
            owner: dev_account(owner_name),
            max_bytes,
            duration,
            price_per_byte,
            bucket_id: None,
            replica_params: None,
        },
    )
    .await
    .ok()?;

    admin
        .establish_storage_agreement(provider_ss58.to_string(), signed.terms, signed.signature)
        .await
        .ok()
}

/// Read the chain's current best block number.
///
/// Returns `None` if the chain is unreachable.
#[allow(dead_code)]
pub async fn current_block() -> Option<u32> {
    let chain = SubstrateClient::connect(CHAIN_WS).await.ok()?;
    Some(chain.api().blocks().at_latest().await.ok()?.number())
}

/// Poll until the chain reaches block `target`, for tests that need to wait
/// out an expiry/window boundary (e.g. an agreement's `expires_at`).
///
/// Returns `None` if the chain is unreachable.
#[allow(dead_code)]
pub async fn wait_for_block(target: u32) -> Option<()> {
    let chain = SubstrateClient::connect(CHAIN_WS).await.ok()?;
    loop {
        let current = chain.api().blocks().at_latest().await.ok()?.number();
        if current >= target {
            return Some(());
        }
        tokio::time::sleep(std::time::Duration::from_millis(500)).await;
    }
}

/// Read an account's free balance (`System::Account(who).data.free`).
///
/// Returns `None` if the chain is unreachable.
#[allow(dead_code)]
pub async fn get_free(account: &AccountId32) -> Option<u128> {
    let chain = SubstrateClient::connect(CHAIN_WS).await.ok()?;
    let thunk = chain
        .api()
        .storage()
        .at_latest()
        .await
        .ok()?
        .fetch(&subxt::dynamic::storage(
            "System",
            "Account",
            vec![subxt::dynamic::Value::from_bytes(account.as_ref() as &[u8])],
        ))
        .await
        .ok()??;

    let value = thunk.to_value().ok()?;
    value.at("data")?.at("free")?.as_u128()
}
