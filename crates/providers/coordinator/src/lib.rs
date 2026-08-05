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
//! drives a finalized-block subscription on its own subxt connection in a
//! reconnect loop; on every relevant provider event it re-fetches the full
//! `ProviderInfo` so `committed_bytes`, `stake`, and all settings stay
//! current — no field-patching, no partial updates, no second writer.

use async_trait::async_trait;
use parking_lot::RwLock;
use provider_chain::chain_connection::{self, ChainHandle, ChainTransport};
use provider_chain::chain_events::{self, BlockEvent, BlockEventTx};
use provider_storage::{NonceStore, NullNonceStore};
use serde::{Deserialize, Serialize};
use sp_runtime::AccountId32;
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering};
use std::sync::Arc;
use std::time::Duration;
use storage_subxt::api::runtime_types::pallet_storage_provider::pallet::ProviderInfo as RuntimeProviderInfo;
use subxt::{OnlineClient, PolkadotConfig};
use tokio::sync::watch;
use tokio::task::JoinHandle;

/// Pallet whose storage, constants, and events the coordinator follows.
const PALLET_NAME: &str = "StorageProvider";

// ── Error ─────────────────────────────────────────────────────────────────────

/// Errors surfaced by the chain-state coordinator.
#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("Internal error: {0}")]
    Internal(String),
}

impl From<provider_chain::Error> for Error {
    fn from(e: provider_chain::Error) -> Self {
        Error::Internal(e.to_string())
    }
}

// ── On-chain Provider Info ────────────────────────────────────────────────────

/// The node's view of its on-chain provider registration.
///
/// Decoded from the `StorageProvider::Providers` storage entry by the
/// chain-state coordinator; consumed by `/negotiate` validation and `/info`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProviderInfo {
    /// Network address for connecting.
    pub multiaddr: String,
    /// Total stake locked.
    pub stake: u128,
    /// Currently committed bytes.
    pub committed_bytes: u64,
    /// Maximum capacity (0 = unlimited).
    pub max_capacity: u64,
    /// Minimum agreement duration.
    pub min_duration: u32,
    /// Maximum agreement duration.
    pub max_duration: u32,
    /// Price per byte per block.
    pub price_per_byte: u128,
    /// Whether accepting primary agreements.
    pub accepting_primary: bool,
    /// Replica sync price (None if not accepting replicas).
    pub replica_sync_price: Option<u128>,
    /// Whether accepting extensions.
    pub accepting_extensions: bool,
    /// Total agreements ever.
    pub agreements_total: u32,
    /// Failed challenges count.
    pub challenges_failed: u32,
    /// Block at which deregistration becomes finalisable (`None` = not deregistering).
    pub deregister_at: Option<u32>,
}

impl From<RuntimeProviderInfo> for ProviderInfo {
    /// Flatten the runtime's nested `ProviderInfo` into the node's view.
    ///
    /// `public_key` and the six unused `stats` counters are deliberately
    /// dropped — they are not part of what `/negotiate` or `/info` expose.
    fn from(info: RuntimeProviderInfo) -> Self {
        Self {
            multiaddr: String::from_utf8_lossy(&info.multiaddr.0).into_owned(),
            stake: info.stake,
            committed_bytes: info.committed_bytes,
            max_capacity: info.settings.max_capacity,
            min_duration: info.settings.min_duration,
            max_duration: info.settings.max_duration,
            price_per_byte: info.settings.price_per_byte,
            accepting_primary: info.settings.accepting_primary,
            replica_sync_price: info.settings.replica_sync_price,
            accepting_extensions: info.settings.accepting_extensions,
            agreements_total: info.stats.agreements_total,
            challenges_failed: info.stats.challenges_failed,
            deregister_at: info.deregister_at,
        }
    }
}

// ── ChainState ────────────────────────────────────────────────────────────────

/// Live chain state kept in sync with the runtime by the chain-state coordinator.
///
/// Held behind `Arc` inside the provider node's `ProviderState` so the
/// coordinator can hold its own handle without a back-reference to the whole
/// node state.
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
    /// Persistence backing for the nonce counter. In disk mode this is a
    /// `DiskNonceStore`; in in-memory mode it is the no-op `NullNonceStore`.
    /// The coordinator uses it to seed a restarted counter above the last
    /// issued nonce.
    pub nonce_store: Arc<dyn NonceStore>,
}

impl Default for ChainState {
    fn default() -> Self {
        Self::with_nonce_store(Arc::new(NullNonceStore))
    }
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
/// (on connect and on every relevant provider event), so the counter resumes at
/// `max(persisted_local, hsn + 1)`:
///
/// * **Local persistence** (disk mode): each allocation is persisted before
///   returning, so a **clean process restart** does not reissue nonces that were
///   signed but not yet redeemed. Power-loss/kernel-panic may lose the last write
///   (RocksDB WAL is not fsynced per allocation); in that case the counter falls
///   back to `chain_hsn + 1`, which is still safe — the chain's replay window
///   rejects any duplicate redemption.
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
    /// Create a counter starting at `start` with a no-op store.
    ///
    /// All existing call sites use this constructor; no persistence occurs
    /// (in-memory mode, or tests). The counter is *not* considered bootstrapped
    /// until [`Self::bootstrap_from_hsn`] aligns it with the chain.
    pub fn new(start: u64) -> Self {
        Self {
            counter: AtomicU64::new(start),
            bootstrapped: AtomicBool::new(false),
            store: Arc::new(NullNonceStore),
        }
    }

