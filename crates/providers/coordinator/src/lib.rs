// SPDX-License-Identifier: Apache-2.0

//! Chain-state coordinator: keeps the provider node's view of the runtime in
//! sync via a finalized-block subscription.
//!
//! [`ChainState`] is the single source of truth for all on-chain state the
//! provider node needs at runtime:
//! - [`ChainState::current_anchor_block`] — the pallet's anchor block (the
//!   clock all on-chain durations use), read via its runtime API.
//! - [`ChainState::constants`] — pallet constants fetched once on connect.
//! - [`ChainState::provider_info`] — full provider registration info.
//! - [`ChainState::nonce_counter`] — nonce counter bootstrapped from the
//!   chain's replay window. `None` until the provider is registered.
//!
//! [`ChainStateCoordinator`] is the **only writer** for all four fields.  It
//! drives a finalized-block subscription on its own chain connection in a
//! reconnect loop; on every relevant provider event it re-fetches the full
//! `ProviderInfo` so `committed_bytes`, `stake`, and all settings stay
//! current — no field-patching, no partial updates, no second writer.
//!
//! It also broadcasts bucket-membership changes as
//! [`BlockEvent::BucketMembershipChanged`], so the membership cache can drop
//! stale authorization on its own rather than being told to.

pub mod chain_client;
pub mod follower;

pub use chain_client::ChainStateChainClient;
pub use follower::{BlockUpdate, ChainFollower, ChainSession, FinalizedBlock, FinalizedBlocks};

use parking_lot::RwLock;
use provider_chain::{BlockEvent, BlockEventTx};
use provider_storage::NonceStore;
use provider_types::ProviderInfo;
use sp_runtime::AccountId32;
use std::future::Future;
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering};
use std::sync::Arc;
use std::time::Duration;
use tokio::task::JoinHandle;

// ── Error ─────────────────────────────────────────────────────────────────────

/// Errors surfaced by the chain-state coordinator.
#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("Internal error: {0}")]
    Internal(String),
}

/// Run `fut` under `budget`, mapping expiry to an [`Error`] naming `what`.
///
/// Every wait in the reconnect loop goes through this: an operation that can
/// hang forever (e.g. a wedged smoldot backend with no timeout of its own)
/// must turn into an `Err` so the loop can rebuild the connection instead of
/// hanging with a stale handle still published.
async fn with_timeout<T>(
    what: &str,
    budget: Duration,
    fut: impl Future<Output = Result<T, Error>>,
) -> Result<T, Error> {
    match tokio::time::timeout(budget, fut).await {
        Ok(result) => result,
        Err(_) => Err(Error::Internal(format!(
            "{what} timed out after {}s",
            budget.as_secs()
        ))),
    }
}

// ── ChainState ────────────────────────────────────────────────────────────────

/// Live chain state kept in sync with the runtime by the chain-state coordinator.
///
/// Held behind `Arc` inside the provider node's `ProviderState` so the coordinator can hold
/// its own handle without a back-reference to the whole node state.
pub struct ChainState {
    /// The pallet's anchor block — the clock all on-chain durations (timeouts,
    /// `valid_until`, nonce age) are measured against — read via the
    /// `StorageProviderApi::current_anchor_block` runtime API at the latest
    /// finalized block. Whether that anchor is a relay, parachain, or other
    /// block number is the pallet's concern, not the provider's. `0` means not
    /// yet known.
    pub current_anchor_block: AtomicU32,
    /// Pallet constants fetched once per connection. `None` until the first
    /// successful fetch; `/negotiate` returns 503 until this is `Some`.
    pub constants: RwLock<Option<PalletConstants>>,
    /// Provider's on-chain registration info. `None` until registered on chain;
    /// re-fetched (full) on every relevant provider event so `committed_bytes`,
    /// `stake`, and all settings stay current.
    pub provider_info: RwLock<Option<ProviderInfo>>,
    /// Nonce counter bootstrapped from the chain's replay window. `None` until
    /// the provider is registered and the replay state is available.
    /// `/negotiate` returns 503 while `None`.
    pub nonce_counter: RwLock<Option<Arc<NonceCounter>>>,
    /// Persistence backing for the nonce counter, so the coordinator can seed
    /// a restarted counter above the last issued nonce.
    pub nonce_store: Arc<dyn NonceStore>,
}

impl ChainState {
    /// Fresh chain state whose nonce counter persists through `store`.
    pub fn with_nonce_store(store: Arc<dyn NonceStore>) -> Self {
        Self {
            current_anchor_block: AtomicU32::new(0),
            constants: RwLock::new(None),
            provider_info: RwLock::new(None),
            nonce_counter: RwLock::new(None),
            nonce_store: store,
        }
    }
}

/// Pallet constants that only change across runtime upgrades.
pub struct PalletConstants {
    /// Chain-enforced validity window (in blocks) for provider-signed terms.
    pub request_timeout: u32,
}

// ── NonceCounter ──────────────────────────────────────────────────────────────

