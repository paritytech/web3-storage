// SPDX-License-Identifier: GPL-3.0-only

//! Chain-state coordinator: keeps the provider node's view of the runtime in
//! sync via a finalized-block subscription.
//!
//! [`ChainState`] is the single source of truth for all on-chain state the
//! provider node needs at runtime:
//! - [`ChainState::current_block`] — relay-chain block anchored to the latest
//!   finalized parachain block (the clock all on-chain durations use).
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

use crate::negotiate::NonceCounter;
use crate::storage::{NonceStore, NullNonceStore};
use crate::types::ProviderInfo;
use crate::Error;
use async_trait::async_trait;
use parking_lot::RwLock;
use sp_runtime::AccountId32;
use std::sync::atomic::AtomicU32;
use std::sync::Arc;
use std::time::Duration;
use subxt::ext::scale_value::{At, Composite, Primitive, Value, ValueDef, Variant};
use subxt::{OnlineClient, PolkadotConfig};
use tokio::task::JoinHandle;

/// Pallet whose storage, constants, and events the coordinator follows.
const PALLET_NAME: &str = "StorageProvider";

// ── ChainState ────────────────────────────────────────────────────────────────

/// Live chain state kept in sync with the runtime by the chain-state coordinator.
///
/// Held behind `Arc` inside [`crate::ProviderState`] so the coordinator can hold
/// its own handle without a back-reference to the whole node state.
pub struct ChainState {
    /// Relay-chain block anchored to the latest finalized parachain block —
    /// the clock all on-chain durations (timeouts, `valid_until`, nonce age)
    /// are measured against. `0` means not yet known.
    pub current_block: AtomicU32,
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
        Self {
            current_block: AtomicU32::new(0),
            constants: RwLock::new(None),
            provider_info: RwLock::new(None),
            nonce_counter: RwLock::new(None),
            nonce_store: Arc::new(NullNonceStore),
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
    async fn fetch_value(
        &self,
        entry: &str,
        who: &AccountId32,
    ) -> Result<Option<Value<u32>>, Error> {
        let addr = subxt::dynamic::storage(
            PALLET_NAME,
            entry,
            vec![Value::from_bytes(who.as_ref() as &[u8])],
        );
        let Some(thunk) = self
            .api
            .storage()
            .at_latest()
            .await
            .map_err(|e| Error::Internal(format!("Failed to get storage: {e}")))?
            .fetch(&addr)
            .await
            .map_err(|e| Error::Internal(format!("Failed to fetch {entry}: {e}")))?
        else {
            return Ok(None);
        };
        thunk
            .to_value()
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
        let value = self
            .api
            .constants()
            .at(&subxt::dynamic::constant(PALLET_NAME, "RequestTimeout"))
            .map_err(|e| Error::Internal(format!("Failed to read RequestTimeout: {e}")))?
            .to_value()
            .map_err(|e| Error::Internal(format!("Failed to decode RequestTimeout: {e}")))?;

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
            let deregistered = match event.variant_name() {
                "ProviderDeregistered" => true,
                "ProviderRegistered"
                | "ProviderSettingsUpdated"
                | "ProviderMultiaddrUpdated"
                | "DeregisterAnnounced"
                | "DeregisterCancelled" => false,
                _ => return None,
            };
            let fields = event.field_values().ok()?;
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
    chain_ws_url: String,
    provider_account: AccountId32,
    chain_state: Arc<ChainState>,
}

impl ChainStateCoordinator {
    pub fn new(
        chain_ws_url: String,
        provider_account: AccountId32,
        chain_state: Arc<ChainState>,
    ) -> Self {
        Self {
            chain_ws_url,
            provider_account,
            chain_state,
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
    /// stream until it ends. Returns `Err` if connecting fails; `Ok(())` if the
    /// stream terminates cleanly — either way the caller reconnects.
    async fn connect_and_follow(&self) -> Result<(), Error> {
        let api = OnlineClient::<PolkadotConfig>::from_url(&self.chain_ws_url)
            .await
            .map_err(|e| Error::Internal(format!("Failed to connect to chain: {e}")))?;
        let mut blocks = api
            .blocks()
            .subscribe_finalized()
            .await
            .map_err(|e| Error::Internal(format!("Failed to subscribe to blocks: {e}")))?;
        let chain = RealChainStateClient { api };

        tracing::info!("chain-state coordinator: connected; following finalized blocks");

        // Fetch pallet constants once per connection (they only change on runtime upgrade).
        sync_constants(&chain, &self.chain_state).await;

        // Bootstrap from any existing on-chain state so a restarted node that was
        // already registered picks up its provider_info and nonce counter immediately
        // rather than waiting for the next relevant event.
        refresh_provider_state(&chain, &self.chain_state, &self.provider_account).await;

        while let Some(next) = blocks.next().await {
            let block = match next {
                Ok(block) => block,
                Err(e) => {
                    tracing::warn!("chain-state coordinator: block subscription error: {e}");
                    break;
                }
            };
            let block_number = block.number();

            tracing::debug!("Finalized block: {}", block_number);
            // All on-chain durations (RequestTimeout, MaxNonceAge, expiries)
            // are denominated in relay-chain blocks, so `current_block` must
            // track the relay block anchored to this finalized block — not
            // its parachain height.
            match storage_client::substrate::fetch_last_relay_block_number(&block.storage()).await {
                Ok(relay_block) => {
                    self.chain_state
                        .current_block
                        .store(relay_block, std::sync::atomic::Ordering::Relaxed);
                }
                Err(e) => tracing::warn!(
                    "chain-state coordinator: failed to fetch relay block for block \
                     {block_number}: {e}; keeping previous value"
                ),
            }

            let parsed = match block.events().await {
                Ok(events) => parse_provider_lifecycle_events(&events),
                Err(e) => {
                    tracing::warn!(
                        "chain-state coordinator: failed to fetch events for block {block_number}: {e}"
                    );
                    continue;
                }
            };

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
fn decode_provider_info(value: &Value<u32>) -> Result<ProviderInfo, Error> {
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
fn named_field<'a>(value: &'a Value<u32>, field: &str) -> Option<&'a Value<u32>> {
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
fn decode_byte_vec(value: &Value<u32>) -> Vec<u8> {
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
fn decode_account(v: &Value<u32>) -> Option<AccountId32> {
    let mut bytes = [0u8; 32];
    if collect_bytes(v, &mut bytes, 0) == 32 {
        Some(AccountId32::new(bytes))
    } else {
        None
    }
}

/// Recursively collect raw bytes from a SCALE value into `buf` starting at
/// `offset`, returning the new offset.
fn collect_bytes(v: &Value<u32>, buf: &mut [u8; 32], offset: usize) -> usize {
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
    use std::sync::atomic::Ordering;

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
        assert_eq!(cs.current_block.load(Ordering::Relaxed), 0);
        assert!(cs.constants.read().is_none());
        assert!(cs.provider_info.read().is_none());
        assert!(cs.nonce_counter.read().is_none());
    }

    #[test]
    fn chain_state_current_block_round_trips() {
        let cs = ChainState::default();
        cs.current_block.store(42, Ordering::Relaxed);
        assert_eq!(cs.current_block.load(Ordering::Relaxed), 42);
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

    // ── dynamic-value decoders ────────────────────────────────────────────

    /// Build a `Providers`-storage-shaped value the way subxt surfaces it:
    /// a named composite with nested `settings`/`stats` composites, `Option`
    /// fields as `Some`/`None` variants, and the `multiaddr` `BoundedVec<u8>`
    /// wrapped in the single-field unnamed composite scale_value produces.
    fn provider_info_value(
        replica_sync_price: Option<u128>,
        deregister_at: Option<u32>,
    ) -> Value<u32> {
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
        .map_context(|_| 0u32)
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
        let value = Value::named_composite([("multiaddr", Value::from_bytes("/ip4/1.2.3.4"))])
            .map_context(|_| 0u32);
        let err = decode_provider_info(&value).unwrap_err();
        assert!(
            matches!(&err, Error::Internal(msg) if msg.contains("stake")),
            "unexpected error: {err:?}"
        );
    }

    #[test]
    fn named_field_finds_and_misses() {
        let value = Value::named_composite([("present", Value::u128(1))]).map_context(|_| 0u32);
        assert!(named_field(&value, "present").is_some());
        assert!(named_field(&value, "absent").is_none());
        // Not a named composite → always None.
        let prim = Value::u128(9).map_context(|_| 0u32);
        assert!(named_field(&prim, "present").is_none());
    }

    #[test]
    fn decode_byte_vec_handles_direct_and_wrapped_and_other() {
        // Direct byte sequence (e.g. `Vec<u8>`).
        let direct = Value::from_bytes(b"hello").map_context(|_| 0u32);
        assert_eq!(decode_byte_vec(&direct), b"hello");
        // Single-field unnamed wrapper (e.g. `BoundedVec<u8, _>`).
        let wrapped = Value::unnamed_composite([Value::from_bytes(b"hi")]).map_context(|_| 0u32);
        assert_eq!(decode_byte_vec(&wrapped), b"hi");
        // Non-composite → empty.
        let prim = Value::u128(5).map_context(|_| 0u32);
        assert!(decode_byte_vec(&prim).is_empty());
    }

    #[test]
    fn decode_account_from_flat_and_nested_bytes() {
        // Flat 32-byte sequence.
        let flat = Value::from_bytes([7u8; 32]).map_context(|_| 0u32);
        assert_eq!(decode_account(&flat), Some(AccountId32::new([7u8; 32])));
        // `[u8; 32]` newtype nests the sequence one level deeper.
        let nested = Value::unnamed_composite([Value::from_bytes([9u8; 32])]).map_context(|_| 0u32);
        assert_eq!(decode_account(&nested), Some(AccountId32::new([9u8; 32])));
    }

    #[test]
    fn decode_account_rejects_wrong_length() {
        let short = Value::from_bytes([0u8; 31]).map_context(|_| 0u32);
        assert_eq!(decode_account(&short), None);
        let long = Value::from_bytes([0u8; 33]).map_context(|_| 0u32);
        assert_eq!(decode_account(&long), None);
    }
}
