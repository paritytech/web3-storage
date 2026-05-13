//! Integration tests for `ProviderClient`.
//!
//! Requires a running parachain at `ws://127.0.0.1:2222`:
//!
//! ```bash
//! just start-chain
//! cargo test --test provider_integration -- --nocapture
//! ```
//!
//! Tests are skipped (not failed) when the chain is unreachable.

mod common;

use common::{alice_provider, chain_guard, chain_setup, dev_account};
use storage_client::ProviderSettings;

/// After `chain_setup`, Alice is a registered provider, so `get_provider_info`
/// returns `Some(info)` with the settings established by setup.
#[tokio::test]
async fn test_get_provider_info_alice_registered() {
    let _guard = chain_guard().await;

    if chain_setup().await.is_none() {
        eprintln!("Chain not reachable — skipping test_get_provider_info_alice_registered");
        return;
    }

    let provider = alice_provider()
        .await
        .expect("chain was already reachable in chain_setup");

    let info = provider
        .get_provider_info(&dev_account("alice"))
        .await
        .expect("get_provider_info should not error")
        .expect("Alice should be registered after chain_setup");

    println!(
        "Alice ProviderInfo: multiaddr={} stake={} committed={} max_capacity={} price={} accepting_primary={}",
        info.multiaddr,
        info.stake,
        info.committed_bytes,
        info.max_capacity,
        info.price_per_byte,
        info.accepting_primary,
    );

    assert!(
        info.stake > 0,
        "stake should be positive after registration"
    );
    assert!(
        info.accepting_primary,
        "chain_setup configures accepting_primary=true"
    );
    assert_eq!(
        info.replica_sync_price, None,
        "chain_setup configures replica_sync_price=None"
    );
    assert!(
        info.max_capacity > 0,
        "chain_setup configures a non-zero max_capacity"
    );
    assert!(
        info.committed_bytes <= info.max_capacity,
        "committed_bytes ({}) should never exceed max_capacity ({})",
        info.committed_bytes,
        info.max_capacity
    );
}

/// Bob is never registered, so `get_provider_info` returns `Ok(None)`.
#[tokio::test]
async fn test_get_provider_info_unregistered_returns_none() {
    let _guard = chain_guard().await;

    if chain_setup().await.is_none() {
        eprintln!(
            "Chain not reachable — skipping test_get_provider_info_unregistered_returns_none"
        );
        return;
    }

    let provider = alice_provider()
        .await
        .expect("chain was already reachable in chain_setup");

    let info = provider
        .get_provider_info(&dev_account("bob"))
        .await
        .expect("get_provider_info should not error");

    assert!(info.is_none(), "Bob should not be a registered provider");
}

/// `list_pending_requests` should succeed and return a (possibly empty) list of
/// pending agreement requests targeting Alice.
#[tokio::test]
async fn test_list_pending_requests() {
    let _guard = chain_guard().await;

    if chain_setup().await.is_none() {
        eprintln!("Chain not reachable — skipping test_list_pending_requests");
        return;
    }

    let provider = alice_provider()
        .await
        .expect("chain was already reachable in chain_setup");

    let requests = provider
        .list_pending_requests()
        .await
        .expect("list_pending_requests should not error");

    println!("Alice has {} pending request(s)", requests.len());

    for r in &requests {
        assert!(
            !r.requester.is_empty(),
            "requester field should not be empty"
        );
        assert!(r.bucket_id > 0, "bucket_id should be non-zero");
        println!(
            "  bucket={} requester={} max_bytes={} payment_locked={} duration={}",
            r.bucket_id, r.requester, r.max_bytes, r.payment_locked, r.duration
        );
    }
}

/// `list_active_agreements` should succeed and return a (possibly empty) list.
#[tokio::test]
async fn test_list_active_agreements() {
    let _guard = chain_guard().await;

    if chain_setup().await.is_none() {
        eprintln!("Chain not reachable — skipping test_list_active_agreements");
        return;
    }

    let provider = alice_provider()
        .await
        .expect("chain was already reachable in chain_setup");

    let agreements = provider
        .list_active_agreements()
        .await
        .expect("list_active_agreements should not error");

    println!("Alice has {} active agreement(s)", agreements.len());

    for a in &agreements {
        assert!(!a.owner.is_empty(), "owner should not be empty");
        assert!(a.bucket_id > 0, "bucket_id should be non-zero");
        println!(
            "  bucket={} owner={} max_bytes={} expires_at={} primary={}",
            a.bucket_id, a.owner, a.max_bytes, a.expires_at, a.is_primary
        );
    }
}