/// Monotonic nonce counter for provider-signed terms.
///
/// Nonces are atomically allocated via [`Self::next`]. The chain-state
/// coordinator aligns the counter with the chain's `ProviderReplayState.hsn + 1`
/// (`hsn` = highest sequence nonce, the top of the chain's replay window) on
/// connect and on every relevant provider event, so the counter resumes at
/// `max(persisted_local, hsn + 1)`:
///
/// * **Local persistence** (disk mode): each allocation is persisted before
///   returning, so a **clean process restart** does not reissue nonces that were
///   signed but not yet redeemed. Power-loss/kernel-panic may lose the last write
///   (the RocksDB WAL - write-ahead log - is not fsynced per allocation); in that
///   case the counter falls back to `chain_hsn + 1`, which is still safe - the
///   chain's replay window rejects any duplicate redemption.
/// * **Chain alignment**: `bootstrap_from_hsn` advances the counter past any
///   nonce the chain has already accepted, covering redemptions that happened
///   while the node was down or while we weren't watching.
///
/// Gap-skipping is fine: unused nonces just expire from the replay window
/// without effect. The on-chain replay window is authoritative and rejects
/// any out-of-range reuse, so a missed nonce can never lead to a double
/// redemption.
///
/// Until the first successful [`Self::bootstrap_from_hsn`] the counter has not
/// been reconciled with the chain, so `/negotiate` must not sign with it; query
/// [`Self::is_bootstrapped`] to gate that.
pub struct NonceCounter {
    counter: AtomicU64,
    /// Set once the counter has been aligned with the chain's replay window.
    bootstrapped: AtomicBool,
    store: Arc<dyn NonceStore>,
}

impl std::fmt::Debug for NonceCounter {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("NonceCounter")
            .field("counter", &self.counter)
            .field("bootstrapped", &self.bootstrapped)
            .finish()
    }
}

impl NonceCounter {
    /// Create a counter starting at `start` backed by `store` for persistence.
    ///
    /// Seed `start` from `store.load().unwrap_or(1)` (the persisted high-water
    /// mark), then call `bootstrap_from_hsn` to advance past the chain's replay
    /// head. The counter is *not* considered bootstrapped until then.
    pub fn with_store(start: u64, store: Arc<dyn NonceStore>) -> Self {
        Self {
            counter: AtomicU64::new(start),
            bootstrapped: AtomicBool::new(false),
            store,
        }
    }

    /// Whether the counter has been reconciled with the chain's replay window
    /// at least once. `/negotiate` gates on this so it never signs a nonce
    /// that was not derived from on-chain state.
    pub fn is_bootstrapped(&self) -> bool {
        self.bootstrapped.load(Ordering::SeqCst)
    }

    /// Advance the counter to at least `hsn + 1` and mark it bootstrapped.
    /// Idempotent — only advances forward.
    pub fn bootstrap_from_hsn(&self, hsn: u64) {
        self.bootstrapped.store(true, Ordering::SeqCst);
        let target = hsn.saturating_add(1);
        // Standard CAS loop — bump only if our target is higher than
        // whatever is already there.
        let mut current = self.counter.load(Ordering::SeqCst);
        while current < target {
            match self.counter.compare_exchange_weak(
                current,
                target,
                Ordering::SeqCst,
                Ordering::SeqCst,
            ) {
                Ok(_) => break,
                Err(observed) => current = observed,
            }
        }
    }

    /// Allocate the next nonce. Atomic: concurrent callers each get a distinct
    /// value.
    ///
    /// The value *after* the increment (`nonce + 1`) is persisted as the new
    /// high-water mark before the nonce is returned. This means the persisted
    /// value always equals the next nonce that *will* be issued, so a counter
    /// seeded with `store.load().unwrap_or(1)` on restart correctly resumes
    /// above every nonce that was signed.
    pub fn next(&self) -> u64 {
        let nonce = self.counter.fetch_add(1, Ordering::SeqCst);
        self.store.persist(nonce.saturating_add(1));
        nonce
    }
}

// ── provider lifecycle events ─────────────────────────────────────────────────

/// Minimal decoded view of a `StorageProvider` provider-lifecycle event.
///
/// The coordinator re-fetches the full provider state on any relevant event,
/// so only the affected provider account — and whether the event is a
/// confirmed deregistration — needs decoding.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ProviderLifecycleEvent {
    /// `ProviderRegistered`, `ProviderSettingsUpdated`,
    /// `ProviderMultiaddrUpdated`, `DeregisterAnnounced`, or
    /// `DeregisterCancelled`.
    Updated { provider: AccountId32 },
    /// Confirmed `ProviderDeregistered`.
    Deregistered { provider: AccountId32 },
}

impl ProviderLifecycleEvent {
    /// The provider account the event concerns.
    pub fn provider(&self) -> &AccountId32 {
        match self {
            Self::Updated { provider } | Self::Deregistered { provider } => provider,
        }
    }
}

// ── ChainStateCoordinator ─────────────────────────────────────────────────────

