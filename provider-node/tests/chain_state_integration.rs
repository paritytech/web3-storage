//! Integration tests for the chain-state coordinator that need no blockchain.
//!
//! The chain reads the coordinator performs sit behind the
//! [`ChainStateChainClient`] trait (like the other coordinators' `*ChainClient`
//! traits), so its synchronisation logic is driven here against a mock. Three
//! things are covered without ever touching a chain:
//!
//! 1. **State synchronisation.** [`sync_constants`] and [`refresh_provider_state`]
//!    are driven through [`MockChainClient`] across every branch — registered,
//!    not-registered, with/without replay state, and each chain-error path — and
//!    we assert the resulting [`ChainState`].
//!
//! 2. **Event relevance.** [`is_relevant_provider_event`] decides which block
//!    events trigger a refresh; tested across the provider lifecycle variants,
//!    wrong-account events, and unrelated events.
//!
//! 3. **Resilience.** [`ChainStateCoordinator::start`] drives a reconnect loop.
//!    Pointed at an unreachable chain it must stay up, never panic, leave
//!    [`ChainState`] at its defaults (so `/negotiate` keeps returning 503), and
//!    shut down cleanly when stopped.

use async_trait::async_trait;
use sp_core::H256;
use sp_runtime::AccountId32;
use std::sync::atomic::Ordering;
use std::sync::Arc;
use std::time::Duration;
use storage_client::discovery::ProviderInfo;
use storage_client::{ClientError, ProviderSettings, StorageEvent};
use storage_provider_node::{
    is_relevant_provider_event, refresh_if_relevant_event, refresh_provider_state, sync_constants,
    ChainState, ChainStateChainClient, ChainStateCoordinator, NonceCounter, NonceStore,
    PalletConstants,
};

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
        stake: 1_000,
        committed_bytes: 500,
        max_capacity: 10_000,
        min_duration: 10,
        max_duration: 100,
        price_per_byte: 5,
        accepting_primary: true,
        replica_sync_price: None,
        accepting_extensions: true,
        agreements_total: 3,
        challenges_failed: 1,
        deregister_at: None,
    }
}

// ── resilience against an unreachable chain ───────────────────────────────────

#[tokio::test]
async fn coordinator_leaves_state_at_defaults_while_chain_unreachable() {
    let chain_state = Arc::new(ChainState::default());
    let coordinator = ChainStateCoordinator::new(
        UNREACHABLE_CHAIN.to_string(),
        provider_account(),
        chain_state.clone(),
    );
    let handle = coordinator.start();

    // Give the reconnect loop time to attempt (and fail) at least one connect.
    tokio::time::sleep(Duration::from_millis(100)).await;

    // A chain it can't reach must never produce state: every field stays at the
    // default that makes `/negotiate` return 503.
    assert_eq!(chain_state.current_block.load(Ordering::Relaxed), 0);
    assert!(chain_state.constants.read().is_none());
    assert!(chain_state.provider_info.read().is_none());
    assert!(chain_state.nonce_counter.read().is_none());

    // And it shuts down cleanly rather than hanging.
    handle.stop().await;
}

#[tokio::test]
async fn coordinator_shares_chain_state_with_caller() {
    let chain_state = Arc::new(ChainState::default());
    let before = Arc::strong_count(&chain_state);

    let coordinator = ChainStateCoordinator::new(
        UNREACHABLE_CHAIN.to_string(),
        provider_account(),
        chain_state.clone(),
    );
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
    let chain_state = Arc::new(ChainState::default());
    let handle = ChainStateCoordinator::new(
        UNREACHABLE_CHAIN.to_string(),
        provider_account(),
        chain_state,
    )
    .start();

    // Stopping aborts the loop even while it is mid-backoff; it must not block
    // for the full RECONNECT_DELAY.
    tokio::time::timeout(Duration::from_secs(2), handle.stop())
        .await
        .expect("coordinator stop should return promptly");
}

