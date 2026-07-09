// SPDX-License-Identifier: Apache-2.0

//! E2E Workflow 02 — Agreement Lifecycle (happy-case port of
//! `examples/papi/e2e/02-agreement-lifecycle.ts`).
//!
//! Accounts: `alice` (provider), `bob` (client).
//!
//! Agreements are opened by redeeming provider-signed terms via
//! `establish_storage_agreement` - the bucket is created atomically. Covers
//! establish, end (Pay), and end (Burn). Failure/rejection cases from the
//! JS reference (2.4-2.9) are out of scope for this happy-case suite.
//!
//! Requires a running parachain (`just start-chain`) and a live provider
//! node registered on-chain (`just start-provider`); skipped (not failed)
//! when either is unreachable.

#[path = "common.rs"]
mod e2e_common;

use e2e_common::common::{chain_guard, dev_ss58};
use e2e_common::{
    admin_for, ensure_provider_registered, get_free, negotiate_and_establish, wait_for_block,
};
use storage_primitives::EndAction;

/// 2.1 - Establish a storage agreement: the bucket is created and owned by
/// the redeeming account, with the max_bytes from the signed terms.
#[tokio::test]
async fn establish_storage_agreement() {
    let _guard = chain_guard().await;

    let Some(_provider) = ensure_provider_registered("alice", 1).await else {
        eprintln!("skipping: chain or provider node unreachable");
        return;
    };
    let alice_ss58 = dev_ss58("alice");
    let max_bytes = 1_048_576u64;

    let Some(bucket_id) = negotiate_and_establish("bob", &alice_ss58, max_bytes, 10, 1).await
    else {
        eprintln!("skipping: chain unreachable");
        return;
    };

    let Some(admin) = admin_for("bob").await else {
        eprintln!("skipping: chain unreachable");
        return;
    };
    let buckets = admin.list_my_buckets().await.expect("list_my_buckets");
    assert!(
        buckets.contains(&bucket_id),
        "bob should be a member of the newly established bucket"
    );

    let agreements = admin
        .list_bucket_agreements(bucket_id)
        .await
        .expect("list_bucket_agreements");
    assert!(
        agreements
            .iter()
            .any(|a| a.max_bytes == max_bytes && a.is_primary),
        "expected a primary agreement with max_bytes={max_bytes}"
    );
}

/// 2.2 - Ending an agreement with `Pay` after expiry should not reduce the
/// provider's balance (they get paid, or unchanged if payment is nil).
#[tokio::test]
async fn end_agreement_pay() {
    let _guard = chain_guard().await;

    let Some(_provider) = ensure_provider_registered("alice", 1).await else {
        eprintln!("skipping: chain or provider node unreachable");
        return;
    };
    let alice_ss58 = dev_ss58("alice");
    let alice_account = e2e_common::common::dev_account("alice");

    let Some(bucket_id) = negotiate_and_establish("bob", &alice_ss58, 1_048_576, 10, 1).await
    else {
        eprintln!("skipping: chain unreachable");
        return;
    };

    let Some(admin) = admin_for("bob").await else {
        eprintln!("skipping: chain unreachable");
        return;
    };
    let agreements = admin
        .list_bucket_agreements(bucket_id)
        .await
        .expect("list_bucket_agreements");
    let expires_at = agreements
        .iter()
        .find(|a| a.is_primary)
        .expect("primary agreement should exist")
        .expires_at;

    let Some(()) = wait_for_block(expires_at).await else {
        eprintln!("skipping: chain unreachable while waiting for expiry");
        return;
    };

    let Some(before) = get_free(&alice_account).await else {
        eprintln!("skipping: chain unreachable");
        return;
    };
    admin
        .terminate_agreement(bucket_id, alice_ss58.clone(), EndAction::Pay)
        .await
        .expect("terminate_agreement (Pay) should succeed");
    let Some(after) = get_free(&alice_account).await else {
        eprintln!("skipping: chain unreachable");
        return;
    };
    assert!(
        after >= before,
        "provider should be paid (or unchanged) on Pay"
    );
}

/// 2.3 - Ending an agreement with `Burn` after expiry succeeds. Uses a large
/// `max_bytes` so `payment_locked` clears the existential deposit (mirrors
/// the JS reference's 1 GiB choice).
#[tokio::test]
async fn end_agreement_burn() {
    let _guard = chain_guard().await;

    let Some(_provider) = ensure_provider_registered("alice", 1).await else {
        eprintln!("skipping: chain or provider node unreachable");
        return;
    };
    let alice_ss58 = dev_ss58("alice");
    let burn_max_bytes = 1_073_741_824u64; // 1 GiB

    let Some(bucket_id) = negotiate_and_establish("bob", &alice_ss58, burn_max_bytes, 10, 1).await
    else {
        eprintln!("skipping: chain unreachable");
        return;
    };

    let Some(admin) = admin_for("bob").await else {
        eprintln!("skipping: chain unreachable");
        return;
    };
    let agreements = admin
        .list_bucket_agreements(bucket_id)
        .await
        .expect("list_bucket_agreements");
    let expires_at = agreements
        .iter()
        .find(|a| a.is_primary)
        .expect("primary agreement should exist")
        .expires_at;

    let Some(()) = wait_for_block(expires_at).await else {
        eprintln!("skipping: chain unreachable while waiting for expiry");
        return;
    };

    admin
        .terminate_agreement(bucket_id, alice_ss58, EndAction::Burn { burn_percent: 100 })
        .await
        .expect("terminate_agreement (Burn) should succeed");
}