/// Builds and starts the live chain-state synchronisation for a single provider.
///
/// Start with [`ChainStateCoordinator::start`]; keep the returned
/// [`ChainStateCoordinatorHandle`] alive for the duration of the server.
pub struct ChainStateCoordinator {
    /// Builds and (re)connects the underlying chain connection. Also
    /// responsible for publishing each new connection to the node's other
    /// chain consumers, once its block stream is confirmed up.
    follower: Arc<dyn ChainFollower>,
    provider_account: AccountId32,
    chain_state: Arc<ChainState>,
    /// Fan-out of decoded per-block events to the background coordinators.
    events_tx: BlockEventTx,
}

impl ChainStateCoordinator {
    pub fn new(
        follower: Arc<dyn ChainFollower>,
        provider_account: AccountId32,
        chain_state: Arc<ChainState>,
        events_tx: BlockEventTx,
    ) -> Self {
        Self {
            follower,
            provider_account,
            chain_state,
            events_tx,
        }
    }

    /// Spawn the coordinator and return immediately.
    ///
    /// The spawned task connects to the chain, follows finalized blocks, and
    /// reconnects automatically: a chain that is unreachable at startup or that
    /// drops the connection later is retried with a fixed backoff instead of
    /// taking the coordinator down. Runs until the returned handle is dropped or
    /// [`ChainStateCoordinatorHandle::stop`] is called.
    pub fn start(self) -> ChainStateCoordinatorHandle {
        let task = tokio::spawn(self.run());
        ChainStateCoordinatorHandle { task }
    }

    /// Reconnect loop: (re)connect and follow finalized blocks forever, sleeping
    /// [`RECONNECT_DELAY`] between attempts so an unreachable chain doesn't spin.
    async fn run(self) {
        const RECONNECT_DELAY: Duration = Duration::from_secs(5);

        loop {
            match self.connect_and_follow().await {
                Ok(()) => tracing::warn!(
                    "chain-state coordinator: block stream ended; reconnecting in {}s",
                    RECONNECT_DELAY.as_secs()
                ),
                Err(e) => tracing::warn!(
                    "chain-state coordinator: connection lost ({e}); retrying in {}s",
                    RECONNECT_DELAY.as_secs()
                ),
            }
            tokio::time::sleep(RECONNECT_DELAY).await;
        }
    }

    /// Connect to the chain, bootstrap initial state, then drive the finalized-block
    /// stream until it ends or stalls. Returns `Err` if connecting or bootstrapping
    /// fails; `Ok(())` if the stream terminates — either way the caller reconnects.
    async fn connect_and_follow(&self) -> Result<(), Error> {
        /// Budget for building a cold connection. On the light transport,
        /// `connect` awaits smoldot's peer discovery and warp sync with no
        /// timeout of its own, so a wedged light client would otherwise hang
        /// here forever — with the previous (dead) handle still published to
        /// consumers — and the reconnect loop could never rebuild it. Generous
        /// because killing a slow warp sync throws its progress away.
        const CONNECT_TIMEOUT: Duration = Duration::from_secs(300);

        let session = with_timeout("Connecting to the chain", CONNECT_TIMEOUT, async {
            self.follower.connect().await
        })
        .await?;
        self.follow(session).await
    }

    /// Bootstrap state from the connection and follow its finalized blocks
    /// until the stream ends or stalls. Split from
    /// [`Self::connect_and_follow`] so tests can drive the full pipeline over
    /// a mock connection.
    async fn follow(&self, session: Box<dyn ChainSession>) -> Result<(), Error> {
        /// How long without a finalized block before the connection is treated
        /// as dead and rebuilt. Finality can pause briefly (session boundaries,
        /// backend resubscriptions), so this is several times the block time;
        /// a genuinely stalled stream otherwise hangs forever with no error.
        /// The connection is already warp-synced by `connect`, so the first
        /// block gets the same budget as every other.
        const STALL_TIMEOUT: Duration = Duration::from_secs(60);
        /// Budget for subscribing and the bootstrap reads below: ordinary RPC
        /// round-trips on an already-synced connection, but on the light
        /// client they have no timeout of their own and a wedged backend
        /// would otherwise hang the reconnect loop forever.
        const BOOTSTRAP_TIMEOUT: Duration = Duration::from_secs(60);

        let (mut blocks, chain) = with_timeout("Chain bootstrap", BOOTSTRAP_TIMEOUT, async {
            let (blocks, chain) = session.subscribe().await?;

            tracing::info!("chain-state coordinator: connected; following finalized blocks");

            // Fetch pallet constants once per connection (they only change on runtime upgrade).
            sync_constants(chain.as_ref(), &self.chain_state).await;

            // Bootstrap from any existing on-chain state so a restarted node that was
            // already registered picks up its provider_info and nonce counter immediately
            // rather than waiting for the next relevant event.
            refresh_provider_state(chain.as_ref(), &self.chain_state, &self.provider_account).await;

            Ok::<_, Error>((blocks, chain))
        })
        .await?;

        // Tell coordinators to reconcile: events emitted while the stream was
        // down were missed for good, so they re-scan chain state instead.
        let _ = self.events_tx.send(BlockEvent::Resubscribed {
            at_block: self
                .chain_state
                .current_anchor_block
                .load(std::sync::atomic::Ordering::Relaxed),
        });

        loop {
            let update = match tokio::time::timeout(STALL_TIMEOUT, blocks.next()).await {
                Ok(Some(update)) => update,
                Ok(None) => break,
                Err(_) => {
                    tracing::warn!(
                        "chain-state coordinator: no finalized block for {}s; rebuilding connection",
                        STALL_TIMEOUT.as_secs()
                    );
                    break;
                }
            };
            let block = match update {
                BlockUpdate::Block(block) => block,
                BlockUpdate::Unreadable { number } => {
                    escalate_block_read_failure(&self.events_tx, number);
                    continue;
                }
            };

            // A failed anchor read keeps the previous value rather than
            // resetting it - see `FinalizedBlock::anchor_block`.
            if let Some(anchor_block) = block.anchor_block {
                self.chain_state
                    .current_anchor_block
                    .store(anchor_block, std::sync::atomic::Ordering::Relaxed);
            }

            // Fan out the coordinator-relevant events. Send failures just mean
            // no coordinator is subscribed.
            for event in block.events {
                let _ = self.events_tx.send(event);
            }

            self.process_provider_events(chain.as_ref(), &block.lifecycle, block.number)
                .await;
        }

        Ok(())
    }