#[tokio::test]
async fn coordinator_keeps_retrying_without_panicking() {
    let chain_state = Arc::new(ChainState::default());
    let handle = ChainStateCoordinator::new(
        UNREACHABLE_CHAIN.to_string(),
        provider_account(),
        chain_state.clone(),
    )
    .start();

    // Across several connect/backoff cycles the loop stays alive and never
    // dirties state. If the task had panicked, `stop()`'s join would surface it.
    for _ in 0..3 {
        tokio::time::sleep(Duration::from_millis(60)).await;
        assert_eq!(chain_state.current_block.load(Ordering::Relaxed), 0);
        assert!(chain_state.provider_info.read().is_none());
    }

    handle.stop().await;
}

#[tokio::test]
async fn coordinator_releases_shared_state_after_stop() {
    let chain_state = Arc::new(ChainState::default());
    let handle = ChainStateCoordinator::new(
        UNREACHABLE_CHAIN.to_string(),
        provider_account(),
        chain_state.clone(),
    )
    .start();

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
/// set, a `ClientError` — so every branch of `sync_constants` /
/// `refresh_provider_state` is reachable.
#[derive(Default)]
struct MockChainClient {
    info: Option<ProviderInfo>,
    info_err: bool,
    hsn: Option<u64>,
    hsn_err: bool,
    request_timeout: Option<u32>,
    request_timeout_err: bool,
}

#[async_trait]
impl ChainStateChainClient for MockChainClient {
    async fn get_provider_info(
        &self,
        _who: &AccountId32,
    ) -> Result<Option<ProviderInfo>, ClientError> {
        if self.info_err {
            return Err(ClientError::Chain("mock get_provider_info failure".into()));
        }
        Ok(self.info.clone())
    }

    async fn fetch_replay_hsn(&self, _who: &AccountId32) -> Result<Option<u64>, ClientError> {
        if self.hsn_err {
            return Err(ClientError::Chain("mock fetch_replay_hsn failure".into()));
        }
        Ok(self.hsn)
    }

    async fn fetch_request_timeout(&self) -> Result<Option<u32>, ClientError> {
        if self.request_timeout_err {
            return Err(ClientError::Chain(
                "mock fetch_request_timeout failure".into(),
            ));
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
    let cs = ChainState::default();
    let chain = MockChainClient {
        request_timeout: Some(200),
        ..Default::default()
    };

    sync_constants(&chain, &cs).await;

    assert_eq!(cs.constants.read().as_ref().unwrap().request_timeout, 200);
}

#[tokio::test]
async fn sync_constants_leaves_none_when_constant_absent() {
    let cs = ChainState::default();
    // request_timeout None → constant absent from metadata.
    sync_constants(&MockChainClient::default(), &cs).await;
    assert!(cs.constants.read().is_none());
}

#[tokio::test]
async fn sync_constants_leaves_none_on_chain_error() {
    let cs = ChainState::default();
    let chain = MockChainClient {
        request_timeout_err: true,
        ..Default::default()
    };
    sync_constants(&chain, &cs).await;
    assert!(cs.constants.read().is_none());
}

// ── refresh_provider_state ────────────────────────────────────────────────────

#[tokio::test]
async fn refresh_publishes_info_and_bootstrapped_counter() {
    let cs = ChainState::default();
    let chain = MockChainClient {
        info: Some(sample_provider_info()),
        hsn: Some(7),
        ..Default::default()
    };

    refresh_provider_state(&chain, &cs, &provider_account()).await;

    // Info is published…
    assert!(cs.provider_info.read().is_some());
    // …and the counter is bootstrapped from hsn 7, so it resumes at hsn + 1.
    let guard = cs.nonce_counter.read();
    let counter = guard.as_ref().expect("counter published");
    assert!(counter.is_bootstrapped());
    assert_eq!(counter.next(), 8);
}

#[tokio::test]
async fn refresh_publishes_unbootstrapped_counter_when_no_replay_state() {
    // Registered but the replay window isn't readable yet (`hsn` None): the
    // coordinator still publishes a counter alongside the info, but it must not
    // report bootstrapped — `/negotiate` keeps returning 503 until a later refresh.
    let cs = ChainState::default();
    let chain = MockChainClient {
        info: Some(sample_provider_info()),
        hsn: None,
        ..Default::default()
    };

    refresh_provider_state(&chain, &cs, &provider_account()).await;

    assert!(cs.provider_info.read().is_some());
    assert!(
        !cs.nonce_counter
            .read()
            .as_ref()
            .expect("counter published")
            .is_bootstrapped(),
        "counter must not report bootstrapped before the replay window is read"
    );
}

#[tokio::test]
async fn refresh_clears_info_and_counter_when_not_registered() {
    // Pre-seed a ready state, then refresh against a chain that reports the
    // provider is not (or no longer) registered — both fields are dropped so
    // `/negotiate` reports `provider_info_unavailable`.
    let cs = ChainState::default();
    cs.current_block.store(100, Ordering::Relaxed);
    *cs.constants.write() = Some(PalletConstants {
        request_timeout: 200,
    });
    let counter = Arc::new(NonceCounter::new(1));
    counter.bootstrap_from_hsn(0);
    *cs.nonce_counter.write() = Some(counter);
    *cs.provider_info.write() = Some(sample_provider_info());

    // info None → ProviderDeregistered / never-registered branch.
    refresh_provider_state(&MockChainClient::default(), &cs, &provider_account()).await;

    assert!(cs.provider_info.read().is_none());
    assert!(cs.nonce_counter.read().is_none());
    // Per-connection constants and the block height are not tied to registration.
    assert_eq!(cs.current_block.load(Ordering::Relaxed), 100);
    assert_eq!(cs.constants.read().as_ref().unwrap().request_timeout, 200);
}

#[tokio::test]
async fn refresh_leaves_existing_state_untouched_on_get_info_error() {
    // A transient chain error on `get_provider_info` must not clobber a
    // previously-published good state.
    let cs = ChainState::default();
    *cs.provider_info.write() = Some(sample_provider_info());
    let counter = Arc::new(NonceCounter::new(1));
    counter.bootstrap_from_hsn(3);
    *cs.nonce_counter.write() = Some(counter);

    let chain = MockChainClient {
        info_err: true,
        ..Default::default()
    };
    refresh_provider_state(&chain, &cs, &provider_account()).await;

    assert!(cs.provider_info.read().is_some());
    assert!(cs.nonce_counter.read().is_some());
}

#[tokio::test]
async fn refresh_publishes_info_but_leaves_counter_when_replay_fetch_errors() {
    // Provider is registered, but reading the replay window fails: info is still
    // published (settings/multiaddr remain fresh) but the counter is left as-is
    // (None here) so /negotiate stays correctly gated by is_bootstrapped().
    let cs = ChainState::default();
    let chain = MockChainClient {
        info: Some(sample_provider_info()),
        hsn_err: true,
        ..Default::default()
    };

    refresh_provider_state(&chain, &cs, &provider_account()).await;

    assert!(cs.provider_info.read().is_some());
    assert!(cs.nonce_counter.read().is_none());
}

#[tokio::test]
async fn refresh_preserves_bootstrapped_counter_on_later_event() {
    // A live, bootstrapped counter is never recreated on subsequent refreshes —
    // recreating would reset to hsn+1 and reissue nonces for in-flight quotes.
    let cs = ChainState::default();
    let counter = Arc::new(NonceCounter::new(1));
    counter.bootstrap_from_hsn(10); // counter now at 11
    counter.next(); // 11 -> 12
    counter.next(); // 12 -> 13 (two nonces issued beyond hsn+1)
    *cs.nonce_counter.write() = Some(counter);
    *cs.provider_info.write() = Some(sample_provider_info());

    // Chain reports hsn=0 — if we recreated the counter it would reset to 1.
    let chain = MockChainClient {
        info: Some(sample_provider_info()),
        hsn: Some(0),
        ..Default::default()
    };
    refresh_provider_state(&chain, &cs, &provider_account()).await;

    // Info was refreshed, but the counter is the original object at 13, not reset.
    assert!(cs.provider_info.read().is_some());
    let guard = cs.nonce_counter.read();
    let c = guard.as_ref().expect("counter still present");
    assert!(c.is_bootstrapped());
    assert_eq!(c.next(), 13, "counter must not have been reset");
}

#[tokio::test]
async fn refresh_completes_pending_bootstrap_when_replay_state_appears() {
    // After the transient window where hsn was None, a later refresh with a real
    // hsn must bootstrap the existing counter rather than leave it unbootstrapped.
    let cs = ChainState::default();
    let counter = Arc::new(NonceCounter::new(1));
    // Un-bootstrapped (simulates the hsn=None transient window).
    *cs.nonce_counter.write() = Some(counter);
    *cs.provider_info.write() = Some(sample_provider_info());

    let chain = MockChainClient {
        info: Some(sample_provider_info()),
        hsn: Some(5),
        ..Default::default()
    };
    refresh_provider_state(&chain, &cs, &provider_account()).await;

    let guard = cs.nonce_counter.read();
    let c = guard.as_ref().expect("counter present");
    assert!(c.is_bootstrapped(), "counter must now be bootstrapped");
    assert_eq!(c.next(), 6, "counter resumes at hsn+1");
}

// ── is_relevant_provider_event ────────────────────────────────────────────────

/// All provider-lifecycle events for the coordinator's own account trigger a refresh.
#[test]
fn lifecycle_events_for_self_are_relevant() {
    let me = provider_account();
    let bh = H256::zero();
    let events = [
        StorageEvent::ProviderRegistered {
            provider: me.clone(),
            stake: 0,
            block_hash: bh,
            block_number: 1,
        },
        StorageEvent::ProviderSettingsUpdated {
            provider: me.clone(),
            block_hash: bh,
            block_number: 1,
            provider_settings: ProviderSettings {
                price_per_byte: 5,
                min_duration: 10,
                max_duration: 100,
                accepting_primary: true,
                replica_sync_price: None,
                accepting_extensions: true,
                max_capacity: 0,
            },
        },
        StorageEvent::ProviderMultiaddrUpdated {
            provider: me.clone(),
            multiaddr: "/ip4/1.2.3.4/tcp/3333".to_string(),
            block_hash: bh,
            block_number: 1,
        },
        StorageEvent::DeregisterAnnounced {
            provider: me.clone(),
            complete_after: 10,
            block_hash: bh,
            block_number: 1,
        },
        StorageEvent::ProviderDeregistered {
            provider: me.clone(),
            stake_returned: 0,
            block_hash: bh,
            block_number: 1,
        },
        StorageEvent::DeregisterCancelled {
            provider: me.clone(),
            block_hash: bh,
            block_number: 1,
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
    let event = StorageEvent::ProviderRegistered {
        provider: provider_account_2(),
        stake: 0,
        block_hash: H256::zero(),
        block_number: 1,
    };
    // Same event shape, different account → not ours, ignore it.
    assert!(!is_relevant_provider_event(&event, &provider_account()));
}

#[test]
fn non_lifecycle_event_is_irrelevant() {
    // A checkpoint event names no `provider` we filter on; it must never trigger
    // a provider-state refresh.
    let event = StorageEvent::BucketCheckpointed {
        bucket_id: 1,
        mmr_root: H256::zero(),
        start_seq: 0,
        leaf_count: 0,
        providers: vec![provider_account()],
        block_hash: H256::zero(),
        block_number: 1,
    };
    assert!(!is_relevant_provider_event(&event, &provider_account()));
}

// ── refresh_if_relevant_event (block-event dispatch) ──────────────────────────

fn registered_event(provider: AccountId32) -> StorageEvent {
    StorageEvent::ProviderRegistered {
        provider,
        stake: 0,
        block_hash: H256::zero(),
        block_number: 1,
    }
}

#[tokio::test]
async fn relevant_block_event_triggers_a_refresh() {
    // A block carrying a lifecycle event for our account refreshes state from chain.
    let cs = ChainState::default();
    let chain = MockChainClient {
        info: Some(sample_provider_info()),
        hsn: Some(0),
        ..Default::default()
    };
    let events = [
        registered_event(provider_account_2()), // someone else — ignored
        registered_event(provider_account()),   // us — triggers the refresh
    ];

    refresh_if_relevant_event(&chain, &cs, &provider_account(), &events, 1).await;

    assert!(cs.provider_info.read().is_some());
    assert!(cs.nonce_counter.read().is_some());
}

#[tokio::test]
async fn irrelevant_block_events_do_not_refresh() {
    // Only other-provider / non-lifecycle events → no refresh, so a chain that
    // *would* return info is never consulted and state stays at defaults.
    let cs = ChainState::default();
    let chain = MockChainClient {
        info: Some(sample_provider_info()),
        hsn: Some(0),
        ..Default::default()
    };
    let events = [
        registered_event(provider_account_2()),
        StorageEvent::BucketCheckpointed {
            bucket_id: 1,
            mmr_root: H256::zero(),
            start_seq: 0,
            leaf_count: 0,
            providers: vec![provider_account()],
            block_hash: H256::zero(),
            block_number: 1,
        },
    ];

    refresh_if_relevant_event(&chain, &cs, &provider_account(), &events, 1).await;

    assert!(cs.provider_info.read().is_none());
    assert!(cs.nonce_counter.read().is_none());
}

#[tokio::test]
async fn empty_block_does_not_refresh() {
    let cs = ChainState::default();
    let chain = MockChainClient {
        info: Some(sample_provider_info()),
        hsn: Some(0),
        ..Default::default()
    };

    refresh_if_relevant_event(&chain, &cs, &provider_account(), &[], 1).await;

    assert!(cs.provider_info.read().is_none());
}

// ─── Nonce counter persistence (nonce_store seeding) ──────────────────────────

/// A minimal in-memory NonceStore for testing: holds the persisted value in a
/// Mutex so we can inspect it without touching disk.
struct RecordingNonceStore(std::sync::Mutex<Option<u64>>);

impl RecordingNonceStore {
    fn new(initial: Option<u64>) -> Self {
        Self(std::sync::Mutex::new(initial))
    }
}

impl NonceStore for RecordingNonceStore {
    fn load(&self) -> Option<u64> {
        *self.0.lock().unwrap()
    }

    fn persist(&self, value: u64) {
        let mut guard = self.0.lock().unwrap();
        if guard.is_none_or(|prev| value > prev) {
            *guard = Some(value);
        }
    }
}

#[tokio::test]
async fn refresh_seeds_counter_from_persisted_value_above_chain_hsn() {
    // When the persisted nonce high-water is higher than the chain's hsn+1,
    // the counter must resume above the persisted value — not reset to hsn+1.
    // This prevents reissuing nonces that were signed but not yet redeemed.

    // Simulate: last issued nonce was 20 (watermark = 21 = "next to issue").
    let store = std::sync::Arc::new(RecordingNonceStore::new(Some(21)));
    // In production, command.rs installs the store before the coordinator starts.
    // Since nonce_store is pub, set it directly.
    let cs = ChainState {
        nonce_store: store,
        ..Default::default()
    };

    // Chain reports hsn=5 → floor = 6. Persisted watermark = 21 wins.
    let chain = MockChainClient {
        info: Some(sample_provider_info()),
        hsn: Some(5),
        ..Default::default()
    };
    refresh_provider_state(&chain, &cs, &provider_account()).await;

    let guard = cs.nonce_counter.read();
    let counter = guard.as_ref().expect("counter created");
    assert!(counter.is_bootstrapped());
    // bootstrap_from_hsn(5) sets floor=6; counter was seeded at 21 which is
    // higher, so next() must be ≥ 21.
    assert!(
        counter.next() >= 21,
        "counter must resume at the persisted watermark, not at hsn+1"
    );
}
