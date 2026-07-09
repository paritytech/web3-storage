// SPDX-License-Identifier: Apache-2.0

//! E2E Workflow 08 — Provider Deregistration (happy-case port of
//! `examples/papi/e2e/08-provider-deregistration.ts`).
//!
//! Account: `ferdie` (bare provider registration, no HTTP node needed - the
//! announce/cancel paths are pure chain state).
//!
//! Note: `DeregisterAnnouncementPeriod` is 48 hours in production config -
//! too long for an e2e test, so `complete_deregister` is deliberately
//! excluded (the JS reference notes the same). Covers announce
//! (`deregister`) and cancel (`cancel_deregister`, the G2 gap function).
//! The "active agreements block deregister" failure case is out of scope.
//!
//! Requires a running parachain (`just start-chain`); does not need the
//! provider node since ferdie never serves HTTP. Skipped (not failed) when
//! the chain is unreachable.

#[path = "common.rs"]
mod e2e_common;

use e2e_common::common::{chain_guard, dev_account, dev_ss58, MIN_STAKE};
use storage_client::{ClientConfig, ProviderClient};

fn chain_config() -> ClientConfig {
    ClientConfig {
        chain_ws_url: e2e_common::common::CHAIN_WS.to_string(),
        provider_urls: vec![],
        timeout_secs: 30,
        enable_retries: false,
    }
}

#[tokio::test]
async fn deregister_announce_and_cancel() {
    let _guard = chain_guard().await;

    let ferdie_ss58 = dev_ss58("ferdie");
    let mut provider = match ProviderClient::new(chain_config(), ferdie_ss58) {
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
    provider.set_dev_signer("ferdie").expect("dev signer");

    // Bare registration (skip if a previous run already registered ferdie).
    let already_registered = matches!(
        provider.get_provider_info(&dev_account("ferdie")).await,
        Ok(Some(_))
    );
    if !already_registered {
        let public_key = (dev_account("ferdie").as_ref() as &[u8]).to_vec();
        provider
            .register("/ip4/127.0.0.1/tcp/4444".to_string(), public_key, MIN_STAKE)
            .await
            .expect("register should succeed");
    }
    // If a previous cancel left deregistration announced (shouldn't happen,
    // but keep the test idempotent across reruns against a persistent chain).
    let info = provider
        .get_provider_info(&dev_account("ferdie"))
        .await
        .expect("get_provider_info")
        .expect("ferdie should be registered");
    if info.deregister_at.is_some() {
        provider
            .cancel_deregister()
            .await
            .expect("clearing a stale announcement should succeed");
    }

    // 8.1 - Announce deregistration: accepting_primary flips false.
    provider
        .deregister()
        .await
        .expect("deregister should succeed");
    let info = provider
        .get_provider_info(&dev_account("ferdie"))
        .await
        .expect("get_provider_info")
        .expect("ferdie should still be registered (announced, not completed)");
    assert!(
        !info.accepting_primary,
        "accepting_primary should be false after deregister announcement"
    );
    assert!(
        info.deregister_at.is_some(),
        "deregister_at should be set after announcement"
    );

    // 8.2 - Cancel deregistration: accepting_primary restored, deregister_at cleared.
    provider
        .cancel_deregister()
        .await
        .expect("cancel_deregister should succeed");
    let info = provider
        .get_provider_info(&dev_account("ferdie"))
        .await
        .expect("get_provider_info")
        .expect("ferdie should still be registered");
    assert!(
        info.accepting_primary,
        "accepting_primary should be restored to true after cancel"
    );
    assert!(
        info.deregister_at.is_none(),
        "deregister_at should be cleared after cancel"
    );
}
