// SPDX-License-Identifier: Apache-2.0

//! E2E Workflow 01 — Provider Registration (happy-case port of
//! `examples/papi/e2e/01-provider-registration.ts`).
//!
//! Account: `charlie` (provider) - kept distinct from the accounts other
//! workflow files register (`alice`), so this test exercises a genuine
//! first-time registration when run against a fresh chain.
//!
//! Covers register, update settings, add stake, update multiaddr (the G1
//! gap function), settings-take-effect-for-matching, accepting_primary=false
//! blocking matching, and max_capacity=0 (unlimited). Failure/rejection
//! cases from the JS reference (1.6-1.10) are out of scope for this
//! happy-case suite.
//!
//! Requires a running parachain (`just start-chain`) and a live provider
//! node registered on-chain (`just start-provider`); skipped (not failed)
//! when unreachable.

#[path = "common.rs"]
mod e2e_common;

use e2e_common::common::dev_ss58;
use e2e_common::PROVIDER_URL;
use storage_client::{
    ClientConfig, DiscoveryClient, ProviderClient, ProviderSettings, StorageRequirements,
};

fn chain_config() -> ClientConfig {
    ClientConfig {
        chain_ws_url: e2e_common::common::CHAIN_WS.to_string(),
        provider_urls: vec![PROVIDER_URL.to_string()],
        timeout_secs: 30,
        enable_retries: false,
    }
}

#[tokio::test]
async fn provider_registration_lifecycle() {
    let _guard = e2e_common::common::chain_guard().await;

    let charlie_ss58 = dev_ss58("charlie");
    let mut provider = match ProviderClient::new(chain_config(), charlie_ss58.clone()) {
        Ok(p) => p,
        Err(e) => {
            eprintln!("skipping: {e}");
            return;
        }
    };
    if provider.connect().await.is_err() {
        eprintln!("skipping: chain unreachable");
        return;
    }
    provider.set_dev_signer("charlie").expect("dev signer");

    // 1.1 - Register (skip gracefully if a previous run already registered
    // charlie on this chain, mirroring the JS reference's SKIPPED branch).
    let already_registered = matches!(
        provider
            .get_provider_info(&e2e_common::common::dev_account("charlie"))
            .await,
        Ok(Some(_))
    );
    if !already_registered {
        let public_key = (e2e_common::common::dev_account("charlie").as_ref() as &[u8]).to_vec();
        provider
            .register(
                PROVIDER_URL.to_string(),
                public_key,
                e2e_common::common::MIN_STAKE,
            )
            .await
            .expect("register should succeed");
    }

    // 1.2 - Update settings.
    provider
        .update_settings(ProviderSettings {
            min_duration: 10,
            max_duration: 100_000,
            price_per_byte: 2,
            accepting_primary: true,
            replica_sync_price: None,
            accepting_extensions: true,
            max_capacity: 0,
        })
        .await
        .expect("update_settings should succeed");
    let stored = provider
        .get_provider_info(&e2e_common::common::dev_account("charlie"))
        .await
        .expect("get_provider_info")
        .expect("provider should be registered");
    assert_eq!(stored.price_per_byte, 2, "price_per_byte should be 2");

    // 1.3 - Add stake.
    let before_stake = stored.stake;
    provider
        .add_stake(500 * 1_000_000_000_000u128)
        .await
        .expect("add_stake should succeed");
    let after_stake = provider
        .get_provider_info(&e2e_common::common::dev_account("charlie"))
        .await
        .expect("get_provider_info")
        .expect("provider should be registered")
        .stake;
    assert!(after_stake > before_stake, "stake should have increased");

    // 1.4 - Update multiaddr (G1), then restore the original so later
    // reruns / other tests still find charlie at his real endpoint.
    provider
        .update_multiaddr("/ip4/127.0.0.1/tcp/9999".to_string())
        .await
        .expect("update_multiaddr should succeed");
    let updated = provider
        .get_provider_info(&e2e_common::common::dev_account("charlie"))
        .await
        .expect("get_provider_info")
        .expect("provider should be registered");
    assert_eq!(updated.multiaddr, "/ip4/127.0.0.1/tcp/9999");
    let original_port = PROVIDER_URL.rsplit(':').next().unwrap_or("3333");
    provider
        .update_multiaddr(format!("/ip4/127.0.0.1/tcp/{original_port}"))
        .await
        .expect("restoring multiaddr should succeed");

    // 1.5 - Settings take effect for matching: with accepting_primary=true
    // and price 2, charlie is a perfect match for a request priced at 10.
    let Some(mut discovery) = DiscoveryClient::new(chain_config()).ok() else {
        eprintln!("skipping: could not build DiscoveryClient");
        return;
    };
    if discovery.connect().await.is_err() {
        eprintln!("skipping: chain unreachable");
        return;
    }
    let matches = discovery
        .find_providers(
            StorageRequirements {
                bytes_needed: 1_048_576,
                min_duration: 50,
                max_price_per_byte: 10,
                primary_only: true,
            },
            50,
        )
        .await
        .expect("find_providers should succeed");
    let entry = matches
        .iter()
        .find(|m| m.account == charlie_ss58)
        .expect("charlie should appear in matching results");
    assert_eq!(entry.match_score, 100, "charlie should be a perfect match");
    assert!(
        entry.partial_reason.is_none(),
        "no partial-match reason expected"
    );

    // 1.11 - accepting_primary=false blocks matching (score 0, NotAccepting).
    provider
        .update_settings(ProviderSettings {
            min_duration: 10,
            max_duration: 100_000,
            price_per_byte: 2,
            accepting_primary: false,
            replica_sync_price: None,
            accepting_extensions: true,
            max_capacity: 0,
        })
        .await
        .expect("update_settings(accepting_primary=false) should succeed");
    let matches = discovery
        .find_providers(
            StorageRequirements {
                bytes_needed: 1_048_576,
                min_duration: 50,
                max_price_per_byte: 10,
                primary_only: true,
            },
            50,
        )
        .await
        .expect("find_providers should succeed");
    let entry = matches
        .iter()
        .find(|m| m.account == charlie_ss58)
        .expect("charlie should still be listed");
    assert_eq!(
        entry.match_score, 0,
        "non-accepting provider should score 0"
    );
    assert!(
        matches!(
            entry.partial_reason,
            Some(storage_client::discovery::PartialMatchReason::NotAccepting)
        ),
        "reason should be NotAccepting"
    );
    // Restore accepting_primary for anything downstream sharing this chain.
    provider
        .update_settings(ProviderSettings {
            min_duration: 10,
            max_duration: 100_000,
            price_per_byte: 2,
            accepting_primary: true,
            replica_sync_price: None,
            accepting_extensions: true,
            max_capacity: 0,
        })
        .await
        .expect("restoring accepting_primary should succeed");

    // 1.12 - max_capacity=0 means unlimited (already proven able to match at
    // any size by 1.5; just confirm the stored value).
    let stored = provider
        .get_provider_info(&e2e_common::common::dev_account("charlie"))
        .await
        .expect("get_provider_info")
        .expect("provider should be registered");
    assert_eq!(
        stored.max_capacity, 0,
        "max_capacity should be 0 (unlimited)"
    );
}