    /// Create a counter starting at `start` backed by `store` for persistence.
    ///
    /// Use this in disk mode: seed `start` from `store.load().unwrap_or(1)`
    /// (the persisted high-water mark), then call `bootstrap_from_hsn` to
    /// advance past the chain's replay head.
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

// ── anchor block ──────────────────────────────────────────────────────────────

/// Query the pallet's `StorageProviderApi::current_anchor_block` runtime API —
/// the block every on-chain duration (timeouts, expiries, `valid_until`, nonce
/// age) is measured against. Reading it through the runtime API keeps the
/// provider agnostic to whether the anchor is a relay, parachain, or other
/// block number: the pallet decides via its `BlockNumberProvider`, and the
/// provider no longer reaches into a specific storage item.
///
/// Kept here (rather than in `storage-client`) so the provider node stays
/// dependency-light (see #275).
pub async fn fetch_current_anchor_block<C>(
    at: &subxt::client::ClientAtBlock<subxt::PolkadotConfig, C>,
) -> Result<u32, Error>
where
    C: subxt::client::OnlineClientAtBlockT<subxt::PolkadotConfig>,
{
    at.runtime_apis()
        .call(
            storage_subxt::api::runtime_apis()
                .storage_provider_api()
                .current_anchor_block(),
        )
        .await
        .map_err(|e| Error::Internal(format!("current_anchor_block runtime API call failed: {e}")))
}

// ── chain reads ───────────────────────────────────────────────────────────────

/// The chain reads the coordinator needs to keep [`ChainState`] in sync.
///
/// Abstracted behind a trait — exactly like the other coordinators'
/// `*ChainClient` traits — so the [`sync_constants`] / [`refresh_provider_state`]
/// logic can be driven by a mock in tests without a live chain.
#[async_trait]
pub trait ChainStateChainClient: Send + Sync {
    /// Full on-chain `ProviderInfo`, or `None` if the provider is not registered.
    async fn get_provider_info(&self, who: &AccountId32) -> Result<Option<ProviderInfo>, Error>;

    /// Provider's replay-window head sequence (`hsn`), or `None` if no replay
    /// state exists yet (the provider has never signed any terms).
    async fn fetch_replay_hsn(&self, who: &AccountId32) -> Result<Option<u64>, Error>;

    /// `StorageProvider::RequestTimeout` runtime constant, or `None` if absent
    /// from the node's metadata.
    async fn fetch_request_timeout(&self) -> Result<Option<u32>, Error>;
}

/// Production [`ChainStateChainClient`] running typed storage queries — through
/// the generated `storage-subxt` bindings — on the coordinator's own subxt
/// connection (shared with the block subscription).
struct RealChainStateClient {
    api: OnlineClient<PolkadotConfig>,
}

/// Convert an account from the `sp_runtime` representation the node uses into
/// the `subxt` one the generated bindings expect. Same 32 bytes either way.
fn subxt_account(who: &AccountId32) -> subxt::utils::AccountId32 {
    subxt::utils::AccountId32(*<AccountId32 as AsRef<[u8; 32]>>::as_ref(who))
}

#[async_trait]
impl ChainStateChainClient for RealChainStateClient {
    async fn get_provider_info(&self, who: &AccountId32) -> Result<Option<ProviderInfo>, Error> {
        let addr = storage_subxt::api::storage().storage_provider().providers();
        let at = self
            .api
            .at_current_block()
            .await
            .map_err(|e| Error::Internal(format!("Failed to get storage: {e}")))?;
        let Some(value) = at
            .storage()
            .try_fetch(addr, (subxt_account(who),))
            .await
            .map_err(|e| Error::Internal(format!("Failed to fetch Providers: {e}")))?
        else {
            return Ok(None);
        };
        let info = value
            .decode()
            .map_err(|e| Error::Internal(format!("Failed to decode Providers: {e}")))?;
        Ok(Some(ProviderInfo::from(info)))
    }

    async fn fetch_replay_hsn(&self, who: &AccountId32) -> Result<Option<u64>, Error> {
        let addr = storage_subxt::api::storage()
            .storage_provider()
            .provider_replay_states();
        let at = self
            .api
            .at_current_block()
            .await
            .map_err(|e| Error::Internal(format!("Failed to get storage: {e}")))?;
        let Some(value) = at
            .storage()
            .try_fetch(addr, (subxt_account(who),))
            .await
            .map_err(|e| Error::Internal(format!("Failed to fetch ProviderReplayStates: {e}")))?
        else {
            return Ok(None);
        };
        let window = value
            .decode()
            .map_err(|e| Error::Internal(format!("Failed to decode ProviderReplayStates: {e}")))?;
        Ok(Some(window.hsn))
    }