/// `list_active_challenges` should succeed and return a (possibly empty) list.
#[tokio::test]
async fn test_list_active_challenges() {
    let _guard = chain_guard().await;

    if chain_setup().await.is_none() {
        eprintln!("Chain not reachable — skipping test_list_active_challenges");
        return;
    }

    let provider = alice_provider()
        .await
        .expect("chain was already reachable in chain_setup");

    let challenges = provider
        .list_active_challenges()
        .await
        .expect("list_active_challenges should not error");

    println!("Alice has {} active challenge(s)", challenges.len());

    for c in &challenges {
        assert!(c.bucket_id > 0, "bucket_id should be non-zero");
        println!(
            "  challenge=({},{}) bucket={} deadline={} leaf={} chunk={}",
            c.challenge_id.0,
            c.challenge_id.1,
            c.bucket_id,
            c.deadline,
            c.leaf_index,
            c.chunk_index
        );
    }
}

/// `get_stats` aggregates fields from `ProviderInfo`. Verify the aggregates are
/// self-consistent (e.g. `challenges_received >= challenges_failed`).
#[tokio::test]
async fn test_get_stats_consistent() {
    let _guard = chain_guard().await;

    if chain_setup().await.is_none() {
        eprintln!("Chain not reachable — skipping test_get_stats_consistent");
        return;
    }

    let provider = alice_provider()
        .await
        .expect("chain was already reachable in chain_setup");

    let stats = provider
        .get_stats()
        .await
        .expect("get_stats should not error");

    println!(
        "Alice stats: stake={} committed={} agreements_total={} extended={} challenges_received={} failed={} reputation={}",
        stats.stake,
        stats.committed_bytes,
        stats.agreements_total,
        stats.agreements_extended,
        stats.challenges_received,
        stats.challenges_failed,
        stats.reputation,
    );

    assert!(
        stats.stake > 0,
        "stake should be positive for registered provider"
    );
    assert!(stats.reputation <= 100, "reputation must be 0–100");
    assert!(
        stats.challenges_received >= stats.challenges_failed,
        "received ({}) should be >= failed ({})",
        stats.challenges_received,
        stats.challenges_failed
    );
}

/// `get_total_earnings` always returns `Ok(0)` — earnings aren't tracked on-chain.
#[tokio::test]
async fn test_get_total_earnings_returns_zero() {
    let _guard = chain_guard().await;

    if chain_setup().await.is_none() {
        eprintln!("Chain not reachable — skipping test_get_total_earnings_returns_zero");
        return;
    }

    let provider = alice_provider()
        .await
        .expect("chain was already reachable in chain_setup");

    let earnings = provider
        .get_total_earnings()
        .await
        .expect("get_total_earnings should not error");

    assert_eq!(
        earnings, 0,
        "earnings are not stored on-chain; get_total_earnings always returns 0"
    );
}

/// `get_capacity_info` returns Alice's stake and capacity-vs-committed breakdown.
#[tokio::test]
async fn test_get_capacity_info() {
    let _guard = chain_guard().await;

    if chain_setup().await.is_none() {
        eprintln!("Chain not reachable — skipping test_get_capacity_info");
        return;
    }

    let provider = alice_provider()
        .await
        .expect("chain was already reachable in chain_setup");

    let info = provider
        .get_capacity_info()
        .await
        .expect("get_capacity_info should not error");

    println!(
        "Capacity: committed={} available={} stake={}",
        info.committed_bytes, info.available_bytes, info.stake
    );

    assert!(
        info.stake > 0,
        "stake should be positive for registered provider"
    );
    // get_capacity_info uses saturating_sub internally; the sum is therefore bounded
    // by max_capacity. Re-query max_capacity through ProviderInfo and cross-check.
    let pi = provider
        .get_provider_info(&dev_account("alice"))
        .await
        .expect("get_provider_info should not error")
        .expect("Alice should be registered");
    assert_eq!(
        info.committed_bytes, pi.committed_bytes,
        "committed_bytes should match ProviderInfo"
    );
    assert_eq!(
        info.available_bytes,
        pi.max_capacity.saturating_sub(pi.committed_bytes),
        "available_bytes should equal max_capacity − committed_bytes"
    );
}

