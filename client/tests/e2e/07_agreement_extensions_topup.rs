// SPDX-License-Identifier: Apache-2.0

//! E2E Workflow 07 — Agreement Extensions & Top-up (happy-case port of
//! `examples/papi/e2e/07-agreement-extensions-topup.ts`).
//!
//! Accounts: `alice` (provider), `bob` (client).
//!
//! Covers extend duration, top up bytes, and block extensions. Failure/
//! permission-check cases from the JS reference (7.3-7.6, plus the
//! non-owner-extend feature demo 7.5) are out of scope for this happy-case
//! suite.
//!
//! Requires a running parachain (`just start-chain`) and a live provider
//! node registered on-chain (`just start-provider`); skipped (not failed)
//! when either is unreachable.

#[path = "common.rs"]
mod e2e_common;

use e2e_common::common::{chain_guard, dev_account, dev_ss58};
use e2e_common::{admin_for, ensure_provider_registered, negotiate_and_establish};
use storage_client::roles::admin::AgreementInfo;

/// Hex-encode a dev account the same way `AdminClient::list_bucket_agreements`
/// decodes the on-chain provider key (`0x`-prefixed raw bytes, not SS58).
fn provider_hex(name: &str) -> String {
    format!("0x{}", hex::encode(dev_account(name).as_ref() as &[u8]))
}

fn find_agreement(agreements: &[AgreementInfo], provider_hex: &str) -> AgreementInfo {
    agreements
        .iter()
        .find(|a| a.provider == provider_hex)
        .cloned()
        .expect("agreement with provider should exist")
}

#[tokio::test]
async fn agreement_extensions_and_topup() {
    let _guard = chain_guard().await;

    let Some(_provider) = ensure_provider_registered("alice", 1).await else {
        eprintln!("skipping: chain or provider node unreachable");
        return;
    };
    let alice_ss58 = dev_ss58("alice");
    let alice_hex = provider_hex("alice");
    let max_bytes = 1_048_576u64;
    let duration = 100u32;

    // 7.0 - Setup bucket + agreement.
    let Some(bucket_id) = negotiate_and_establish("bob", &alice_ss58, max_bytes, duration, 1).await
    else {
        eprintln!("skipping: chain unreachable");
        return;
    };

    let Some(bob) = admin_for("bob").await else {
        eprintln!("skipping: chain unreachable");
        return;
    };

    // 7.1 - Extend agreement duration.
    let before = find_agreement(
        &bob.list_bucket_agreements(bucket_id).await.expect("list"),
        &alice_hex,
    );
    let ext_duration = 200u32;
    bob.extend_agreement(
        bucket_id,
        alice_ss58.clone(),
        ext_duration,
        max_bytes as u128 * ext_duration as u128 * 10,
    )
    .await
    .expect("extend_agreement should succeed");
    let after = find_agreement(
        &bob.list_bucket_agreements(bucket_id).await.expect("list"),
        &alice_hex,
    );
    assert!(
        after.expires_at > before.expires_at,
        "expires_at should increase after extend"
    );

    // 7.2 - Top up bytes.
    let before = after;
    let extra_bytes = 524_288u64; // 512 KiB
    bob.top_up_agreement(
        bucket_id,
        alice_ss58.clone(),
        extra_bytes,
        extra_bytes as u128 * duration as u128 * 10,
    )
    .await
    .expect("top_up_agreement should succeed");
    let after = find_agreement(
        &bob.list_bucket_agreements(bucket_id).await.expect("list"),
        &alice_hex,
    );
    assert!(
        after.max_bytes > before.max_bytes,
        "max_bytes should increase after top-up"
    );

    // Block extensions is a provider-side call - the signer must be the
    // provider of the agreement (alice), not the bucket admin (bob).
    let Some(alice) = admin_for("alice").await else {
        eprintln!("skipping: chain unreachable");
        return;
    };
    alice
        .block_extensions(bucket_id, alice_ss58)
        .await
        .expect("block_extensions should succeed");
}
