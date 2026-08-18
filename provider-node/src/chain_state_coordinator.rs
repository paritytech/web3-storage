// SPDX-License-Identifier: GPL-3.0-only

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
//!
//! It also broadcasts bucket-membership changes as
//! [`BlockEvent::BucketMembershipChanged`], so the membership cache can drop
//! stale authorization on its own rather than being told to.

use crate::chain_connection::{self, ChainHandle, ChainTransport};
use crate::chain_events::{self, BlockEvent, BlockEventTx};
use crate::negotiate::NonceCounter;
use crate::types::ProviderInfo;
use crate::Error;
use async_trait::async_trait;
use parking_lot::RwLock;
use provider_storage::NonceStore;
use sp_runtime::AccountId32;
use std::sync::atomic::AtomicU32;
use std::sync::Arc;
use std::time::Duration;
use subxt::ext::scale_value::{At, Composite, Primitive, Value, ValueDef, Variant};
use subxt::{OnlineClient, PolkadotConfig};
use tokio::sync::watch;
use tokio::task::JoinHandle;

/// Pallet whose storage, constants, and events the coordinator follows.
const PALLET_NAME: &str = "StorageProvider";

// ── ChainState ────────────────────────────────────────────────────────────────

/// Live chain state kept in sync with the runtime by the chain-state coordinator.
///
/// Held behind `Arc` inside [`crate::ProviderState`] so the coordinator can hold
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

/// Production [`ChainStateChainClient`] running dynamic storage queries on the
/// coordinator's own subxt connection (shared with the block subscription).
struct RealChainStateClient {
    api: OnlineClient<PolkadotConfig>,
}

impl RealChainStateClient {
    async fn fetch_value(&self, entry: &str, who: &AccountId32) -> Result<Option<Value>, Error> {
        let addr = subxt::dynamic::storage::<(Value,), Value>(PALLET_NAME, entry);
        let at = self
            .api
            .at_current_block()
            .await
            .map_err(|e| Error::Internal(format!("Failed to get storage: {e}")))?;
        let Some(value) = at
            .storage()
            .try_fetch(addr, (Value::from_bytes(who.as_ref() as &[u8]),))
            .await
            .map_err(|e| Error::Internal(format!("Failed to fetch {entry}: {e}")))?
        else {
            return Ok(None);
        };
        value
            .decode()
            .map(Some)
            .map_err(|e| Error::Internal(format!("Failed to decode {entry}: {e}")))
    }
}

#[async_trait]
impl ChainStateChainClient for RealChainStateClient {
    async fn get_provider_info(&self, who: &AccountId32) -> Result<Option<ProviderInfo>, Error> {
        match self.fetch_value("Providers", who).await? {
            Some(value) => decode_provider_info(&value).map(Some),
            None => Ok(None),
        }
    }

    async fn fetch_replay_hsn(&self, who: &AccountId32) -> Result<Option<u64>, Error> {
        Ok(self
            .fetch_value("ProviderReplayStates", who)
            .await?
            .as_ref()
            .and_then(|value| named_field(value, "hsn"))
            .and_then(|v| v.as_u128())
            .map(|h| h as u64))
    }