    /// Refresh state if any of `parsed` is a relevant provider event.
    async fn process_provider_events(
        &self,
        chain: &dyn ChainStateChainClient,
        parsed: &[ProviderLifecycleEvent],
        block_number: u32,
    ) {
        refresh_if_relevant_event(
            chain,
            &self.chain_state,
            &self.provider_account,
            parsed,
            block_number,
        )
        .await;
    }
}

// ── state synchronisation (chain-client-agnostic) ─────────────────────────────

/// Fetch the `StorageProvider::RequestTimeout` runtime constant and store it in
/// `chain_state.constants`. Called once on each (re)connect. Logs at warn if
/// absent so operators notice a metadata problem rather than silent 503s.
pub async fn sync_constants(chain: &dyn ChainStateChainClient, chain_state: &ChainState) {
    match chain.fetch_request_timeout().await {
        Ok(Some(timeout)) => {
            *chain_state.constants.write() = Some(PalletConstants {
                request_timeout: timeout,
            });
            tracing::debug!("chain-state coordinator: RequestTimeout = {timeout}");
        }
        Ok(None) => tracing::warn!(
            "chain-state coordinator: RequestTimeout constant absent from runtime metadata;"
        ),
        Err(e) => tracing::warn!("chain-state coordinator: failed to fetch RequestTimeout: {e}"),
    }
}

/// Re-fetch `ProviderInfo` from chain and update `chain_state`.
///
/// **Nonce-counter lifecycle** (bootstrap-once / preserve / drop):
/// - While registered, the counter is bootstrapped at most once. If the counter
///   is already `Some` and bootstrapped, it is left completely untouched (and
///   `fetch_replay_hsn` is not called) so that in-flight nonces are never
///   reissued. If it is `None` or not yet bootstrapped, the replay head is
///   fetched and a new counter is created.
/// - `provider_info` is always refreshed when the provider is registered,
///   regardless of whether the hsn fetch errors (counter left as-is).
/// - When the provider is not (or no longer) registered, both `provider_info`
///   and `nonce_counter` are cleared.
///
/// Called both on the initial connect (restart recovery) and on every relevant
/// provider event.
pub async fn refresh_provider_state(
    chain: &dyn ChainStateChainClient,
    chain_state: &ChainState,
    provider_account: &AccountId32,
) {
    match chain.get_provider_info(provider_account).await {
        Ok(Some(info)) => {
            // Check bootstrap status before taking any write lock.
            let needs_bootstrap = chain_state
                .nonce_counter
                .read()
                .as_ref()
                .is_none_or(|c| !c.is_bootstrapped());

            if needs_bootstrap {
                match chain.fetch_replay_hsn(provider_account).await {
                    Ok(hsn) => {
                        // Seed from the locally-persisted high-water mark so a
                        // restart resumes at max(persisted, hsn+1) rather than
                        // resetting to hsn+1 (which would reissue un-redeemed nonces).
                        let start = chain_state.nonce_store.load().unwrap_or(1);
                        tracing::debug!(
                            "chain-state coordinator: loaded nonce counter start from {}",
                            start
                        );
                        let counter = Arc::new(NonceCounter::with_store(
                            start,
                            chain_state.nonce_store.clone(),
                        ));
                        if let Some(hsn) = hsn {
                            counter.bootstrap_from_hsn(hsn);
                            tracing::info!("chain-state coordinator: provider state synced");
                        }
                        // Registered but no replay state yet — transient view;
                        // a later refresh will call bootstrap_from_hsn.
                        *chain_state.nonce_counter.write() = Some(counter);
                    }
                    Err(e) => {
                        tracing::debug!("chain-state coordinator: failed to fetch replay hsn: {e}");
                        // Leave the counter as-is; info is still published below.
                    }
                }
            }

            *chain_state.provider_info.write() = Some(info);
        }
        // Provider is not (or no longer) registered on chain.
        Ok(None) => {
            *chain_state.provider_info.write() = None;
            *chain_state.nonce_counter.write() = None;
            tracing::debug!("chain-state coordinator: provider not registered on chain");
        }
        Err(e) => tracing::warn!("chain-state coordinator: failed to fetch provider info: {e}"),
    }
}

