//! Chain-state coordinator: keeps the provider node's view of the runtime in
//! sync via a finalized-block subscription.
//!
//! [`ChainState`] is the single source of truth for all on-chain state the
//! provider node needs at runtime:
//! - [`ChainState::current_block`] — latest finalized block height.
//! - [`ChainState::constants`] — pallet constants fetched once on connect.
//! - [`ChainState::provider_info`] — full provider registration info.
//! - [`ChainState::nonce_counter`] — nonce counter bootstrapped from the
//!   chain's replay window. `None` until the provider is registered.
//!
//! [`ChainStateCoordinator`] is the **only writer** for all four fields.  It
//! drives a [`BlockSubscriberStream`] in a reconnect loop; on every relevant
//! provider event it re-fetches the full `ProviderInfo` so `committed_bytes`,
//! `stake`, and all settings stay current — no field-patching, no partial
//! updates, no second writer.

use crate::negotiate::NonceCounter;
use async_trait::async_trait;
use parking_lot::RwLock;
use sp_core::H256;
use sp_runtime::AccountId32;
use std::sync::atomic::AtomicU32;
use std::sync::Arc;
use std::time::Duration;
use storage_client::discovery::ProviderInfo;
use storage_client::{
    BlockSubscriberStream, ClientConfig, ClientError, EventParser, ProviderClient, StorageEvent,
    StorageProviderEventParser,
};
use subxt::ext::futures::StreamExt;
use tokio::task::JoinHandle;

// ── ChainState ────────────────────────────────────────────────────────────────

/// Live chain state kept in sync with the runtime by the chain-state coordinator.
///
/// Held behind `Arc` inside [`crate::ProviderState`] so the coordinator can hold
/// its own handle without a back-reference to the whole node state.
#[derive(Default)]
pub struct ChainState {
    /// Latest finalized block height. `0` means not yet known.
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
    async fn get_provider_info(
        &self,
        who: &AccountId32,
    ) -> Result<Option<ProviderInfo>, ClientError>;

    /// Provider's replay-window head sequence (`hsn`), or `None` if no replay
    /// state exists yet (the provider has never signed any terms).
    async fn fetch_replay_hsn(&self, who: &AccountId32) -> Result<Option<u64>, ClientError>;

    /// `StorageProvider::RequestTimeout` runtime constant, or `None` if absent
    /// from the node's metadata.
    async fn fetch_request_timeout(&self) -> Result<Option<u32>, ClientError>;
}

/// Production [`ChainStateChainClient`] backed by a connected [`ProviderClient`].
///
/// `get_provider_info` reuses the already-connected client; the two read-only
/// queries are associated functions that open their own short-lived connection,
/// so they only need the WS URL.
struct RealChainStateClient {
    client: ProviderClient,
    chain_ws_url: String,
}

#[async_trait]
impl ChainStateChainClient for RealChainStateClient {
    async fn get_provider_info(
        &self,
        who: &AccountId32,
    ) -> Result<Option<ProviderInfo>, ClientError> {
        self.client.get_provider_info(who).await
    }

    async fn fetch_replay_hsn(&self, who: &AccountId32) -> Result<Option<u64>, ClientError> {
        ProviderClient::fetch_replay_hsn(&self.chain_ws_url, who).await
    }

    async fn fetch_request_timeout(&self) -> Result<Option<u32>, ClientError> {
        ProviderClient::fetch_request_timeout(&self.chain_ws_url).await
    }
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
    async fn connect_and_follow(&self) -> Result<(), ClientError> {
        let mut stream = BlockSubscriberStream::connect(&self.chain_ws_url).await?;

        let mut client = ProviderClient::new(
            ClientConfig {
                chain_ws_url: self.chain_ws_url.clone(),
                ..Default::default()
            },
            self.provider_account.to_string(),
        )?;
        client.connect().await?;
        let chain = RealChainStateClient {
            client,
            chain_ws_url: self.chain_ws_url.clone(),
        };

        tracing::info!("chain-state coordinator: connected; following finalized blocks");

        // Fetch pallet constants once per connection (they only change on runtime upgrade).
        sync_constants(&chain, &self.chain_state).await;

        // Bootstrap from any existing on-chain state so a restarted node that was
        // already registered picks up its provider_info and nonce counter immediately
        // rather than waiting for the next relevant event.
        refresh_provider_state(&chain, &self.chain_state, &self.provider_account).await;

        while let Some(block) = stream.next().await {
            let block_hash = H256::from_slice(block.hash().as_ref());
            let block_number = block.number();

            tracing::debug!("Finalized block: {}", block_number);
            self.chain_state
                .current_block
                .store(block_number, std::sync::atomic::Ordering::Relaxed);

            let events = match block.events().await {
                Ok(events) => events,
                Err(e) => {
                    tracing::warn!(
                        "chain-state coordinator: failed to fetch events for block {block_number}: {e}"
                    );
                    continue;
                }
            };

            self.process_provider_events(&chain, &events, block_hash, block_number)
                .await;
        }

        Ok(())
    }