    async fn fetch_request_timeout(&self) -> Result<Option<u32>, Error> {
        let value: Value = self
            .api
            .at_current_block()
            .await
            .map_err(|e| Error::Internal(format!("Failed to get current block: {e}")))?
            .constants()
            .entry(subxt::dynamic::constant::<Value>(
                PALLET_NAME,
                "RequestTimeout",
            ))
            .map_err(|e| Error::Internal(format!("Failed to read RequestTimeout: {e}")))?;

        Ok(value.as_u128().map(|v| v as u32))
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
    events
        .iter()
        .filter_map(|event| event.ok())
        .filter(|event| event.pallet_name() == PALLET_NAME)
        .filter_map(|event| {
            let deregistered = match event.event_name() {
                "ProviderDeregistered" => true,
                "ProviderRegistered"
                | "ProviderSettingsUpdated"
                | "ProviderMultiaddrUpdated"
                | "DeregisterAnnounced"
                | "DeregisterCancelled" => false,
                _ => return None,
            };
            let fields = event.decode_fields_unchecked_as::<Value>().ok()?;
            let provider = decode_account(fields.at("provider")?)?;
            Some(if deregistered {
                ProviderLifecycleEvent::Deregistered { provider }
            } else {
                ProviderLifecycleEvent::Updated { provider }
            })
        })
        .collect()
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
            match crate::subxt_client::fetch_current_anchor_block(&at).await {
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

// ── dynamic-value decoding ────────────────────────────────────────────────────

/// Decode a `StorageProvider::Providers` storage value into [`ProviderInfo`].
fn decode_provider_info(value: &Value) -> Result<ProviderInfo, Error> {
    let missing = |field: &str| Error::Internal(format!("Missing '{field}' in ProviderInfo"));

    let multiaddr = named_field(value, "multiaddr")
        .map(|v| String::from_utf8_lossy(&decode_byte_vec(v)).into_owned())
        .unwrap_or_default();

    let stake = named_field(value, "stake")
        .and_then(|v| v.as_u128())
        .ok_or_else(|| missing("stake"))?;

    let committed_bytes = named_field(value, "committed_bytes")
        .and_then(|v| v.as_u128())
        .ok_or_else(|| missing("committed_bytes"))? as u64;

    let settings = named_field(value, "settings").ok_or_else(|| missing("settings"))?;

    let replica_sync_price =
        named_field(settings, "replica_sync_price").and_then(|v| match &v.value {
            ValueDef::Variant(Variant { name, values }) if name == "Some" => {
                values.values().next().and_then(|v| v.as_u128())
            }
            _ => None,
        });

    let stats = named_field(value, "stats");
    let agreements_total = stats
        .and_then(|s| named_field(s, "agreements_total"))
        .and_then(|v| v.as_u128())
        .unwrap_or(0) as u32;
    let challenges_failed = stats
        .and_then(|s| named_field(s, "challenges_failed"))
        .and_then(|v| v.as_u128())
        .unwrap_or(0) as u32;

    Ok(ProviderInfo {
        multiaddr,
        stake,
        committed_bytes,
        max_capacity: named_field(settings, "max_capacity")
            .and_then(|v| v.as_u128())
            .ok_or_else(|| missing("max_capacity"))? as u64,
        min_duration: named_field(settings, "min_duration")
            .and_then(|v| v.as_u128())
            .ok_or_else(|| missing("min_duration"))? as u32,
        max_duration: named_field(settings, "max_duration")
            .and_then(|v| v.as_u128())
            .ok_or_else(|| missing("max_duration"))? as u32,
        price_per_byte: named_field(settings, "price_per_byte")
            .and_then(|v| v.as_u128())
            .ok_or_else(|| missing("price_per_byte"))?,
        accepting_primary: named_field(settings, "accepting_primary")
            .and_then(|v| v.as_bool())
            .ok_or_else(|| missing("accepting_primary"))?,
        replica_sync_price,
        accepting_extensions: named_field(settings, "accepting_extensions")
            .and_then(|v| v.as_bool())
            .ok_or_else(|| missing("accepting_extensions"))?,
        agreements_total,
        challenges_failed,
        deregister_at: named_field(value, "deregister_at").and_then(|v| match &v.value {
            ValueDef::Variant(Variant { name, values }) if name == "Some" => values
                .values()
                .next()
                .and_then(|v| v.as_u128())
                .map(|n| n as u32),
            _ => None,
        }),
    })
}

/// Look up a named field in a scale_value composite.
fn named_field<'a>(value: &'a Value, field: &str) -> Option<&'a Value> {
    match &value.value {
        ValueDef::Composite(Composite::Named(fields)) => {
            fields.iter().find(|(n, _)| n == field).map(|(_, v)| v)
        }
        _ => None,
    }
}

/// Decode a `Vec<u8>` / `BoundedVec<u8, _>` from a scale_value composite.
///
/// `BoundedVec<T, N>` serializes its `TypeInfo` as a 1-field unnamed composite
/// wrapping the inner `Vec<T>`, so scale_value surfaces it as
/// `Composite::Unnamed([inner_vec])`. This helper drills through that wrapper
/// if present, then collects the bytes.
fn decode_byte_vec(value: &Value) -> Vec<u8> {
    let ValueDef::Composite(Composite::Unnamed(items)) = &value.value else {
        return Vec::new();
    };
    // Direct sequence of byte primitives.
    let bytes: Vec<u8> = items
        .iter()
        .filter_map(|b| b.as_u128().map(|n| n as u8))
        .collect();
    if !items.is_empty() && bytes.len() == items.len() {
        return bytes;
    }
    // BoundedVec wrapper: single inner field holds the actual sequence.
    if items.len() == 1 {
        return decode_byte_vec(&items[0]);
    }
    Vec::new()
}

/// Decode an [`AccountId32`] from a SCALE value (a possibly-nested composite of
/// 32 byte primitives).
pub(crate) fn decode_account(v: &Value) -> Option<AccountId32> {
    let mut bytes = [0u8; 32];
    if collect_bytes(v, &mut bytes, 0) == 32 {
        Some(AccountId32::new(bytes))
    } else {
        None
    }
}

/// Recursively collect raw bytes from a SCALE value into `buf` starting at
/// `offset`, returning the new offset.
fn collect_bytes(v: &Value, buf: &mut [u8; 32], offset: usize) -> usize {
    match &v.value {
        ValueDef::Primitive(Primitive::U128(n)) => {
            // Keep counting past the buffer so oversized inputs fail the
            // exact-length check in `decode_account`.
            if offset < 32 {
                buf[offset] = *n as u8;
            }
            offset + 1
        }
        ValueDef::Composite(Composite::Unnamed(items)) => {
            let mut pos = offset;
            for item in items {
                pos = collect_bytes(item, buf, pos);
            }
            pos
        }
        _ => offset,
    }
}

// ── tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use provider_storage::temp_rocksdb;
    use std::sync::atomic::Ordering;