/// Refresh provider state iff at least one of `events` is relevant to
/// `provider_account`. Collapsing multiple events in one block to a single
/// refresh is correct: [`refresh_provider_state`] always reads the latest chain
/// state, so no intermediate event is "missed".
pub async fn refresh_if_relevant_event(
    chain: &dyn ChainStateChainClient,
    chain_state: &ChainState,
    provider_account: &AccountId32,
    events: &[ProviderLifecycleEvent],
    block_number: u32,
) {
    let relevant = events
        .iter()
        .any(|e| is_relevant_provider_event(e, provider_account));

    if relevant {
        tracing::debug!(
            "chain-state coordinator: provider event in block {block_number}, refreshing state"
        );
        refresh_provider_state(chain, chain_state, provider_account).await;
    }

    // On a confirmed deregistration, clear the persisted nonce high-water mark so
    // a later re-registration restarts the sequence from the chain's fresh replay
    // head (hsn + 1) rather than the stale watermark.
    //
    // The reset is deliberate, not cosmetic, and is safe: every quote signed
    // before deregistration has `valid_until <= sign_block + RequestTimeout`, and
    // RequestTimeout < DeregisterAnnouncementPeriod, so all such quotes have
    // already expired by the time `complete_deregister` is callable. No
    // pre-deregister nonce can be replayed against the new incarnation, so the
    // counter need not be held above the old watermark. (Keeping it would also be
    // safe but would needlessly inflate nonces across a re-registration.)
    //
    // Gate strictly on a confirmed `ProviderDeregistered` event, not on a generic
    // `Ok(None)` from `refresh_provider_state` (which also fires on
    // reconnect/bootstrap and non-finalized reads). This preserves the watermark
    // as a backstop on every path that is not a real deregistration.
    let deregistered = events.iter().any(|e| {
        matches!(e, ProviderLifecycleEvent::Deregistered { provider } if provider == provider_account)
    });
    if deregistered {
        chain_state.nonce_store.reset();
    }
}

/// Whether `event` is a provider lifecycle event for `provider_account` — i.e. one
/// that should trigger a [`refresh_provider_state`]. Settings, multiaddr, and the
/// (de)registration events all change state `/negotiate` depends on; everything
/// else (checkpoints, challenges, agreements, other providers) is filtered out
/// at parse time already.
pub fn is_relevant_provider_event(
    event: &ProviderLifecycleEvent,
    provider_account: &AccountId32,
) -> bool {
    event.provider() == provider_account
}

// ── ChainStateCoordinatorHandle ───────────────────────────────────────────────

/// Keeps the coordinator alive. Drop or call [`stop`](Self::stop) to shut down.
pub struct ChainStateCoordinatorHandle {
    task: JoinHandle<()>,
}

impl ChainStateCoordinatorHandle {
    /// Stop the coordinator. Aborting the loop drops the stream, which aborts the
    /// underlying block subscription.
    pub async fn stop(self) {
        tracing::info!("chain-state coordinator: stopped");
        self.task.abort();
        let _ = self.task.await;
    }
}

/// A finalized block's handle or events could not be fetched at all, so its
/// membership events (if any) are lost rather than merely dropped one at a
/// time - the same "no bucket id left to invalidate" situation an undecodable
/// event escalates to, and for the same reason: a plain `continue` here would
/// let a revoked member keep authorizing for a full `--auth-cache-ttl` on a
/// node that is otherwise connected and healthy.
fn escalate_block_read_failure(events_tx: &BlockEventTx, block_number: u32) {
    let _ = events_tx.send(BlockEvent::MembershipScopeUnknown {
        at_block: block_number,
    });
}

// ── tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use async_trait::async_trait;
    use provider_storage::temp_rocksdb;
    use provider_types::{ProviderSettings, ProviderStats};
    use std::collections::VecDeque;
    use std::sync::atomic::{AtomicUsize, Ordering};

    /// Chain state over a throwaway backend's nonce store.
    fn test_chain_state() -> (ChainState, tempfile::TempDir) {
        let (_storage, nonce_store, dir) = temp_rocksdb();
        (ChainState::with_nonce_store(nonce_store), dir)
    }

    fn provider_account() -> AccountId32 {
        AccountId32::new([7u8; 32])
    }

    #[test]
    fn escalate_block_read_failure_broadcasts_membership_scope_unknown() {
        let (tx, mut rx) = tokio::sync::broadcast::channel(4);
        escalate_block_read_failure(&tx, 99);
        assert!(matches!(
            rx.try_recv(),
            Ok(BlockEvent::MembershipScopeUnknown { at_block: 99 })
        ));
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

    #[test]
    fn chain_state_defaults_to_unknown() {
        let (cs, _dir) = test_chain_state();
        assert_eq!(cs.current_anchor_block.load(Ordering::Relaxed), 0);
        assert!(cs.constants.read().is_none());
        assert!(cs.provider_info.read().is_none());
        assert!(cs.nonce_counter.read().is_none());
    }

    #[test]
    fn chain_state_current_anchor_block_round_trips() {
        let (cs, _dir) = test_chain_state();
        cs.current_anchor_block.store(42, Ordering::Relaxed);
        assert_eq!(cs.current_anchor_block.load(Ordering::Relaxed), 42);
    }

    #[test]
    fn chain_state_provider_info_round_trips() {
        let (cs, _dir) = test_chain_state();
        *cs.provider_info.write() = Some(sample_provider_info());
        let guard = cs.provider_info.read();
        let info = guard.as_ref().unwrap();
        assert_eq!(info.settings.price_per_byte, 5);
        assert_eq!(info.committed_bytes, 500);
        assert_eq!(info.multiaddr, "/ip4/1.2.3.4/tcp/3333");
    }

    #[test]
    fn chain_state_nonce_counter_round_trips() {
        let (cs, _dir) = test_chain_state();
        assert!(cs.nonce_counter.read().is_none());
        let counter = Arc::new(NonceCounter::with_store(1, cs.nonce_store.clone()));
        counter.bootstrap_from_hsn(5);
        *cs.nonce_counter.write() = Some(counter);
        assert!(cs.nonce_counter.read().is_some());
    }

    /// The hand-written `Debug` impl exists because `NonceCounter` holds an
    /// `Arc<dyn NonceStore>`, which is not `Debug`; it must still show the two
    /// fields that matter when a counter is logged.
    #[test]
    fn nonce_counter_debug_shows_counter_and_bootstrap_state() {
        let (cs, _dir) = test_chain_state();
        let counter = NonceCounter::with_store(7, cs.nonce_store.clone());

        let before = format!("{counter:?}");
        assert!(before.starts_with("NonceCounter"));
        assert!(before.contains('7'), "current value missing: {before}");
        assert!(before.contains("false"), "should not be bootstrapped");

        counter.bootstrap_from_hsn(41);
        let after = format!("{counter:?}");
        assert!(after.contains("42"), "counter should be hsn + 1: {after}");
        assert!(after.contains("true"), "should be bootstrapped: {after}");
    }

    #[test]
    fn chain_state_constants_round_trips() {
        let (cs, _dir) = test_chain_state();
        assert!(cs.constants.read().is_none());
        *cs.constants.write() = Some(PalletConstants {
            request_timeout: 100,
        });
        assert_eq!(cs.constants.read().as_ref().unwrap().request_timeout, 100);
    }

    #[test]
    fn lifecycle_event_relevance_matches_on_provider() {
        let me = AccountId32::new([1u8; 32]);
        let other = AccountId32::new([2u8; 32]);
        let mine = ProviderLifecycleEvent::Updated {
            provider: me.clone(),
        };
        let theirs = ProviderLifecycleEvent::Deregistered { provider: other };
        assert!(is_relevant_provider_event(&mine, &me));
        assert!(!is_relevant_provider_event(&theirs, &me));
    }

    #[tokio::test(start_paused = true)]
    async fn with_timeout_expiry_maps_to_error() {
        // Paused time auto-advances past the budget instantly.
        let err = with_timeout(
            "Chain bootstrap",
            Duration::from_secs(5),
            std::future::pending::<Result<(), Error>>(),
        )
        .await
        .expect_err("a never-ready future must hit the budget");
        assert!(
            err.to_string()
                .contains("Chain bootstrap timed out after 5s"),
            "unexpected error: {err}"
        );
    }

    #[tokio::test]
    async fn with_timeout_passes_results_through() {
        let ok = with_timeout("op", Duration::from_secs(1), async { Ok::<_, Error>(7) })
            .await
            .expect("value passes through");
        assert_eq!(ok, 7);

        let err = with_timeout("op", Duration::from_secs(1), async {
            Err::<(), _>(Error::Internal("inner failure".to_string()))
        })
        .await
        .expect_err("inner error passes through");
        assert!(
            err.to_string().contains("inner failure"),
            "unexpected error: {err}"
        );
    }

    // ── mock chain follower ────────────────────────────────────────────────
    //
    // These drive [`ChainStateCoordinator`] end to end without a chain: a mock
    // [`ChainStateChainClient`] answers the bootstrap reads, and a mock
    // [`FinalizedBlocks`] hands `follow` a canned sequence of already-decoded
    // updates. Decoding itself (subxt bindings, SCALE) is provider-node's
    // concern now, so nothing here touches it.

    /// [`ChainStateChainClient`] returning fixed answers.
    struct MockChainClient {
        provider_info: Option<ProviderInfo>,
        replay_hsn: Option<u64>,
        request_timeout: Option<u32>,
    }

    #[async_trait]
    impl ChainStateChainClient for MockChainClient {
        async fn get_provider_info(
            &self,
            _who: &AccountId32,
        ) -> Result<Option<ProviderInfo>, Error> {
            Ok(self.provider_info.clone())
        }

        async fn fetch_replay_hsn(&self, _who: &AccountId32) -> Result<Option<u64>, Error> {
            Ok(self.replay_hsn)
        }

        async fn fetch_request_timeout(&self) -> Result<Option<u32>, Error> {
            Ok(self.request_timeout)
        }
    }

    /// [`FinalizedBlocks`] yielding a fixed queue of updates, then ending.
    struct MockFinalizedBlocks {
        updates: VecDeque<BlockUpdate>,
    }

    impl MockFinalizedBlocks {
        fn new(updates: Vec<BlockUpdate>) -> Self {
            Self {
                updates: updates.into(),
            }
        }
    }

    #[async_trait]
    impl FinalizedBlocks for MockFinalizedBlocks {
        async fn next(&mut self) -> Option<BlockUpdate> {
            self.updates.pop_front()
        }
    }

    /// [`ChainSession`] handing back a canned chain client and block stream.
    struct MockSession {
        chain: Arc<dyn ChainStateChainClient>,
        blocks: MockFinalizedBlocks,
    }

    #[async_trait]
    impl ChainSession for MockSession {
        async fn subscribe(
            self: Box<Self>,
        ) -> Result<(Box<dyn FinalizedBlocks>, Arc<dyn ChainStateChainClient>), Error> {
            let MockSession { chain, blocks } = *self;
            Ok((Box::new(blocks), chain))
        }
    }

    /// [`ChainFollower`] whose `connect()` must never be called - for tests
    /// that drive [`ChainStateCoordinator::follow`] directly with an
    /// already-built session.
    struct NeverConnectFollower;

    #[async_trait]
    impl ChainFollower for NeverConnectFollower {
        async fn connect(&self) -> Result<Box<dyn ChainSession>, Error> {
            unreachable!("connect() must not be called when driving follow() directly")
        }
    }

    /// [`ChainFollower`] whose `connect()` always fails, counting attempts.
    struct AlwaysFailFollower {
        attempts: Arc<AtomicUsize>,
    }

    #[async_trait]
    impl ChainFollower for AlwaysFailFollower {
        async fn connect(&self) -> Result<Box<dyn ChainSession>, Error> {
            self.attempts.fetch_add(1, Ordering::SeqCst);
            Err(Error::Internal("mock connect failure".to_string()))
        }
    }

    fn coordinator_over(
        follower: Arc<dyn ChainFollower>,
        chain_state: Arc<ChainState>,
        events_tx: BlockEventTx,
    ) -> ChainStateCoordinator {
        ChainStateCoordinator::new(follower, provider_account(), chain_state, events_tx)
    }

    #[tokio::test]
    async fn follow_processes_finalized_blocks_and_provider_events() {
        let account = provider_account();
        let info = sample_provider_info();

        let chain: Arc<dyn ChainStateChainClient> = Arc::new(MockChainClient {
            provider_info: Some(info.clone()),
            // No replay state yet: exercises the un-bootstrapped nonce path.
            replay_hsn: None,
            request_timeout: Some(100),
        });
        let blocks = MockFinalizedBlocks::new(vec![BlockUpdate::Block(FinalizedBlock {
            number: 42,
            anchor_block: Some(4242),
            events: vec![BlockEvent::ChallengeCreated {
                deadline: 777,
                index: 3,
                bucket_id: 9,
                provider: account.clone(),
            }],
            lifecycle: vec![ProviderLifecycleEvent::Updated {
                provider: account.clone(),
            }],
        })]);
        let session: Box<dyn ChainSession> = Box::new(MockSession { chain, blocks });

        let (chain_state, _dir) = test_chain_state();
        let chain_state = Arc::new(chain_state);
        let (events_tx, mut events_rx) = tokio::sync::broadcast::channel(16);
        let coordinator = coordinator_over(
            Arc::new(NeverConnectFollower),
            chain_state.clone(),
            events_tx,
        );

        coordinator
            .follow(session)
            .await
            .expect("follow runs to stream end");

        assert_eq!(
            chain_state.current_anchor_block.load(Ordering::Relaxed),
            4242
        );
        let stored = chain_state.provider_info.read();
        let stored = stored.as_ref().expect("provider info synced from chain");
        assert_eq!(stored.stake, info.stake);
        assert!(chain_state.constants.read().is_some());
        assert!(chain_state.nonce_counter.read().is_some());

        let mut saw_resubscribed = false;
        let mut saw_challenge = false;
        while let Ok(event) = events_rx.try_recv() {
            match event {
                BlockEvent::Resubscribed { .. } => saw_resubscribed = true,
                BlockEvent::ChallengeCreated {
                    deadline: 777,
                    index: 3,
                    bucket_id: 9,
                    ref provider,
                } if *provider == account => saw_challenge = true,
                _ => {}
            }
        }
        assert!(saw_resubscribed, "follow should broadcast Resubscribed");
        assert!(
            saw_challenge,
            "follow should forward the block's decoded events"
        );
    }

    #[tokio::test]
    async fn follow_broadcasts_membership_changes() {
        let chain: Arc<dyn ChainStateChainClient> = Arc::new(MockChainClient {
            provider_info: Some(sample_provider_info()),
            replay_hsn: None,
            request_timeout: Some(100),
        });
        // Duplicates included: invalidation is idempotent, and the fan-out
        // does not deduplicate.
        let blocks = MockFinalizedBlocks::new(vec![BlockUpdate::Block(FinalizedBlock {
            number: 1,
            anchor_block: Some(1),
            events: vec![
                BlockEvent::BucketMembershipChanged { bucket_id: 9 },
                BlockEvent::BucketMembershipChanged { bucket_id: 7 },
                BlockEvent::BucketMembershipChanged { bucket_id: 7 },
                BlockEvent::BucketMembershipChanged { bucket_id: 8 },
            ],
            lifecycle: vec![],
        })]);
        let session: Box<dyn ChainSession> = Box::new(MockSession { chain, blocks });

        let (chain_state, _dir) = test_chain_state();
        let (events_tx, mut events_rx) = tokio::sync::broadcast::channel(16);
        let coordinator = coordinator_over(
            Arc::new(NeverConnectFollower),
            Arc::new(chain_state),
            events_tx,
        );

        coordinator
            .follow(session)
            .await
            .expect("follow runs to stream end");

        let mut changed_buckets = Vec::new();
        while let Ok(event) = events_rx.try_recv() {
            if let BlockEvent::BucketMembershipChanged { bucket_id } = event {
                changed_buckets.push(bucket_id);
            }
        }
        assert_eq!(changed_buckets, vec![9, 7, 7, 8]);
    }

    #[tokio::test]
    async fn follow_continues_past_an_unreadable_block() {
        let chain: Arc<dyn ChainStateChainClient> = Arc::new(MockChainClient {
            provider_info: None,
            replay_hsn: None,
            request_timeout: None,
        });
        let blocks = MockFinalizedBlocks::new(vec![
            BlockUpdate::Unreadable { number: 10 },
            BlockUpdate::Block(FinalizedBlock {
                number: 11,
                anchor_block: Some(11),
                events: vec![],
                lifecycle: vec![],
            }),
        ]);
        let session: Box<dyn ChainSession> = Box::new(MockSession { chain, blocks });

        let (chain_state, _dir) = test_chain_state();
        let chain_state = Arc::new(chain_state);
        let (events_tx, mut events_rx) = tokio::sync::broadcast::channel(16);
        let coordinator = coordinator_over(
            Arc::new(NeverConnectFollower),
            chain_state.clone(),
            events_tx,
        );

        coordinator
            .follow(session)
            .await
            .expect("follow runs past the unreadable block to stream end");

        // The unreadable block escalates instead of being silently dropped;
        // the block behind it still gets processed.
        assert_eq!(chain_state.current_anchor_block.load(Ordering::Relaxed), 11);
        let mut saw_scope_unknown = false;
        while let Ok(event) = events_rx.try_recv() {
            if let BlockEvent::MembershipScopeUnknown { at_block: 10 } = event {
                saw_scope_unknown = true;
            }
        }
        assert!(
            saw_scope_unknown,
            "an unreadable block must escalate MembershipScopeUnknown"
        );
    }

    #[tokio::test]
    async fn follow_keeps_the_previous_anchor_block_when_a_read_fails() {
        let chain: Arc<dyn ChainStateChainClient> = Arc::new(MockChainClient {
            provider_info: None,
            replay_hsn: None,
            request_timeout: None,
        });
        let blocks = MockFinalizedBlocks::new(vec![
            BlockUpdate::Block(FinalizedBlock {
                number: 1,
                anchor_block: Some(100),
                events: vec![],
                lifecycle: vec![],
            }),
            BlockUpdate::Block(FinalizedBlock {
                number: 2,
                // A failed anchor read on this block must not reset the value.
                anchor_block: None,
                events: vec![],
                lifecycle: vec![],
            }),
        ]);
        let session: Box<dyn ChainSession> = Box::new(MockSession { chain, blocks });

        let (chain_state, _dir) = test_chain_state();
        let chain_state = Arc::new(chain_state);
        let (events_tx, _events_rx) = tokio::sync::broadcast::channel(16);
        let coordinator = coordinator_over(
            Arc::new(NeverConnectFollower),
            chain_state.clone(),
            events_tx,
        );

        coordinator
            .follow(session)
            .await
            .expect("follow runs to stream end");

        assert_eq!(
            chain_state.current_anchor_block.load(Ordering::Relaxed),
            100
        );
    }

    #[tokio::test(start_paused = true)]
    async fn run_retries_after_a_failed_connect() {
        const RECONNECT_DELAY: Duration = Duration::from_secs(5);

        let attempts = Arc::new(AtomicUsize::new(0));
        let follower: Arc<dyn ChainFollower> = Arc::new(AlwaysFailFollower {
            attempts: attempts.clone(),
        });
        let (chain_state, _dir) = test_chain_state();
        let coordinator = coordinator_over(
            follower,
            Arc::new(chain_state),
            tokio::sync::broadcast::channel(16).0,
        );
        let handle = coordinator.start();

        // Drive the paused clock through several reconnect cycles: each
        // failed `connect()` is followed by a `RECONNECT_DELAY` sleep before
        // `run()` tries again.
        for _ in 0..3 {
            tokio::time::advance(RECONNECT_DELAY).await;
        }

        assert!(
            attempts.load(Ordering::SeqCst) >= 2,
            "run() must retry connect() after a failure, got {} attempts",
            attempts.load(Ordering::SeqCst)
        );

        handle.stop().await;
    }
}