    /// Parse this block's pallet events and refresh state if any are relevant.
    async fn process_provider_events(
        &self,
        chain: &dyn ChainStateChainClient,
        events: &subxt::events::Events<subxt::PolkadotConfig>,
        block_hash: H256,
        block_number: u32,
    ) {
        let parsed = parse_pallet_events::<StorageEvent, StorageProviderEventParser>(
            events,
            storage_client::substrate::PALLET_NAME,
            block_hash,
            block_number,
        );

        refresh_if_relevant_event(
            chain,
            &self.chain_state,
            &self.provider_account,
            &parsed,
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

/// Re-fetch the full `ProviderInfo` and replay state from chain and update
/// `chain_state` atomically.
///
/// The nonce counter is bootstrapped *before* `provider_info` is published so
/// `/negotiate` never sees a populated info without a ready counter.
///
/// Called both on the initial connect (restart recovery) and on every relevant
/// provider event. Using a full re-fetch (rather than field-patching) keeps
/// `committed_bytes`, `stake`, and all settings consistent in one shot.
pub async fn refresh_provider_state(
    chain: &dyn ChainStateChainClient,
    chain_state: &ChainState,
    provider_account: &AccountId32,
) {
    match chain.get_provider_info(provider_account).await {
        Ok(Some(info)) => {
            match chain.fetch_replay_hsn(provider_account).await {
                Ok(hsn) => {
                    let counter = Arc::new(NonceCounter::new(1));
                    if let Some(hsn) = hsn {
                        tracing::info!("chain-state coordinator: provider state synced");
                        counter.bootstrap_from_hsn(hsn);
                    }
                    // Else:
                    // Registered but no replay state yet — registration inserts both
                    // atomically, so this is a transient view. The next event or block
                    // that triggers a refresh will resolve it.
                    *chain_state.nonce_counter.write() = Some(counter);
                    *chain_state.provider_info.write() = Some(info);
                }
                Err(e) => {
                    tracing::debug!("chain-state coordinator: failed to fetch replay hsn: {e}")
                }
            }
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
    events: &[StorageEvent],
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
}

/// Whether `event` is a provider lifecycle event for `provider_account` — i.e. one
/// that should trigger a [`refresh_provider_state`]. Settings, multiaddr, and the
/// (de)registration events all change state `/negotiate` depends on; everything
/// else (checkpoints, challenges, agreements, other providers) is ignored.
pub fn is_relevant_provider_event(event: &StorageEvent, provider_account: &AccountId32) -> bool {
    match event {
        StorageEvent::ProviderRegistered { provider, .. }
        | StorageEvent::ProviderSettingsUpdated { provider, .. }
        | StorageEvent::ProviderMultiaddrUpdated { provider, .. }
        | StorageEvent::DeregisterAnnounced { provider, .. }
        | StorageEvent::ProviderDeregistered { provider, .. }
        | StorageEvent::DeregisterCancelled { provider, .. } => provider == provider_account,
        _ => false,
    }
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

/// Filter a block's events down to a single pallet and parse them with `P`.
fn parse_pallet_events<E, P: EventParser<E>>(
    events: &subxt::events::Events<subxt::PolkadotConfig>,
    pallet_name: &str,
    block_hash: H256,
    block_number: u32,
) -> Vec<E> {
    events
        .iter()
        .filter_map(|event| event.ok())
        .filter(|event| event.pallet_name() == pallet_name)
        .filter_map(|event| P::parse_event_detail(&event, block_hash, block_number))
        .collect()
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
}
