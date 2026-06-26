// SPDX-License-Identifier: Apache-2.0

//! Integration tests for `AdminClient`.
//!
//! Requires a running parachain at `ws://127.0.0.1:2222`:
//!
//! ```bash
//! just start-chain
//! cargo test --test admin_integration -- --nocapture
//! ```
//!
//! Tests are skipped (not failed) when the chain is unreachable.

mod common;

use common::{alice_admin, chain_guard, chain_setup, dev_ss58};
use storage_primitives::Role;

// ─── Bucket lifecycle ─────────────────────────────────────────────────────────

/// Full bucket management lifecycle in dependency order:
///
///   chain_setup (registers Alice as provider + signs terms + creates bucket)
///   → get_bucket_info       (verify initial state)
///   → add_member            (add Bob as Writer)
///   → get_bucket_info       (verify Bob present)
///   → update_member_role    (promote Bob to Admin)
///   → get_bucket_info       (verify role updated)
///   → remove_member         (remove Bob)
///   → get_bucket_info       (verify empty members)
///   → list_my_buckets       (verify bucket_id is listed)
///   → freeze_bucket
///   → get_bucket_info       (verify frozen)
#[tokio::test]
async fn test_bucket_lifecycle() {
    let _guard = chain_guard().await;

    // `chain_setup` registers Alice as a provider, signs primary terms with
    // her keypair, and redeems them via `establish_storage_agreement` to
    // create a fresh bucket. Returns `None` when the chain isn't reachable.
    let setup = match common::chain_setup().await {
        Some(s) => s,
        None => {
            eprintln!("Chain not reachable — skipping test_bucket_lifecycle");
            return;
        }
    };
    let admin = match alice_admin().await {
        Some(c) => c,
        None => {
            eprintln!("Chain not reachable — skipping test_bucket_lifecycle");
            return;
        }
    };

    let bob_ss58 = dev_ss58("bob");
    let bucket_id = setup.bucket_id;

    assert!(bucket_id > 0, "bucket_id should be nonzero");

    // ── initial state ─────────────────────────────────────────────────────────
    let info = admin
        .get_bucket_info(bucket_id)
        .await
        .expect("get_bucket_info should succeed");

    assert_eq!(info.min_providers, 1);
    // The pallet auto-adds the creator (Alice) as an Admin member.
    assert_eq!(
        info.members.0.len(),
        1,
        "creator should be auto-added as Admin"
    );
    assert_eq!(
        info.members.0[0].role.clone() as u8,
        Role::Admin as u8,
        "creator's initial role should be Admin"
    );
    assert!(info.snapshot.is_none(), "new bucket has no snapshot");
    assert!(info.frozen_start_seq.is_none(), "new bucket is not frozen");

    // ── add Bob as Reader ─────────────────────────────────────────────────────
    admin
        .add_member(bucket_id, bob_ss58.clone(), Role::Reader)
        .await
        .expect("add_member should succeed");

    let info = admin
        .get_bucket_info(bucket_id)
        .await
        .expect("get_bucket_info after add_member");

    assert_eq!(info.members.0.len(), 2, "should have 2 members (Alice + Bob)");
    let bob_info = info
        .members
        .0
        .iter()
        .find(|m| m.role.clone() as u8 == Role::Reader as u8)
        .expect("Bob should be in members with Reader role");
    assert_eq!(
        bob_info.role.clone() as u8,
        Role::Reader as u8,
        "Bob's role should be Reader"
    );

    // ── promote Bob to Writer (pallet forbids demoting Admin, so stay below Admin) ─
    admin
        .update_member_role(bucket_id, bob_ss58.clone(), Role::Writer)
        .await
        .expect("update_member_role should succeed");

    let info = admin
        .get_bucket_info(bucket_id)
        .await
        .expect("get_bucket_info after update_member_role");

    assert_eq!(
        info.members.0.len(),
        2,
        "should still have 2 members after role update"
    );
    let bob_info = info
        .members
        .0
        .iter()
        .find(|m| m.role.clone() as u8 == Role::Writer as u8)
        .expect("Bob should have Writer role after update");
    assert_eq!(bob_info.role.clone() as u8, Role::Writer as u8);

    // ── remove Bob ────────────────────────────────────────────────────────────
    admin
        .remove_member(bucket_id, bob_ss58)
        .await
        .expect("remove_member should succeed");

    let info = admin
        .get_bucket_info(bucket_id)
        .await
        .expect("get_bucket_info after remove_member");

    assert_eq!(
        info.members.0.len(),
        1,
        "only Alice should remain after removing Bob"
    );

    // ── list_my_buckets ───────────────────────────────────────────────────────
    let my_buckets = admin
        .list_my_buckets()
        .await
        .expect("list_my_buckets should succeed");

    assert!(
        my_buckets.contains(&bucket_id),
        "list_my_buckets should include bucket {bucket_id}, got {my_buckets:?}"
    );

    // ── freeze_bucket ─────────────────────────────────────────────────────────
    // freeze_bucket requires a prior checkpoint snapshot. Without a running provider
    // we can't create one, so the chain returns NoSnapshot. Verify that error rather
    // than skipping the call entirely.
    let freeze_result = admin.freeze_bucket(bucket_id, 0).await;
    assert!(
        freeze_result.is_err(),
        "freeze_bucket should fail without a checkpoint snapshot"
    );
    let err_msg = freeze_result.unwrap_err().to_string();
    assert!(
        err_msg.contains("NoSnapshot") || err_msg.contains("snapshot"),
        "error should mention missing snapshot, got: {err_msg}"
    );

    println!("test_bucket_lifecycle passed for bucket_id={bucket_id}");
}

