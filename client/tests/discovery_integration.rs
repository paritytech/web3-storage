// SPDX-License-Identifier: Apache-2.0

//! Integration tests for `DiscoveryClient`.
//!
//! All tests are read-only (no extrinsics submitted). They use `chain_setup`
//! to ensure at least one provider (Alice) is registered before querying.
//!
//! Requires a running parachain at `ws://127.0.0.1:2222`:
//!
//! ```bash
//! just start-chain
//! cargo test --test discovery_integration -- --nocapture
//! ```
//!
//! Tests are skipped (not failed) when the chain is unreachable.

mod common;

use common::{chain_guard, chain_setup, dev_discovery};
use sp_core::crypto::Ss58Codec;
use storage_subxt::storage_paseo_runtime::api::runtime_types::pallet_storage_provider::runtime_api::StorageRequirements;

fn account_bytes_to_ss58(bytes: &[u8]) -> String {
    let arr: [u8; 32] = bytes.try_into().unwrap_or_default();
    sp_runtime::AccountId32::from(arr).to_ss58check()
}

// ─── list_providers ───────────────────────────────────────────────────────────

/// After `chain_setup` registers Alice, `list_providers` returns at least one entry.
#[tokio::test]
async fn test_list_providers() {
    let _guard = chain_guard().await;

    let setup = match chain_setup().await {
        Some(s) => s,
        None => {
            eprintln!("Chain not reachable — skipping test_list_providers");
            return;
        }
    };

    let discovery = dev_discovery()
        .await
        .expect("chain was already reachable in chain_setup");

    let providers = discovery
        .list_providers(0, 100)
        .await
        .expect("list_providers should not error");

    assert!(
        !providers.is_empty(),
        "at least Alice should be registered after chain_setup"
    );

    let accounts: Vec<&str> = providers.iter().map(|(a, _)| a.as_str()).collect();
    assert!(
        accounts.contains(&setup.alice_ss58.as_str()),
        "Alice ({}) should appear in list_providers, got {:?}",
        setup.alice_ss58,
        accounts
    );

    for (account, info) in &providers {
        assert!(!account.is_empty(), "account should not be empty");
        println!(
            "  provider={} stake={} capacity={} price={} accepting={}",
            account,
            info.stake,
            info.settings.max_capacity,
            info.settings.price_per_byte,
            info.settings.accepting_primary
        );
    }
}

/// Both pages have at most 1 item; page1 differs from page0 when ≥ 2 providers exist.
#[tokio::test]
async fn test_list_providers_pagination() {
    let _guard = chain_guard().await;

    let _setup = match chain_setup().await {
        Some(s) => s,
        None => {
            eprintln!("Chain not reachable — skipping test_list_providers_pagination");
            return;
        }
    };

    let discovery = dev_discovery()
        .await
        .expect("chain was already reachable in chain_setup");

    let page0 = discovery
        .list_providers(0, 1)
        .await
        .expect("list_providers page 0");
    let page1 = discovery
        .list_providers(1, 1)
        .await
        .expect("list_providers page 1");

    assert!(page0.len() <= 1, "page0 should have at most 1 item");
    assert!(page1.len() <= 1, "page1 should have at most 1 item");

    if page0.len() == 1 && page1.len() == 1 {
        assert_ne!(
            page0[0].0, page1[0].0,
            "page0 and page1 should return different providers when there are ≥ 2"
        );
    }

    println!("page0 len={}, page1 len={}", page0.len(), page1.len());
}

// ─── get_provider_info ────────────────────────────────────────────────────────

/// Returns `Some` for Alice (registered by chain_setup) and `None` for unknown.
#[tokio::test]
async fn test_get_provider_info() {
    let _guard = chain_guard().await;

    let setup = match chain_setup().await {
        Some(s) => s,
        None => {
            eprintln!("Chain not reachable — skipping test_get_provider_info");
            return;
        }
    };

    let discovery = dev_discovery()
        .await
        .expect("chain was already reachable in chain_setup");

    let info = discovery
        .get_provider_info(&setup.alice_ss58)
        .await
        .expect("get_provider_info should not error")
        .expect("Alice should be found after chain_setup");

    assert!(
        info.settings.accepting_primary,
        "Alice should be accepting primary"
    );
    assert!(info.stake > 0, "Alice's stake should be > 0");

    println!(
        "get_provider_info OK: account={} stake={} accepting={}",
        setup.alice_ss58, info.stake, info.settings.accepting_primary
    );

    // Zero account — must return None.
    let unknown = "5C4hrfjw9DjXZTzV3MwzrrAr9P1MJhSrvWGWqi1eSuyUpnhM";
    let none_result = discovery
        .get_provider_info(unknown)
        .await
        .expect("get_provider_info for unknown should not error");

    assert!(none_result.is_none(), "unknown account should return None");
}

// ─── can_provider_accept ──────────────────────────────────────────────────────

