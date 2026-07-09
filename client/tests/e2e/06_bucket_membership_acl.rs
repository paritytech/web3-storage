// SPDX-License-Identifier: Apache-2.0

//! E2E Workflow 06 — Bucket Membership ACL (happy-case port of
//! `examples/papi/e2e/06-bucket-membership-acl.ts`).
//!
//! Accounts: `alice` (provider), `dave` (admin), `eve` (writer), `ferdie` (reader).
//!
//! Covers add Writer, add Reader, promote Writer to Admin, self-demotion back
//! to Writer, and remove member. Failure/permission-check cases from the JS
//! reference (6.6-6.9) are out of scope for this happy-case suite.
//!
//! Requires a running parachain (`just start-chain`) and a live provider
//! node registered on-chain (`just start-provider`); skipped (not failed)
//! when either is unreachable.

#[path = "common.rs"]
mod e2e_common;

use e2e_common::common::{chain_guard, dev_account, dev_ss58};
use e2e_common::{admin_for, ensure_provider_registered, negotiate_and_establish};
use storage_primitives::Role;

/// Hex-encode a dev account the same way `AdminClient::get_bucket_info`
/// decodes on-chain member accounts (`0x`-prefixed raw bytes, not SS58).
fn member_hex(name: &str) -> String {
    format!("0x{}", hex::encode(dev_account(name).as_ref() as &[u8]))
}

#[tokio::test]
async fn bucket_membership_lifecycle() {
    let _guard = chain_guard().await;

    let Some(_provider) = ensure_provider_registered("alice", 1).await else {
        eprintln!("skipping: chain or provider node unreachable");
        return;
    };
    let alice_ss58 = dev_ss58("alice");
    let eve_ss58 = dev_ss58("eve");
    let ferdie_ss58 = dev_ss58("ferdie");

    // 6.0 - Create bucket: Dave redeems the signed terms, so he's admin.
    let Some(bucket_id) = negotiate_and_establish("dave", &alice_ss58, 1_048_576, 100, 1).await
    else {
        eprintln!("skipping: chain unreachable");
        return;
    };

    let Some(dave) = admin_for("dave").await else {
        eprintln!("skipping: chain unreachable");
        return;
    };

    // 6.1 - Add Eve as Writer.
    dave.add_member(bucket_id, eve_ss58.clone(), Role::Writer)
        .await
        .expect("add_member(Writer) should succeed");
    let bucket = dave
        .get_bucket_info(bucket_id)
        .await
        .expect("get_bucket_info");
    let eve_hex = member_hex("eve");
    assert!(
        bucket
            .members
            .iter()
            .any(|m| m.account == eve_hex && matches!(m.role, Role::Writer)),
        "Eve should be a Writer member"
    );

    // 6.2 - Add Ferdie as Reader.
    dave.add_member(bucket_id, ferdie_ss58.clone(), Role::Reader)
        .await
        .expect("add_member(Reader) should succeed");
    let bucket = dave
        .get_bucket_info(bucket_id)
        .await
        .expect("get_bucket_info");
    let ferdie_hex = member_hex("ferdie");
    assert!(
        bucket
            .members
            .iter()
            .any(|m| m.account == ferdie_hex && matches!(m.role, Role::Reader)),
        "Ferdie should be a Reader member"
    );

    // 6.3 - Promote Eve (Writer) to Admin.
    dave.update_member_role(bucket_id, eve_ss58.clone(), Role::Admin)
        .await
        .expect("update_member_role(Admin) should succeed");
    let bucket = dave
        .get_bucket_info(bucket_id)
        .await
        .expect("get_bucket_info");
    assert!(
        bucket
            .members
            .iter()
            .any(|m| m.account == eve_hex && matches!(m.role, Role::Admin)),
        "Eve should now be Admin"
    );

    // 6.4 - Eve self-demotes back to Writer (the pallet only allows self-demotion).
    let Some(eve) = admin_for("eve").await else {
        eprintln!("skipping: chain unreachable");
        return;
    };
    eve.update_member_role(bucket_id, eve_ss58, Role::Writer)
        .await
        .expect("self-demotion to Writer should succeed");
    let bucket = dave
        .get_bucket_info(bucket_id)
        .await
        .expect("get_bucket_info");
    assert!(
        bucket
            .members
            .iter()
            .any(|m| m.account == eve_hex && matches!(m.role, Role::Writer)),
        "Eve should be Writer again after self-demotion"
    );

    // 6.5 - Remove Ferdie.
    dave.remove_member(bucket_id, ferdie_ss58)
        .await
        .expect("remove_member should succeed");
    let bucket = dave
        .get_bucket_info(bucket_id)
        .await
        .expect("get_bucket_info");
    assert!(
        !bucket.members.iter().any(|m| m.account == ferdie_hex),
        "Ferdie should be gone from members"
    );
}