/// `get_reputation` delegates to `get_stats().reputation`. With no failed challenges
/// in fresh test state, reputation defaults to 100.
#[tokio::test]
async fn test_get_reputation_matches_stats() {
    let _guard = chain_guard().await;

    if chain_setup().await.is_none() {
        eprintln!("Chain not reachable — skipping test_get_reputation_matches_stats");
        return;
    }

    let provider = alice_provider()
        .await
        .expect("chain was already reachable in chain_setup");

    let reputation = provider
        .get_reputation()
        .await
        .expect("get_reputation should not error");
    let stats = provider
        .get_stats()
        .await
        .expect("get_stats should not error");

    println!("Alice's reputation: {reputation}");
    assert!(reputation <= 100, "reputation must be 0–100");
    assert_eq!(
        reputation, stats.reputation,
        "get_reputation should match get_stats().reputation"
    );
}

// ─── State-changing — submit extrinsics ───────────────────────────────────────

/// `update_settings` round-trip: bump `price_per_byte`, verify it took effect via
/// `get_provider_info`, then restore the prior value so other tests are unaffected.
#[tokio::test]
async fn test_update_settings_round_trip() {
    let _guard = chain_guard().await;

    if chain_setup().await.is_none() {
        eprintln!("Chain not reachable — skipping test_update_settings_round_trip");
        return;
    }

    let provider = alice_provider()
        .await
        .expect("chain was already reachable in chain_setup");

    let before = provider
        .get_provider_info(&dev_account("alice"))
        .await
        .expect("get_provider_info should not error")
        .expect("Alice should be registered");

    // Pick a price clearly different from the current one to make the assertion
    // meaningful regardless of what previous tests left behind.
    let new_price = before.price_per_byte.saturating_add(500_000).max(1_500_000);

    let new_settings = ProviderSettings {
        price_per_byte: new_price,
        min_duration: before.min_duration,
        max_duration: before.max_duration,
        accepting_primary: before.accepting_primary,
        replica_sync_price: before.replica_sync_price,
        accepting_extensions: before.accepting_extensions,
        max_capacity: before.max_capacity,
    };

    provider
        .update_settings(new_settings)
        .await
        .expect("update_settings should succeed");

    let after = provider
        .get_provider_info(&dev_account("alice"))
        .await
        .expect("get_provider_info should not error")
        .expect("Alice should still be registered");

    assert_eq!(
        after.price_per_byte, new_price,
        "price_per_byte should reflect the update"
    );

    // Restore prior settings so subsequent tests see Alice in her chain_setup state.
    let restored = ProviderSettings {
        price_per_byte: before.price_per_byte,
        min_duration: before.min_duration,
        max_duration: before.max_duration,
        accepting_primary: before.accepting_primary,
        replica_sync_price: before.replica_sync_price,
        accepting_extensions: before.accepting_extensions,
        max_capacity: before.max_capacity,
    };
    provider
        .update_settings(restored)
        .await
        .expect("restore update_settings should succeed");
}

/// `add_stake` increases the provider's stake by exactly the additional amount.
#[tokio::test]
async fn test_add_stake_increases_stake() {
    let _guard = chain_guard().await;

    if chain_setup().await.is_none() {
        eprintln!("Chain not reachable — skipping test_add_stake_increases_stake");
        return;
    }

    let provider = alice_provider()
        .await
        .expect("chain was already reachable in chain_setup");

    let stake_before = provider
        .get_stats()
        .await
        .expect("get_stats should succeed")
        .stake;

    let increment = 1_000_000_000_000u128; // 1 token.

    provider
        .add_stake(increment)
        .await
        .expect("add_stake should succeed");

    let stake_after = provider
        .get_stats()
        .await
        .expect("get_stats should succeed")
        .stake;

    assert_eq!(
        stake_after,
        stake_before + increment,
        "stake should increase by exactly {increment} (before={stake_before}, after={stake_after})"
    );
}

/// `accept_agreement` must fail when no pending request exists for the bucket —
/// the chain has nothing for the provider to accept.
#[tokio::test]
async fn test_accept_agreement_without_request_errors() {
    let _guard = chain_guard().await;

    let setup = match chain_setup().await {
        Some(s) => s,
        None => {
            eprintln!(
                "Chain not reachable — skipping test_accept_agreement_without_request_errors"
            );
            return;
        }
    };

    let provider = alice_provider()
        .await
        .expect("chain was already reachable in chain_setup");

    let result = provider.accept_agreement(setup.bucket_id).await;
    assert!(
        result.is_err(),
        "accept_agreement should fail when no pending request exists for the bucket"
    );
}
