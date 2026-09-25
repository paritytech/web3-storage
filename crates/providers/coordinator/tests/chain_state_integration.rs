// SPDX-License-Identifier: Apache-2.0

//! Integration tests for the chain-state coordinator that need no blockchain.
//!
//! The chain reads the coordinator performs sit behind the
//! [`ChainStateChainClient`] trait (like the other coordinators' `*ChainClient`
//! traits), so its synchronisation logic is driven here against a mock. Three
//! things are covered without ever touching a chain:
//!
//! 1. **State synchronisation.** [`sync_constants`] and [`refresh_provider_state`]
//!    are driven through [`MockChainClient`] across every branch — registered,
//!    not-registered, and each chain-error path — and we assert the resulting
//!    [`ChainState`].
//!
//! 2. **Event relevance.** [`is_relevant_provider_event`] decides which block
//!    events trigger a refresh; tested across the provider lifecycle variants
//!    and wrong-account events.
//!
//! 3. **Resilience.** [`ChainStateCoordinator::start`] drives a reconnect loop.
//!    Pointed at an unreachable chain it must stay up, never panic, leave
//!    [`ChainState`] at its defaults (so `/negotiate` keeps returning 503), and
//!    shut down cleanly when stopped.
//!
//! Membership invalidation is covered separately, in
//! `tests/coordinators/membership.rs`: the coordinator only broadcasts
//! `BlockEvent::BucketMembershipChanged`/`Resubscribed` now, and the
//! membership cache pulls from that feed itself.

use async_trait::async_trait;
use provider_chain::chain_connection::{ChainHandle, ChainTransport};
use provider_coordinator::{
    is_relevant_provider_event, refresh_if_relevant_event, refresh_provider_state, sync_constants,
    ChainState, ChainStateChainClient, ChainStateCoordinator, Error, PalletConstants,
    ProviderLifecycleEvent,
};
use provider_types::{ProviderInfo, ProviderSettings, ProviderStats};
use sp_runtime::AccountId32;
use std::sync::atomic::Ordering;
use std::sync::Arc;
use std::time::Duration;

/// Coordinator against the unreachable chain, with freshly-made (and
/// immediately caller-dropped) channel counterparts: `send` failures are
/// ignored by the coordinator, so this exercises the same loop as production.
fn unreachable_coordinator(chain_state: Arc<ChainState>) -> ChainStateCoordinator {
    ChainStateCoordinator::new(
        ChainTransport::Rpc {
            url: UNREACHABLE_CHAIN.to_string(),
        },
        provider_account(),
        chain_state,
        tokio::sync::watch::channel::<Option<ChainHandle>>(None).0,
        tokio::sync::broadcast::channel(16).0,
    )
}

/// A WS URL that refuses immediately: port 1 on loopback is never listening, so
/// every connect attempt fails fast and the coordinator loops on the error arm.
const UNREACHABLE_CHAIN: &str = "ws://127.0.0.1:1";

/// `[1u8; 32]` provider account — the coordinator only uses it to identify
/// relevant events, which never fire here since the chain is unreachable.
fn provider_account() -> sp_runtime::AccountId32 {
    sp_runtime::AccountId32::new([1u8; 32])
}

fn sample_provider_info() -> ProviderInfo {
    ProviderInfo {
        multiaddr: "/ip4/1.2.3.4/tcp/3333".to_string(),
        public_key: vec![1u8; 32],
        stake: 1_000,
        committed_bytes: 500,
        settings: ProviderSettings {
            min_duration: 10,
            max_duration: 100,
            price_per_byte: 5,
            accepting_primary: true,
            replica_sync_price: None,
            accepting_extensions: true,
            max_capacity: 10_000,
        },
        stats: ProviderStats {
            agreements_total: 3,
            challenges_failed: 1,
            ..Default::default()
        },
        deregister_at: None,
    }
}

// ── resilience against an unreachable chain ───────────────────────────────────

#[tokio::test]
async fn coordinator_leaves_state_at_defaults_while_chain_unreachable() {
    let chain_state = Arc::new(ChainState::new());
    let coordinator = unreachable_coordinator(chain_state.clone());
    let handle = coordinator.start();

    // Give the reconnect loop time to attempt (and fail) at least one connect.
    tokio::time::sleep(Duration::from_millis(100)).await;

    // A chain it can't reach must never produce state: every field stays at the
    // default that makes `/negotiate` return 503.
    assert_eq!(chain_state.current_anchor_block.load(Ordering::Relaxed), 0);
    assert!(chain_state.constants.read().is_none());
    assert!(chain_state.provider_info.read().is_none());

    // And it shuts down cleanly rather than hanging.
    handle.stop().await;
}

