//! Integration tests for `ChallengerClient`.
//!
//! Requires a running parachain at `ws://127.0.0.1:2222`:
//!
//! ```bash
//! just start-chain
//! cargo test --test challenger_integration -- --nocapture
//! ```
//!
//! Tests are skipped (not failed) when the chain is unreachable.

mod common;

use common::{alice_challenger, chain_guard, chain_setup};

// ─── Read-only — no extrinsics submitted ──────────────────────────────────────

/// `list_my_challenges` should succeed and return a (possibly empty) list
/// even when Alice has submitted no challenges yet.
#[tokio::test]
async fn test_list_my_challenges_empty() {
    let _guard = chain_guard().await;

    let challenger = match alice_challenger().await {
        Some(c) => c,
        None => {
            eprintln!("Chain not reachable — skipping test_list_my_challenges_empty");
            return;
        }
    };

    let challenges = challenger
        .list_my_challenges()
        .await
        .expect("list_my_challenges should not error");

    println!("Alice has {} challenge(s) on-chain", challenges.len());

    for c in &challenges {
        assert!(!c.provider.is_empty(), "provider field should not be empty");
        println!(
            "  deadline={} index={} bucket={} provider={}",
            c.challenge_id.deadline, c.challenge_id.index, c.bucket_id, c.provider
        );
    }
}

/// `get_challenge_stats` aggregates `list_my_challenges`: total ≥ successful.
#[tokio::test]
async fn test_get_challenge_stats() {
    let _guard = chain_guard().await;

    let challenger = match alice_challenger().await {
        Some(c) => c,
        None => {
            eprintln!("Chain not reachable — skipping test_get_challenge_stats");
            return;
        }
    };

    let stats = challenger
        .get_challenge_stats()
        .await
        .expect("get_challenge_stats should not error");

    println!(
        "Challenge stats: total={} successful={} failed={} earnings={}",
        stats.total_challenges,
        stats.successful_challenges,
        stats.failed_challenges,
        stats.total_earnings
    );

    assert!(
        stats.total_challenges >= stats.successful_challenges,
        "total_challenges should be >= successful_challenges"
    );
}

/// `get_total_challenge_earnings` always returns `Ok(0)` — rewards are
/// auto-distributed by `on_finalize` with no per-challenger aggregate.
#[tokio::test]
async fn test_get_total_challenge_earnings() {
    let _guard = chain_guard().await;

    let challenger = match alice_challenger().await {
        Some(c) => c,
        None => {
            eprintln!("Chain not reachable — skipping test_get_total_challenge_earnings");
            return;
        }
    };

    let earnings = challenger
        .get_total_challenge_earnings()
        .await
        .expect("get_total_challenge_earnings should not error");

    assert_eq!(
        earnings, 0,
        "earnings should be 0 (rewards auto-distributed, no on-chain aggregate)"
    );
}

/// `find_challenge_targets` scores all active agreements; on an empty chain
/// it returns an empty vec without error.
#[tokio::test]
async fn test_find_challenge_targets() {
    let _guard = chain_guard().await;

    let challenger = match alice_challenger().await {
        Some(c) => c,
        None => {
            eprintln!("Chain not reachable — skipping test_find_challenge_targets");
            return;
        }
    };

    let targets = challenger
        .find_challenge_targets(10)
        .await
        .expect("find_challenge_targets should not error");

    println!("Found {} challenge target(s)", targets.len());

    for t in &targets {
        assert!(!t.provider.is_empty(), "provider field should not be empty");
        assert!(
            (0.0..=1.0).contains(&t.success_probability),
            "success_probability out of [0,1] range"
        );
        println!(
            "  provider={} bucket={} expected_value={}",
            t.provider, t.bucket_id, t.expected_value
        );
    }
}

/// `auto_challenge_strategy(threshold=0)` challenges nobody (no provider has
/// reputation below 0), returns empty without error.
#[tokio::test]
async fn test_auto_challenge_strategy_no_crash() {
    let _guard = chain_guard().await;

    let challenger = match alice_challenger().await {
        Some(c) => c,
        None => {
            eprintln!("Chain not reachable — skipping test_auto_challenge_strategy_no_crash");
            return;
        }
    };

    let submitted = challenger
        .auto_challenge_strategy(0, 5)
        .await
        .expect("auto_challenge_strategy should not error");

    println!(
        "auto_challenge_strategy submitted {} challenge(s)",
        submitted.len()
    );
}

// ─── Tests that require a registered provider (chain_setup) ───────────────────

/// `analyze_provider` works for a registered provider even when the bucket has
/// no checkpoint snapshot — returns valid (possibly zero) stats.
#[tokio::test]
async fn test_analyze_provider() {
    let _guard = chain_guard().await;

    let setup = match chain_setup().await {
        Some(s) => s,
        None => {
            eprintln!("Chain not reachable — skipping test_analyze_provider");
            return;
        }
    };

    let challenger = alice_challenger()
        .await
        .expect("chain was already reachable in chain_setup");

    // Alice is the registered provider (set up by chain_setup).
    let analysis = challenger
        .analyze_provider(setup.bucket_id, setup.alice_ss58.clone())
        .await
        .expect("analyze_provider should succeed for a registered provider");

    println!(
        "Provider analysis: reputation={} checkpoint_age={} success_rate={:.2} recommendation={:?}",
        analysis.reputation,
        analysis.last_checkpoint_age,
        analysis.challenge_success_rate,
        analysis.recommendation
    );

    assert!(analysis.reputation <= 100, "reputation should be 0-100");
    assert!(
        (0.0..=100.0).contains(&analysis.challenge_success_rate),
        "challenge_success_rate out of [0,100] range"
    );
    assert_eq!(
        analysis.provider, setup.alice_ss58,
        "analysis.provider should match the queried account"
    );
}

/// `check_and_claim_reward` always returns `None` — rewards are auto-distributed
/// by `on_finalize`; there is no manual claim extrinsic.
#[tokio::test]
async fn test_check_and_claim_reward_returns_none() {
    use storage_client::challenger::ChallengeId;

    let _guard = chain_guard().await;

    let challenger = match alice_challenger().await {
        Some(c) => c,
        None => {
            eprintln!("Chain not reachable — skipping test_check_and_claim_reward_returns_none");
            return;
        }
    };

    let fake_id = ChallengeId {
        deadline: 999_999,
        index: 0,
    };

    let reward = challenger
        .check_and_claim_reward(fake_id)
        .await
        .expect("check_and_claim_reward should not error");

    assert!(
        reward.is_none(),
        "reward should be None (rewards are auto-distributed by on_finalize)"
    );
}