    async fn fetch_request_timeout(&self) -> Result<Option<u32>, Error> {
        let at = self
            .api
            .at_current_block()
            .await
            .map_err(|e| Error::Internal(format!("Failed to get current block: {e}")))?;

        match at.constants().entry(
            storage_subxt::api::constants()
                .storage_provider()
                .request_timeout(),
        ) {
            Ok(timeout) => Ok(Some(timeout)),
            // A runtime without the constant is a different thing from a failed
            // read: the caller logs it as a metadata gap and leaves the pallet
            // constants unset, rather than treating it as a chain error.
            Err(
                subxt::error::ConstantError::PalletNameNotFound(_)
                | subxt::error::ConstantError::ConstantNameNotFound { .. },
            ) => Ok(None),
            Err(e) => Err(Error::Internal(format!(
                "Failed to read RequestTimeout: {e}"
            ))),
        }
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

/// Decode a finalized block's events down to the provider-lifecycle events.
fn parse_provider_lifecycle_events(
    events: &subxt::events::Events<PolkadotConfig>,
) -> Vec<ProviderLifecycleEvent> {
    use storage_subxt::api::storage_provider::events as ev;

    let updated = |account: subxt::utils::AccountId32| ProviderLifecycleEvent::Updated {
        provider: AccountId32::new(account.0),
    };

    events
        .iter()
        .filter_map(|event| event.ok())
        .filter(|event| event.pallet_name() == PALLET_NAME)
        .filter_map(|event| {
            decode::<ev::ProviderDeregistered>(&event)
                .map(|e| ProviderLifecycleEvent::Deregistered {
                    provider: AccountId32::new(e.provider.0),
                })
                .or_else(|| decode::<ev::ProviderRegistered>(&event).map(|e| updated(e.provider)))
                .or_else(|| {
                    decode::<ev::ProviderSettingsUpdated>(&event).map(|e| updated(e.provider))
                })
                .or_else(|| {
                    decode::<ev::ProviderMultiaddrUpdated>(&event).map(|e| updated(e.provider))
                })
                .or_else(|| decode::<ev::DeregisterAnnounced>(&event).map(|e| updated(e.provider)))
                .or_else(|| decode::<ev::DeregisterCancelled>(&event).map(|e| updated(e.provider)))
        })
        .collect()
}

/// Statically decode `event` as `E` when its pallet/event identity matches;
/// `None` otherwise. An event that matches but fails to decode (a runtime whose
/// event shape drifted from the bindings) is logged and skipped — the next
/// relevant event, or the next reconnect's bootstrap refresh, covers the miss.
fn decode<E: subxt::events::DecodeAsEvent>(
    event: &subxt::events::Event<'_, PolkadotConfig>,
) -> Option<E> {
    match event.decode_fields_as::<E>()? {
        Ok(decoded) => Some(decoded),
        Err(e) => {
            tracing::warn!(
                "chain-state coordinator: failed to decode {}::{} against the static bindings: {e}",
                event.pallet_name(),
                event.event_name(),
            );
            None
        }
    }
}

// ── ChainStateCoordinator ─────────────────────────────────────────────────────

/// Builds and starts the live chain-state synchronisation for a single provider.
///
/// Start with [`ChainStateCoordinator::start`]; keep the returned
/// [`ChainStateCoordinatorHandle`] alive for the duration of the server.
pub struct ChainStateCoordinator {
    transport: ChainTransport,
    provider_account: AccountId32,
    chain_state: Arc<ChainState>,
    /// Publishes the live connection to every chain consumer. This coordinator
    /// is the only writer: it rebuilds the connection on stream loss or stall
    /// and everyone else picks up the new handle from the watch channel.
    chain_tx: watch::Sender<Option<ChainHandle>>,
    /// Fan-out of decoded per-block events to the background coordinators.
    events_tx: BlockEventTx,
}

impl ChainStateCoordinator {
    pub fn new(
        transport: ChainTransport,
        provider_account: AccountId32,
        chain_state: Arc<ChainState>,
        chain_tx: watch::Sender<Option<ChainHandle>>,
        events_tx: BlockEventTx,
    ) -> Self {
        Self {
            transport,
            provider_account,
            chain_state,
            chain_tx,
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
    /// stream until it ends or stalls. Returns `Err` if connecting fails; `Ok(())`
    /// if the stream terminates — either way the caller reconnects.
    async fn connect_and_follow(&self) -> Result<(), Error> {
        let handle = chain_connection::connect(&self.transport).await?;
        self.follow(handle).await
    }

    /// Bootstrap state from the connection and follow its finalized blocks
    /// until the stream ends or stalls. Split from
    /// [`Self::connect_and_follow`] so tests can drive the full pipeline over
    /// a mock RPC connection.
    async fn follow(&self, handle: ChainHandle) -> Result<(), Error> {
        /// How long without a finalized block before the connection is treated
        /// as dead and rebuilt. Finality can pause briefly (session boundaries,
        /// backend resubscriptions), so this is several times the block time;
        /// a genuinely stalled stream otherwise hangs forever with no error.
        const STALL_TIMEOUT: Duration = Duration::from_secs(60);

        let api = handle.api.clone();
        let mut blocks = api
            .stream_blocks()
            .await
            .map_err(|e| Error::Internal(format!("Failed to subscribe to blocks: {e}")))?;

        // Publish the new connection only after the block stream is up, so
        // consumers never observe a handle whose backend failed immediately.
        self.chain_tx.send_replace(Some(handle));
        let chain = RealChainStateClient { api };

        tracing::info!("chain-state coordinator: connected; following finalized blocks");

        // Fetch pallet constants once per connection (they only change on runtime upgrade).
        sync_constants(&chain, &self.chain_state).await;

        // Bootstrap from any existing on-chain state so a restarted node that was
        // already registered picks up its provider_info and nonce counter immediately
        // rather than waiting for the next relevant event.
        refresh_provider_state(&chain, &self.chain_state, &self.provider_account).await;

        // Tell coordinators to reconcile: events emitted while the stream was
        // down were missed for good, so they re-scan chain state instead.
        let _ = self.events_tx.send(BlockEvent::Resubscribed {
            at_block: self
                .chain_state
                .current_anchor_block
                .load(std::sync::atomic::Ordering::Relaxed),
        });

        loop {
            let next = match tokio::time::timeout(STALL_TIMEOUT, blocks.next()).await {
                Ok(Some(next)) => next,
                Ok(None) => break,
                Err(_) => {
                    tracing::warn!(
                        "chain-state coordinator: no finalized block for {}s; rebuilding connection",
                        STALL_TIMEOUT.as_secs()
                    );
                    break;
                }
            };
            let block = match next {
                Ok(block) => block,
                Err(e) => {
                    tracing::warn!("chain-state coordinator: block subscription error: {e}");
                    break;
                }
            };
            let block_number = block.number() as u32;

            tracing::debug!("Finalized block: {}", block_number);

            // One block-scoped handle drives both reads below.
            let at = match block.at().await {
                Ok(at) => at,
                Err(e) => {
                    tracing::warn!(
                        "chain-state coordinator: failed to get block handle for {block_number}: {e}"
                    );
                    continue;
                }
            };

            // Track the pallet's anchor block (the clock all on-chain durations
            // are measured against) at this finalized block, via its runtime
            // API — so the provider never needs to know which block notion the
            // pallet uses.
            match fetch_current_anchor_block(&at).await {
                Ok(anchor_block) => {
                    self.chain_state
                        .current_anchor_block
                        .store(anchor_block, std::sync::atomic::Ordering::Relaxed);
                }
                Err(e) => tracing::warn!(
                    "chain-state coordinator: failed to fetch anchor block for block \
                     {block_number}: {e}; keeping previous value"
                ),
            }

            let events = match at.events().fetch().await {
                Ok(events) => events,
                Err(e) => {
                    tracing::warn!(
                        "chain-state coordinator: failed to fetch events for block {block_number}: {e}"
                    );
                    continue;
                }
            };

            // Fan out the coordinator-relevant events. Send failures just mean
            // no coordinator is subscribed.
            for event in chain_events::decode_block_events(&events) {
                let _ = self.events_tx.send(event);
            }

            let parsed = parse_provider_lifecycle_events(&events);
            self.process_provider_events(&chain, &parsed, block_number)
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

// ── tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::Ordering;
    use subxt::ext::scale_value::Value;

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

    #[test]
    fn chain_state_defaults_to_unknown() {
        let cs = ChainState::default();
        assert_eq!(cs.current_anchor_block.load(Ordering::Relaxed), 0);
        assert!(cs.constants.read().is_none());
        assert!(cs.provider_info.read().is_none());
        assert!(cs.nonce_counter.read().is_none());
    }

    #[test]
    fn chain_state_current_anchor_block_round_trips() {
        let cs = ChainState::default();
        cs.current_anchor_block.store(42, Ordering::Relaxed);
        assert_eq!(cs.current_anchor_block.load(Ordering::Relaxed), 42);
    }

    #[test]
    fn chain_state_provider_info_round_trips() {
        let cs = ChainState::default();
        *cs.provider_info.write() = Some(sample_provider_info());
        let guard = cs.provider_info.read();
        let info = guard.as_ref().unwrap();
        assert_eq!(info.price_per_byte, 5);
        assert_eq!(info.committed_bytes, 500);
        assert_eq!(info.multiaddr, "/ip4/1.2.3.4/tcp/3333");
    }

    #[test]
    fn chain_state_nonce_counter_round_trips() {
        let cs = ChainState::default();
        assert!(cs.nonce_counter.read().is_none());
        let counter = Arc::new(NonceCounter::new(1));
        counter.bootstrap_from_hsn(5);
        *cs.nonce_counter.write() = Some(counter);
        assert!(cs.nonce_counter.read().is_some());
    }

    #[test]
    fn chain_state_constants_round_trips() {
        let cs = ChainState::default();
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

    // ── real subxt client over a mock RPC connection ──────────────────────
    //
    // These tests drive [`RealChainStateClient`] and [`ChainStateCoordinator::follow`]
    // through a real `OnlineClient` (legacy backend) backed by canned RPC
    // responses, using the repo's tracked runtime metadata snapshot. Storage
    // values and events are round-tripped through `scale_value` encoding
    // against the actual runtime types, so these exercise the same dynamic
    // decode paths as a live chain — without one.
    mod real_client {
        use super::*;
        use std::sync::atomic::Ordering;
        use subxt::backend::LegacyBackend;
        use subxt::ext::scale_value::scale::encode_as_type;
        use subxt_rpcs::client::mock_rpc_client::Json;
        use subxt_rpcs::client::{MockRpcClient, RpcClient};

        /// Tracked runtime metadata snapshot (shared with the PAPI codegen).
        const METADATA: &[u8] = include_bytes!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../../packages/papi/.papi/metadata/parachain.scale"
        ));
        const BLOCK_HASH: &str =
            "0x2222222222222222222222222222222222222222222222222222222222222222";
        const GENESIS_HASH: &str =
            "0x1111111111111111111111111111111111111111111111111111111111111111";

        fn metadata() -> subxt::Metadata {
            use codec::Decode;
            subxt::Metadata::decode(&mut &METADATA[..]).expect("tracked metadata decodes")
        }

        fn provider_account() -> AccountId32 {
            AccountId32::new([7u8; 32])
        }

        /// `0x`-prefixed twox128(pallet) ++ twox128(entry) storage-key prefix.
        fn key_prefix(pallet: &str, entry: &str) -> String {
            let mut key = sp_core::twox_128(pallet.as_bytes()).to_vec();
            key.extend(sp_core::twox_128(entry.as_bytes()));
            format!("0x{}", hex::encode(key))
        }

        /// Look up the value type of a storage entry in the runtime metadata.
        fn storage_value_type(md: &subxt::Metadata, pallet: &str, entry: &str) -> u32 {
            md.pallet_by_name(pallet)
                .expect("pallet in metadata")
                .storage()
                .expect("pallet has storage")
                .entry_by_name(entry)
                .expect("entry in metadata")
                .value_ty()
        }

        /// SCALE-encode a dynamic value as the given runtime type.
        fn encode_value(md: &subxt::Metadata, ty: u32, value: &Value) -> Vec<u8> {
            let mut out = Vec::new();
            encode_as_type(value, ty, md.types(), &mut out).expect("value encodes as type");
            out
        }

        /// A `Providers` storage value matching the full runtime `ProviderInfo`
        /// shape: every runtime field must be present for `scale_value` to
        /// encode it against the real type.
        fn runtime_provider_info_value(
            replica_sync_price: Option<u128>,
            deregister_at: Option<u32>,
        ) -> Value {
            let opt = |val: Option<u128>| match val {
                Some(v) => Value::unnamed_variant("Some", vec![Value::u128(v)]),
                None => Value::unnamed_variant("None", Vec::<Value>::new()),
            };
            Value::named_composite([
                ("multiaddr", Value::from_bytes("/ip4/1.2.3.4/tcp/3333")),
                ("public_key", Value::from_bytes([9u8; 32])),
                ("stake", Value::u128(1_000)),
                ("committed_bytes", Value::u128(500)),
                (
                    "settings",
                    Value::named_composite([
                        ("min_duration", Value::u128(10)),
                        ("max_duration", Value::u128(100)),
                        ("price_per_byte", Value::u128(5)),
                        ("accepting_primary", Value::bool(true)),
                        ("replica_sync_price", opt(replica_sync_price)),
                        ("accepting_extensions", Value::bool(true)),
                        ("max_capacity", Value::u128(10_000)),
                    ]),
                ),
                (
                    "stats",
                    Value::named_composite([
                        ("registered_at", Value::u128(1)),
                        ("agreements_total", Value::u128(3)),
                        ("agreements_extended", Value::u128(0)),
                        ("agreements_not_extended", Value::u128(0)),
                        ("agreements_burned", Value::u128(0)),
                        ("total_bytes_committed", Value::u128(500)),
                        ("challenges_received", Value::u128(2)),
                        ("challenges_failed", Value::u128(1)),
                    ]),
                ),
                ("deregister_at", opt(deregister_at.map(u128::from))),
            ])
        }

        /// Wrap a `StorageProvider` event value in an `EventRecord`.
        fn event_record(event: Value) -> Value {
            Value::named_composite([
                ("phase", Value::unnamed_variant("Initialization", vec![])),
                (
                    "event",
                    Value::unnamed_variant("StorageProvider", vec![event]),
                ),
                ("topics", Value::unnamed_composite(Vec::<Value>::new())),
            ])
        }

        /// `System::Events` bytes holding a `ProviderRegistered` (exercising
        /// the dynamic lifecycle decoding) and a `ChallengeCreated`
        /// (exercising the static fan-out decoding) for `provider`, encoded
        /// against the real runtime types.
        fn encoded_events(md: &subxt::Metadata, provider: &AccountId32) -> Vec<u8> {
            let provider_bytes = <AccountId32 as AsRef<[u8]>>::as_ref(provider);
            let registered = event_record(Value::named_variant(
                "ProviderRegistered",
                [
                    ("provider", Value::from_bytes(provider_bytes)),
                    ("stake", Value::u128(1_000)),
                ],
            ));
            let challenge_created = event_record(Value::named_variant(
                "ChallengeCreated",
                [
                    (
                        "challenge_id",
                        Value::named_composite([
                            ("deadline", Value::u128(777)),
                            ("index", Value::u128(3)),
                        ]),
                    ),
                    ("bucket_id", Value::u128(9)),
                    ("provider", Value::from_bytes(provider_bytes)),
                    ("challenger", Value::from_bytes([8u8; 32])),
                    ("respond_by", Value::u128(777)),
                ],
            ));
            let ty = storage_value_type(md, "System", "Events");
            encode_value(
                md,
                ty,
                &Value::unnamed_composite([registered, challenge_created]),
            )
        }

        fn header_json(number: u32) -> serde_json::Value {
            serde_json::json!({
                "parentHash": GENESIS_HASH,
                "number": format!("{number:#x}"),
                "stateRoot": GENESIS_HASH,
                "extrinsicsRoot": GENESIS_HASH,
                "digest": { "logs": [] }
            })
        }

        fn runtime_version_json() -> serde_json::Value {
            serde_json::json!({
                "specName": "test",
                "implName": "test",
                "authoringVersion": 1,
                "specVersion": 1,
                "implVersion": 1,
                "apis": [],
                "transactionVersion": 1,
                "stateVersion": 1
            })
        }

        /// Build a real `OnlineClient` over a mock RPC connection.
        ///
        /// `storage` maps a storage-key prefix (see [`key_prefix`]) to the
        /// hex value served for reads under it; unmapped keys read as absent.
        async fn mock_api(
            storage: Vec<(String, String)>,
        ) -> subxt::OnlineClient<subxt::PolkadotConfig> {
            let metadata_hex = format!("0x{}", hex::encode(METADATA));
            let mock = MockRpcClient::builder()
                .method_handler("state_getMetadata", move |_params| {
                    let metadata_hex = metadata_hex.clone();
                    async move { Json(metadata_hex) }
                })
                .method_handler("state_call", move |params| async move {
                    use codec::Encode;
                    let raw = params.map(|p| p.get().to_string()).unwrap_or_default();
                    let function: String = serde_json::from_str::<Vec<serde_json::Value>>(&raw)
                        .ok()
                        .and_then(|p| p.first().and_then(|f| f.as_str().map(str::to_string)))
                        .unwrap_or_default();
                    let response = match function.as_str() {
                        // The runtime metadata version(s) this "node" serves:
                        // exactly the tracked snapshot's version.
                        "Metadata_metadata_versions" => vec![u32::from(METADATA[4])].encode(),
                        "Metadata_metadata_at_version" => Some(METADATA.to_vec()).encode(),
                        "Metadata_metadata" => METADATA.to_vec().encode(),
                        // Anchor block the coordinator reads per finalized block.
                        // Deliberately distinct from the mocked header number
                        // (42) so the assertion below fails if the coordinator
                        // ever regresses to storing the parachain height.
                        "StorageProviderApi_current_anchor_block" => 4242u32.encode(),
                        // sp_version::RuntimeVersion, field by field.
                        "Core_version" => (
                            "test".to_string(),           // spec_name
                            "test".to_string(),           // impl_name
                            1u32,                         // authoring_version
                            1u32,                         // spec_version
                            1u32,                         // impl_version
                            Vec::<([u8; 8], u32)>::new(), // apis
                            1u32,                         // transaction_version
                            1u8,                          // system_version
                        )
                            .encode(),
                        other => panic!("mock RPC: unhandled state_call {other}"),
                    };
                    Json(format!("0x{}", hex::encode(response)))
                })
                .method_handler("chain_getBlockHash", |_params| async {
                    Json(GENESIS_HASH.to_string())
                })
                .method_handler("chain_getFinalizedHead", |_params| async {
                    Json(BLOCK_HASH.to_string())
                })
                .method_handler("chain_getHeader", |_params| async { Json(header_json(42)) })
                .method_handler("state_getRuntimeVersion", |_params| async {
                    Json(runtime_version_json())
                })
                .method_handler("state_getStorage", move |params| {
                    let storage = storage.clone();
                    async move {
                        let key: String = params
                            .map(|p| {
                                let (key, _rest): (String, serde_json::Value) =
                                    serde_json::from_str(p.get())
                                        .or_else(|_| {
                                            serde_json::from_str::<(String,)>(p.get())
                                                .map(|(k,)| (k, serde_json::Value::Null))
                                        })
                                        .expect("storage params decode");
                                key
                            })
                            .unwrap_or_default();
                        let value = storage
                            .iter()
                            .find(|(prefix, _)| key.starts_with(prefix.as_str()))
                            .map(|(_, value)| value.clone());
                        Json(value)
                    }
                })
                .subscription_handler("chain_subscribeFinalizedHeads", |_params, _unsub| async {
                    vec![Json(header_json(42))]
                })
                .subscription_handler("state_subscribeRuntimeVersion", |_params, _unsub| async {
                    vec![Json(runtime_version_json())]
                })
                .method_fallback(|name, _params| async move {
                    panic!("mock RPC: unhandled method {name}");
                    #[allow(unreachable_code)]
                    Json(serde_json::Value::Null)
                })
                .subscription_fallback(|name, _params, _unsub| async move {
                    panic!("mock RPC: unhandled subscription {name}");
                    #[allow(unreachable_code)]
                    Vec::<Json<serde_json::Value>>::new()
                })
                .build();

            let backend = LegacyBackend::builder().build(RpcClient::new(mock));
            subxt::OnlineClient::<subxt::PolkadotConfig>::from_backend(Arc::new(backend))
                .await
                .expect("client over mock RPC")
        }

        #[tokio::test]
        async fn request_timeout_constant_reads_from_real_metadata() {
            let md = metadata();
            let client = RealChainStateClient {
                api: mock_api(vec![]).await,
            };

            let timeout = client
                .fetch_request_timeout()
                .await
                .expect("constant fetch succeeds")
                .expect("RequestTimeout present in metadata");

            // Self-consistency: the dynamic lookup must agree with the raw
            // constant bytes in the same metadata.
            let expected = {
                use codec::Decode;
                let constant = md
                    .pallet_by_name(PALLET_NAME)
                    .expect("pallet in metadata")
                    .constant_by_name("RequestTimeout")
                    .expect("constant in metadata");
                u32::decode(&mut constant.value()).expect("u32 constant")
            };
            assert_eq!(timeout, expected);
        }

        #[tokio::test]
        async fn provider_info_absent_reads_as_none() {
            let client = RealChainStateClient {
                api: mock_api(vec![]).await,
            };
            let info = client
                .get_provider_info(&provider_account())
                .await
                .expect("storage fetch succeeds");
            assert!(info.is_none());
        }

        /// A `Providers` entry whose bytes don't match the runtime type must
        /// error, not decode to a half-populated `ProviderInfo`. The dynamic
        /// decoder this replaced silently defaulted `multiaddr`,
        /// `replica_sync_price`, `deregister_at` and the two stats counters on
        /// a field miss, which let a runtime mismatch degrade quietly.
        #[tokio::test]
        async fn provider_info_decode_failure_is_an_error() {
            let client = RealChainStateClient {
                api: mock_api(vec![(key_prefix(PALLET_NAME, "Providers"), "0x00".into())]).await,
            };
            let err = client
                .get_provider_info(&provider_account())
                .await
                .expect_err("malformed Providers bytes must not decode");
            let Error::Internal(msg) = &err;
            assert!(
                msg.contains("decode Providers"),
                "unexpected error: {err:?}"
            );
        }

        /// Same for the replay window: a present-but-undecodable entry is an
        /// error, not `Ok(None)`. Collapsing it to `None` would look identical
        /// to "provider has never signed", which seeds the nonce counter
        /// differently.
        #[tokio::test]
        async fn replay_hsn_decode_failure_is_an_error() {
            let client = RealChainStateClient {
                api: mock_api(vec![(
                    key_prefix(PALLET_NAME, "ProviderReplayStates"),
                    "0x00".into(),
                )])
                .await,
            };
            let err = client
                .fetch_replay_hsn(&provider_account())
                .await
                .expect_err("malformed ProviderReplayStates bytes must not decode");
            let Error::Internal(msg) = &err;
            assert!(
                msg.contains("decode ProviderReplayStates"),
                "unexpected error: {err:?}"
            );
        }

        #[tokio::test]
        async fn provider_info_round_trips_through_runtime_types() {
            let md = metadata();
            let ty = storage_value_type(&md, PALLET_NAME, "Providers");
            let encoded = encode_value(&md, ty, &runtime_provider_info_value(Some(7), Some(42)));

            let client = RealChainStateClient {
                api: mock_api(vec![(
                    key_prefix(PALLET_NAME, "Providers"),
                    format!("0x{}", hex::encode(encoded)),
                )])
                .await,
            };

            let info = client
                .get_provider_info(&provider_account())
                .await
                .expect("storage fetch succeeds")
                .expect("provider info decodes");
            assert_eq!(info.multiaddr, "/ip4/1.2.3.4/tcp/3333");
            assert_eq!(info.stake, 1_000);
            assert_eq!(info.max_capacity, 10_000);
            assert_eq!(info.replica_sync_price, Some(7));
            assert_eq!(info.deregister_at, Some(42));
        }

        #[tokio::test]
        async fn follow_processes_finalized_blocks_and_provider_events() {
            let md = metadata();
            let account = provider_account();

            let providers_ty = storage_value_type(&md, PALLET_NAME, "Providers");
            let provider_bytes =
                encode_value(&md, providers_ty, &runtime_provider_info_value(None, None));
            let events_bytes = encoded_events(&md, &account);

            let api = mock_api(vec![
                (
                    key_prefix("System", "Events"),
                    format!("0x{}", hex::encode(events_bytes)),
                ),
                (
                    key_prefix(PALLET_NAME, "Providers"),
                    format!("0x{}", hex::encode(provider_bytes)),
                ),
                // ProviderReplayStates intentionally unmapped: reads as absent,
                // covering the no-replay-state nonce bootstrap path.
            ])
            .await;

            let chain_state = Arc::new(ChainState::default());
            let (chain_tx, chain_rx) = tokio::sync::watch::channel(None);
            let (events_tx, mut events_rx) = tokio::sync::broadcast::channel(16);
            let coordinator = ChainStateCoordinator::new(
                ChainTransport::Rpc {
                    url: "ws://unused.invalid".to_string(),
                },
                account,
                chain_state.clone(),
                chain_tx,
                events_tx,
            );

            // The finalized stream serves exactly one block then ends, so
            // `follow` bootstraps, processes the block (decoding the
            // ProviderRegistered event and refreshing state), and returns.
            coordinator
                .follow(ChainHandle { api })
                .await
                .expect("follow runs to stream end");

            // 4242 comes from the mocked runtime API, NOT the header number
            // (42) — proving the anchor is sourced from the runtime API rather
            // than the parachain height.
            assert_eq!(
                chain_state.current_anchor_block.load(Ordering::Relaxed),
                4242
            );
            let info = chain_state.provider_info.read();
            let info = info.as_ref().expect("provider info synced from chain");
            assert_eq!(info.stake, 1_000);
            assert!(chain_state.constants.read().is_some());
            assert!(chain_state.nonce_counter.read().is_some());

            // The connection was published and the block fanned out, including
            // the statically-decoded ChallengeCreated from the block's events.
            assert!(chain_rx.borrow().is_some());
            use provider_chain::chain_events::BlockEvent;
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
                    } if *provider == provider_account() => saw_challenge = true,
                    _ => {}
                }
            }
            assert!(saw_resubscribed, "follow should broadcast Resubscribed");
            assert!(
                saw_challenge,
                "follow should statically decode and broadcast ChallengeCreated"
            );
        }

        /// `NonceStore` that only records whether [`NonceStore::reset`] fired.
        #[derive(Default)]
        struct ResetSpy(AtomicBool);

        impl NonceStore for ResetSpy {
            fn load(&self) -> Option<u64> {
                None
            }
            fn persist(&self, _value: u64) {}
            fn reset(&self) {
                self.0.store(true, Ordering::SeqCst);
            }
        }

        /// An on-chain `ProviderDeregistered` must reach `nonce_store.reset()`.
        ///
        /// The integration suite covers the same gate, but starting from an
        /// already-built [`ProviderLifecycleEvent`]. This is the only test that
        /// runs the whole path — SCALE-encoded runtime event → static decode →
        /// `Deregistered` → watermark cleared — so it is what proves the
        /// `ProviderDeregistered` arm is wired to the right variant.
        #[tokio::test]
        async fn deregistration_event_clears_the_nonce_watermark() {
            let md = metadata();
            let account = provider_account();

            let deregistered = event_record(Value::named_variant(
                "ProviderDeregistered",
                [
                    (
                        "provider",
                        Value::from_bytes(<AccountId32 as AsRef<[u8]>>::as_ref(&account)),
                    ),
                    ("stake_returned", Value::u128(1_000)),
                ],
            ));
            let events_ty = storage_value_type(&md, "System", "Events");
            let events_bytes =
                encode_value(&md, events_ty, &Value::unnamed_composite([deregistered]));

            let providers_ty = storage_value_type(&md, PALLET_NAME, "Providers");
            let provider_bytes =
                encode_value(&md, providers_ty, &runtime_provider_info_value(None, None));

            let api = mock_api(vec![
                (
                    key_prefix("System", "Events"),
                    format!("0x{}", hex::encode(events_bytes)),
                ),
                (
                    key_prefix(PALLET_NAME, "Providers"),
                    format!("0x{}", hex::encode(provider_bytes)),
                ),
            ])
            .await;

            let store = Arc::new(ResetSpy::default());
            let chain_state = Arc::new(ChainState::with_nonce_store(store.clone()));
            let coordinator = ChainStateCoordinator::new(
                ChainTransport::Rpc {
                    url: "ws://unused.invalid".to_string(),
                },
                account,
                chain_state,
                tokio::sync::watch::channel(None).0,
                tokio::sync::broadcast::channel(16).0,
            );

            coordinator
                .follow(ChainHandle { api })
                .await
                .expect("follow runs to stream end");

            assert!(
                store.0.load(Ordering::SeqCst),
                "a confirmed ProviderDeregistered must reset the persisted nonce watermark"
            );
        }
    }
}