#[tokio::test]
async fn coordinator_shares_chain_state_with_caller() {
    let chain_state = Arc::new(ChainState::new());
    let before = Arc::strong_count(&chain_state);

    let coordinator = unreachable_coordinator(chain_state.clone());
    let handle = coordinator.start();

    // The coordinator holds its own clone of the same `Arc<ChainState>`, so the
    // caller (here, `ProviderState`) observes whatever the coordinator writes
    // without any back-reference.
    assert!(
        Arc::strong_count(&chain_state) > before,
        "coordinator should retain a shared handle to chain_state"
    );

    handle.stop().await;
}

#[tokio::test]
async fn coordinator_stop_is_prompt() {
    let chain_state = Arc::new(ChainState::new());
    let handle = unreachable_coordinator(chain_state).start();

    // Stopping aborts the loop even while it is mid-backoff; it must not block
    // for the full RECONNECT_DELAY.
    tokio::time::timeout(Duration::from_secs(2), handle.stop())
        .await
        .expect("coordinator stop should return promptly");
}

#[tokio::test]
async fn coordinator_keeps_retrying_without_panicking() {
    let chain_state = Arc::new(ChainState::new());
    let handle = unreachable_coordinator(chain_state.clone()).start();

    // Across several connect/backoff cycles the loop stays alive and never
    // dirties state. If the task had panicked, `stop()`'s join would surface it.
    for _ in 0..3 {
        tokio::time::sleep(Duration::from_millis(60)).await;
        assert_eq!(chain_state.current_anchor_block.load(Ordering::Relaxed), 0);
        assert!(chain_state.provider_info.read().is_none());
    }

    handle.stop().await;
}

#[tokio::test]
async fn coordinator_releases_shared_state_after_stop() {
    let chain_state = Arc::new(ChainState::new());
    let handle = unreachable_coordinator(chain_state.clone()).start();

    // `stop()` aborts the task and awaits its teardown, dropping the coordinator
    // and its `chain_state` clone. (Merely *dropping* the handle does not — tokio
    // detaches a dropped `JoinHandle`, leaving the loop running.)
    handle.stop().await;

    assert_eq!(
        Arc::strong_count(&chain_state),
        1,
        "after stop() the only strong ref to chain_state should be this test's"
    );
}

// ── mock chain client ─────────────────────────────────────────────────────────

/// Canned [`ChainStateChainClient`] for driving the synchronisation logic
/// without a chain. Each read is either `Ok(value)` or, when its `*_err` flag is
/// set, an `Error` — so every branch of `sync_constants` /
/// `refresh_provider_state` is reachable.
#[derive(Default)]
struct MockChainClient {
    info: Option<ProviderInfo>,
    info_err: bool,
    request_timeout: Option<u32>,
    request_timeout_err: bool,
}

#[async_trait]
impl ChainStateChainClient for MockChainClient {
    async fn get_provider_info(&self, _who: &AccountId32) -> Result<Option<ProviderInfo>, Error> {
        if self.info_err {
            return Err(Error::Internal("mock get_provider_info failure".into()));
        }
        Ok(self.info.clone())
    }

    async fn fetch_request_timeout(&self) -> Result<Option<u32>, Error> {
        if self.request_timeout_err {
            return Err(Error::Internal("mock fetch_request_timeout failure".into()));
        }
        Ok(self.request_timeout)
    }
}

fn provider_account_2() -> AccountId32 {
    AccountId32::new([2u8; 32])
}

// ── sync_constants ────────────────────────────────────────────────────────────

#[tokio::test]
async fn sync_constants_publishes_request_timeout() {
    let cs = ChainState::new();
    let chain = MockChainClient {
        request_timeout: Some(200),
        ..Default::default()
    };

    sync_constants(&chain, &cs).await;

    assert_eq!(cs.constants.read().as_ref().unwrap().request_timeout, 200);
}

#[tokio::test]
async fn sync_constants_leaves_none_when_constant_absent() {
    let cs = ChainState::new();
    // request_timeout None → constant absent from metadata.
    sync_constants(&MockChainClient::default(), &cs).await;
    assert!(cs.constants.read().is_none());
}

#[tokio::test]
async fn sync_constants_leaves_none_on_chain_error() {
    let cs = ChainState::new();
    let chain = MockChainClient {
        request_timeout_err: true,
        ..Default::default()
    };
    sync_constants(&chain, &cs).await;
    assert!(cs.constants.read().is_none());
}