/// Alice is accepting after chain_setup:
///   - `can_provider_accept(alice, 0)` → true
///   - `can_provider_accept(alice, capacity + 1)` → false
#[tokio::test]
async fn test_can_provider_accept() {
    let _guard = chain_guard().await;

    let setup = match chain_setup().await {
        Some(s) => s,
        None => {
            eprintln!("Chain not reachable — skipping test_can_provider_accept");
            return;
        }
    };

    let discovery = dev_discovery()
        .await
        .expect("chain was already reachable in chain_setup");

    let can_accept_zero = discovery
        .can_provider_accept(&setup.alice_ss58, 0)
        .await
        .expect("can_provider_accept should not error");

    assert!(
        can_accept_zero,
        "Alice should accept 0 bytes after chain_setup"
    );

    // chain_setup registers Alice with 10 GiB capacity.
    let over_capacity = 10u64 * 1024 * 1024 * 1024 + 1;
    let can_accept_over = discovery
        .can_provider_accept(&setup.alice_ss58, over_capacity)
        .await
        .expect("can_provider_accept should not error");

    assert!(
        !can_accept_over,
        "Alice should not accept {over_capacity} bytes (over 10 GiB capacity)"
    );

    println!(
        "can_provider_accept(0)={can_accept_zero}, can_provider_accept({over_capacity})={can_accept_over}"
    );
}

// ─── providers_with_capacity ─────────────────────────────────────────────────

/// `providers_with_capacity(0)` includes Alice; `providers_with_capacity(u64::MAX)` excludes all.
#[tokio::test]
async fn test_providers_with_capacity() {
    let _guard = chain_guard().await;

    let setup = match chain_setup().await {
        Some(s) => s,
        None => {
            eprintln!("Chain not reachable — skipping test_providers_with_capacity");
            return;
        }
    };

    let discovery = dev_discovery()
        .await
        .expect("chain was already reachable in chain_setup");

    let all_accepting = discovery
        .providers_with_capacity(0, 0, 100)
        .await
        .expect("providers_with_capacity(0) should not error");

    let accounts: Vec<&str> = all_accepting.iter().map(|(a, _)| a.as_str()).collect();
    assert!(
        accounts.contains(&setup.alice_ss58.as_str()),
        "Alice should appear in providers_with_capacity(0)"
    );

    for (account, info) in &all_accepting {
        assert!(
            info.settings.accepting_primary || info.settings.replica_sync_price.is_some(),
            "provider {account} is not accepting any agreement kind"
        );
    }

    let none_match = discovery
        .providers_with_capacity(u64::MAX, 0, 100)
        .await
        .expect("providers_with_capacity(u64::MAX) should not error");

    assert!(
        none_match.is_empty(),
        "no provider should match u64::MAX bytes"
    );

    println!(
        "providers_with_capacity(0)={} providers_with_capacity(u64::MAX)={}",
        all_accepting.len(),
        none_match.len()
    );
}

// ─── find_providers ───────────────────────────────────────────────────────────

/// Results are sorted by score descending, and Alice appears with loose requirements.
#[tokio::test]
async fn test_find_providers_sorted_by_score() {
    let _guard = chain_guard().await;

    let setup = match chain_setup().await {
        Some(s) => s,
        None => {
            eprintln!("Chain not reachable — skipping test_find_providers_sorted_by_score");
            return;
        }
    };

    let discovery = dev_discovery()
        .await
        .expect("chain was already reachable in chain_setup");

    let requirements = StorageRequirements {
        bytes_needed: 0,
        min_duration: 0,
        max_price_per_byte: u128::MAX,
        primary_only: true,
    };

    let matched = discovery
        .find_providers(requirements, 100)
        .await
        .expect("find_providers should not error");

    let accounts: Vec<String> = matched
        .iter()
        .map(|m| account_bytes_to_ss58(&m.account))
        .collect();
    assert!(
        accounts.iter().any(|a| a == &setup.alice_ss58),
        "Alice should be in find_providers results"
    );

    let scores: Vec<u8> = matched.iter().map(|m| m.match_score).collect();
    let mut sorted = scores.clone();
    sorted.sort_by(|a, b| b.cmp(a));
    assert_eq!(
        scores, sorted,
        "find_providers results must be sorted by score descending"
    );

    println!("find_providers returned {} match(es)", matched.len());
    for m in &matched {
        println!(
            "  account={} score={} price={}",
            account_bytes_to_ss58(&m.account),
            m.match_score,
            m.info.price_per_byte
        );
    }
}

/// Alice charges `1_000_000` price_per_byte (set in chain_setup), so she must
/// NOT appear when `max_price_per_byte = 0`.
#[tokio::test]
async fn test_find_providers_price_filter_excludes_alice() {
    let _guard = chain_guard().await;

    let setup = match chain_setup().await {
        Some(s) => s,
        None => {
            eprintln!(
                "Chain not reachable — skipping test_find_providers_price_filter_excludes_alice"
            );
            return;
        }
    };

    let discovery = dev_discovery()
        .await
        .expect("chain was already reachable in chain_setup");

    let requirements = StorageRequirements {
        bytes_needed: 0,
        min_duration: 0,
        max_price_per_byte: 0,
        primary_only: true,
    };

    let matched = discovery
        .find_providers(requirements, 100)
        .await
        .expect("find_providers with price=0 should not error");

    let accounts: Vec<String> = matched
        .iter()
        .map(|m| account_bytes_to_ss58(&m.account))
        .collect();
    assert!(
        !accounts.iter().any(|a| a == &setup.alice_ss58),
        "Alice (price=1_000_000) should not match max_price_per_byte=0"
    );

    for m in &matched {
        assert_eq!(
            m.info.price_per_byte, 0,
            "all matched providers should have price_per_byte=0"
        );
    }

    println!("free providers: {}", matched.len());
}