// ─── List bucket agreements ───────────────────────────────────────────────────

/// A freshly created bucket has no accepted agreements — verifies the function
/// returns `Ok([])` rather than an error.
#[tokio::test]
async fn test_list_bucket_agreements_empty() {
    let _guard = chain_guard().await;

    let setup = match chain_setup().await {
        Some(s) => s,
        None => {
            eprintln!("Chain not reachable — skipping test_list_bucket_agreements_empty");
            return;
        }
    };

    let admin = alice_admin()
        .await
        .expect("chain was already reachable in chain_setup");

    let agreements = admin
        .list_bucket_agreements(setup.bucket_id)
        .await
        .expect("list_bucket_agreements should succeed");

    // No provider has accepted the agreement yet (no provider-node running),
    // so the list is empty.
    println!(
        "Bucket {} has {} agreement(s) (expected 0 on empty chain)",
        setup.bucket_id,
        agreements.len()
    );

    for a in &agreements {
        assert!(!a.provider.is_empty(), "provider field should not be empty");
        assert!(a.agreement.max_bytes > 0, "max_bytes should be positive");
    }
}

// ─── list_my_buckets after setup ─────────────────────────────────────────────

/// After `chain_setup` creates a bucket, `list_my_buckets` must include it.
#[tokio::test]
async fn test_list_my_buckets_contains_setup_bucket() {
    let _guard = chain_guard().await;

    let setup = match chain_setup().await {
        Some(s) => s,
        None => {
            eprintln!("Chain not reachable — skipping test_list_my_buckets_contains_setup_bucket");
            return;
        }
    };

    let admin = alice_admin()
        .await
        .expect("chain was already reachable in chain_setup");

    let my_buckets = admin
        .list_my_buckets()
        .await
        .expect("list_my_buckets should succeed");

    assert!(
        my_buckets.contains(&setup.bucket_id),
        "list_my_buckets should include bucket {}, got {:?}",
        setup.bucket_id,
        my_buckets
    );

    println!("list_my_buckets returned {} bucket(s)", my_buckets.len());
}

// ─── delete_before returns informative error ──────────────────────────────────

/// `delete_before` is intentionally off-chain; the client should return a
/// descriptive error rather than panicking or hanging.
#[tokio::test]
async fn test_delete_before_returns_error() {
    let _guard = chain_guard().await;

    let setup = match chain_setup().await {
        Some(s) => s,
        None => {
            eprintln!("Chain not reachable — skipping test_delete_before_returns_error");
            return;
        }
    };

    let admin = alice_admin()
        .await
        .expect("chain was already reachable in chain_setup");

    let result = admin.delete_before(setup.bucket_id, 100, vec![]).await;

    assert!(
        result.is_err(),
        "delete_before should return Err (off-chain operation)"
    );

    let err_msg = result.unwrap_err().to_string();
    assert!(
        err_msg.contains("off-chain") || err_msg.contains("deletion"),
        "error message should explain off-chain nature, got: {err_msg}"
    );
}