// ── refresh_provider_state ────────────────────────────────────────────────────

#[tokio::test]
async fn refresh_publishes_info_when_registered() {
    let cs = ChainState::new();
    let chain = MockChainClient {
        info: Some(sample_provider_info()),
        ..Default::default()
    };

    refresh_provider_state(&chain, &cs, &provider_account()).await;

    assert!(cs.provider_info.read().is_some());
}

#[tokio::test]
async fn refresh_clears_info_when_not_registered() {
    // Pre-seed a ready state, then refresh against a chain that reports the
    // provider is not (or no longer) registered — info is dropped so
    // `/negotiate` reports `provider_info_unavailable`.
    let cs = ChainState::new();
    cs.current_anchor_block.store(100, Ordering::Relaxed);
    *cs.constants.write() = Some(PalletConstants {
        request_timeout: 200,
    });
    *cs.provider_info.write() = Some(sample_provider_info());

    // info None → ProviderDeregistered / never-registered branch.
    refresh_provider_state(&MockChainClient::default(), &cs, &provider_account()).await;

    assert!(cs.provider_info.read().is_none());
    // Per-connection constants and the block height are not tied to registration.
    assert_eq!(cs.current_anchor_block.load(Ordering::Relaxed), 100);
    assert_eq!(cs.constants.read().as_ref().unwrap().request_timeout, 200);
}

#[tokio::test]
async fn refresh_leaves_existing_state_untouched_on_get_info_error() {
    // A transient chain error on `get_provider_info` must not clobber a
    // previously-published good state.
    let cs = ChainState::new();
    *cs.provider_info.write() = Some(sample_provider_info());

    let chain = MockChainClient {
        info_err: true,
        ..Default::default()
    };
    refresh_provider_state(&chain, &cs, &provider_account()).await;

    assert!(cs.provider_info.read().is_some());
}

// ── is_relevant_provider_event ────────────────────────────────────────────────

/// All provider-lifecycle events for the coordinator's own account trigger a refresh.
#[test]
fn lifecycle_events_for_self_are_relevant() {
    let me = provider_account();
    let events = [
        ProviderLifecycleEvent::Updated {
            provider: me.clone(),
        },
        ProviderLifecycleEvent::Deregistered {
            provider: me.clone(),
        },
    ];

    for event in &events {
        assert!(
            is_relevant_provider_event(event, &me),
            "{event:?} should be relevant for the provider's own account"
        );
    }
}

#[test]
fn lifecycle_event_for_other_provider_is_irrelevant() {
    let event = ProviderLifecycleEvent::Updated {
        provider: provider_account_2(),
    };
    // Same event shape, different account → not ours, ignore it.
    assert!(!is_relevant_provider_event(&event, &provider_account()));
}

// ── refresh_if_relevant_event (block-event dispatch) ──────────────────────────

fn registered_event(provider: AccountId32) -> ProviderLifecycleEvent {
    ProviderLifecycleEvent::Updated { provider }
}

#[tokio::test]
async fn relevant_block_event_triggers_a_refresh() {
    // A block carrying a lifecycle event for our account refreshes state from chain.
    let cs = ChainState::new();
    let chain = MockChainClient {
        info: Some(sample_provider_info()),
        ..Default::default()
    };
    let events = [
        registered_event(provider_account_2()), // someone else — ignored
        registered_event(provider_account()),   // us — triggers the refresh
    ];

    refresh_if_relevant_event(&chain, &cs, &provider_account(), &events, 1).await;

    assert!(cs.provider_info.read().is_some());
}

#[tokio::test]
async fn irrelevant_block_events_do_not_refresh() {
    // Only other-provider / non-lifecycle events → no refresh, so a chain that
    // *would* return info is never consulted and state stays at defaults.
    let cs = ChainState::new();
    let chain = MockChainClient {
        info: Some(sample_provider_info()),
        ..Default::default()
    };
    let events = [
        registered_event(provider_account_2()),
        ProviderLifecycleEvent::Deregistered {
            provider: provider_account_2(),
        },
    ];

    refresh_if_relevant_event(&chain, &cs, &provider_account(), &events, 1).await;

    assert!(cs.provider_info.read().is_none());
}

#[tokio::test]
async fn empty_block_does_not_refresh() {
    let cs = ChainState::new();
    let chain = MockChainClient {
        info: Some(sample_provider_info()),
        ..Default::default()
    };

    refresh_if_relevant_event(&chain, &cs, &provider_account(), &[], 1).await;

    assert!(cs.provider_info.read().is_none());
}