    /// Chain state over a throwaway backend's nonce store.
    fn test_chain_state() -> (ChainState, tempfile::TempDir) {
        let (_storage, nonce_store, dir) = temp_rocksdb();
        (ChainState::with_nonce_store(nonce_store), dir)
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
        assert_eq!(info.price_per_byte, 5);
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

    // ── dynamic-value decoders ────────────────────────────────────────────

    /// Build a `Providers`-storage-shaped value the way subxt surfaces it:
    /// a named composite with nested `settings`/`stats` composites, `Option`
    /// fields as `Some`/`None` variants, and the `multiaddr` `BoundedVec<u8>`
    /// wrapped in the single-field unnamed composite scale_value produces.
    fn provider_info_value(replica_sync_price: Option<u128>, deregister_at: Option<u32>) -> Value {
        let opt = |val: Option<u128>| match val {
            Some(v) => Value::unnamed_variant("Some", vec![Value::u128(v)]),
            None => Value::unnamed_variant("None", Vec::<Value<()>>::new()),
        };
        let settings = Value::named_composite([
            ("max_capacity", Value::u128(10_000)),
            ("min_duration", Value::u128(10)),
            ("max_duration", Value::u128(100)),
            ("price_per_byte", Value::u128(5)),
            ("accepting_primary", Value::bool(true)),
            ("accepting_extensions", Value::bool(true)),
            ("replica_sync_price", opt(replica_sync_price)),
        ]);
        let stats = Value::named_composite([
            ("agreements_total", Value::u128(3)),
            ("challenges_failed", Value::u128(1)),
        ]);
        // `BoundedVec<u8>` surfaces as a 1-field unnamed composite wrapping
        // the byte sequence.
        let multiaddr = Value::unnamed_composite([Value::from_bytes("/ip4/1.2.3.4/tcp/3333")]);
        Value::named_composite([
            ("multiaddr", multiaddr),
            ("stake", Value::u128(1_000)),
            ("committed_bytes", Value::u128(500)),
            ("settings", settings),
            ("stats", stats),
            ("deregister_at", opt(deregister_at.map(u128::from))),
        ])
    }

    #[test]
    fn decode_provider_info_full() {
        let info = decode_provider_info(&provider_info_value(Some(7), Some(42))).unwrap();
        assert_eq!(info.multiaddr, "/ip4/1.2.3.4/tcp/3333");
        assert_eq!(info.stake, 1_000);
        assert_eq!(info.committed_bytes, 500);
        assert_eq!(info.max_capacity, 10_000);
        assert_eq!(info.min_duration, 10);
        assert_eq!(info.max_duration, 100);
        assert_eq!(info.price_per_byte, 5);
        assert!(info.accepting_primary);
        assert!(info.accepting_extensions);
        assert_eq!(info.replica_sync_price, Some(7));
        assert_eq!(info.agreements_total, 3);
        assert_eq!(info.challenges_failed, 1);
        assert_eq!(info.deregister_at, Some(42));
    }

    #[test]
    fn decode_provider_info_none_options() {
        let info = decode_provider_info(&provider_info_value(None, None)).unwrap();
        assert_eq!(info.replica_sync_price, None);
        assert_eq!(info.deregister_at, None);
    }

    #[test]
    fn decode_provider_info_missing_required_field_errors() {
        // Everything present except the required `stake` field.
        let value = Value::named_composite([("multiaddr", Value::from_bytes("/ip4/1.2.3.4"))]);
        let err = decode_provider_info(&value).unwrap_err();
        assert!(
            matches!(&err, Error::Internal(msg) if msg.contains("stake")),
            "unexpected error: {err:?}"
        );
    }

    #[test]
    fn named_field_finds_and_misses() {
        let value = Value::named_composite([("present", Value::u128(1))]);
        assert!(named_field(&value, "present").is_some());
        assert!(named_field(&value, "absent").is_none());
        // Not a named composite → always None.
        let prim = Value::u128(9);
        assert!(named_field(&prim, "present").is_none());
    }

    #[test]
    fn decode_byte_vec_handles_direct_and_wrapped_and_other() {
        // Direct byte sequence (e.g. `Vec<u8>`).
        let direct = Value::from_bytes(b"hello");
        assert_eq!(decode_byte_vec(&direct), b"hello");
        // Single-field unnamed wrapper (e.g. `BoundedVec<u8, _>`).
        let wrapped = Value::unnamed_composite([Value::from_bytes(b"hi")]);
        assert_eq!(decode_byte_vec(&wrapped), b"hi");
        // Non-composite → empty.
        let prim = Value::u128(5);
        assert!(decode_byte_vec(&prim).is_empty());
    }

    #[test]
    fn decode_account_from_flat_and_nested_bytes() {
        // Flat 32-byte sequence.
        let flat = Value::from_bytes([7u8; 32]);
        assert_eq!(decode_account(&flat), Some(AccountId32::new([7u8; 32])));
        // `[u8; 32]` newtype nests the sequence one level deeper.
        let nested = Value::unnamed_composite([Value::from_bytes([9u8; 32])]);
        assert_eq!(decode_account(&nested), Some(AccountId32::new([9u8; 32])));
    }

    #[test]
    fn decode_account_rejects_wrong_length() {
        let short = Value::from_bytes([0u8; 31]);
        assert_eq!(decode_account(&short), None);
        let long = Value::from_bytes([0u8; 33]);
        assert_eq!(decode_account(&long), None);
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
            "/../packages/papi/.papi/metadata/parachain.scale"
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
        /// shape — unlike [`provider_info_value`], every runtime field must be
        /// present for `scale_value` to encode it against the real type.
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

        /// `System::Events` bytes holding one of each membership-changing event
        /// (buckets 7, 7, 8, and 9) plus a `ProviderRegistered` that carries no
        /// bucket at all, encoded against the real runtime types.
        fn encoded_membership_events(md: &subxt::Metadata, provider: &AccountId32) -> Vec<u8> {
            let member = Value::from_bytes([3u8; 32]);
            let bucket_created = event_record(Value::named_variant(
                "BucketCreated",
                [
                    ("bucket_id", Value::u128(9)),
                    ("admin", Value::from_bytes([4u8; 32])),
                ],
            ));
            let member_set = event_record(Value::named_variant(
                "MemberSet",
                [
                    ("bucket_id", Value::u128(7)),
                    ("member", member.clone()),
                    (
                        "role",
                        Value::unnamed_variant("Writer", Vec::<Value>::new()),
                    ),
                ],
            ));
            let member_removed = event_record(Value::named_variant(
                "MemberRemoved",
                [("bucket_id", Value::u128(7)), ("member", member)],
            ));
            let bucket_deleted = event_record(Value::named_variant(
                "BucketDeleted",
                [("bucket_id", Value::u128(8))],
            ));
            let registered = event_record(Value::named_variant(
                "ProviderRegistered",
                [
                    (
                        "provider",
                        Value::from_bytes(<AccountId32 as AsRef<[u8]>>::as_ref(provider)),
                    ),
                    ("stake", Value::u128(1_000)),
                ],
            ));
            let ty = storage_value_type(md, "System", "Events");
            encode_value(
                md,
                ty,
                &Value::unnamed_composite([
                    bucket_created,
                    member_set,
                    member_removed,
                    bucket_deleted,
                    registered,
                ]),
            )
        }

        /// The `BucketMembershipChanged` bucket ids `chain_events::decode_block_events`
        /// produces from `events`, in encounter order.
        fn membership_changed_bucket_ids(
            events: &subxt::events::Events<PolkadotConfig>,
        ) -> Vec<u64> {
            chain_events::decode_block_events(events)
                .into_iter()
                .filter_map(|event| match event {
                    BlockEvent::BucketMembershipChanged { bucket_id } => Some(bucket_id),
                    _ => None,
                })
                .collect()
        }

        #[tokio::test]
        async fn membership_changes_decode_to_their_bucket_ids() {
            let md = metadata();
            let api = mock_api(vec![(
                key_prefix("System", "Events"),
                format!(
                    "0x{}",
                    hex::encode(encoded_membership_events(&md, &provider_account()))
                ),
            )])
            .await;

            let at = api.at_current_block().await.expect("block handle");
            let events = at.events().fetch().await.expect("events fetch");

            // Every membership-changing event contributes its bucket, duplicates
            // included (invalidation is idempotent); the provider-lifecycle event
            // carries no bucket and must be skipped.
            assert_eq!(membership_changed_bucket_ids(&events), vec![9, 7, 7, 8]);
        }

        #[tokio::test]
        async fn blocks_without_membership_changes_decode_to_nothing() {
            let md = metadata();
            let api = mock_api(vec![(
                key_prefix("System", "Events"),
                format!(
                    "0x{}",
                    hex::encode(encoded_events(&md, &provider_account()))
                ),
            )])
            .await;

            let at = api.at_current_block().await.expect("block handle");
            let events = at.events().fetch().await.expect("events fetch");

            assert!(membership_changed_bucket_ids(&events).is_empty());
        }

        #[tokio::test]
        async fn follow_broadcasts_membership_changes() {
            let md = metadata();
            let account = provider_account();

            let providers_ty = storage_value_type(&md, PALLET_NAME, "Providers");
            let provider_bytes =
                encode_value(&md, providers_ty, &runtime_provider_info_value(None, None));
            let events_bytes = encoded_membership_events(&md, &account);

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

            let (chain_state, _dir) = test_chain_state();
            let chain_state = Arc::new(chain_state);
            let (chain_tx, _chain_rx) = tokio::sync::watch::channel(None);
            let (events_tx, mut events_rx) = tokio::sync::broadcast::channel(16);
            let coordinator = ChainStateCoordinator::new(
                ChainTransport::Rpc {
                    url: "ws://unused.invalid".to_string(),
                },
                account,
                chain_state,
                chain_tx,
                events_tx,
            );

            coordinator
                .follow(ChainHandle { api })
                .await
                .expect("follow runs to stream end");

            use crate::chain_events::BlockEvent;
            let mut changed_buckets = Vec::new();
            while let Ok(event) = events_rx.try_recv() {
                if let BlockEvent::BucketMembershipChanged { bucket_id } = event {
                    changed_buckets.push(bucket_id);
                }
            }
            // Matches `membership_changes_decode_to_their_bucket_ids`: duplicates
            // included (invalidation is idempotent), the provider-lifecycle event
            // in the same block contributes nothing.
            assert_eq!(changed_buckets, vec![9, 7, 7, 8]);
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

            let (state, _dir) = test_chain_state();
            let chain_state = Arc::new(state);
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
            use crate::chain_events::BlockEvent;
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
    }
}
