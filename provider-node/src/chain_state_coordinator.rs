//! Chain-state coordinator: keeps the provider node's view of the runtime in
//! sync via a finalized-block subscription.
//!
//! [`ChainState`] is the live-synced slice of [`crate::ProviderState`]:
//! - [`ChainState::current_block`] is written on every finalized block.
//! - [`ChainState::provider_info`] is re-fetched whenever the provider's
//!   on-chain settings or multiaddr change.
//!
//! [`ChainStateCoordinator`] drives a [`BlockSubscriberStream`] in a single
//! async loop and is meant to run unconditionally whenever a chain RPC is
//! reachable.

use parking_lot::RwLock;
use sp_core::H256;
use sp_runtime::AccountId32;
use std::sync::atomic::AtomicU32;
use std::sync::Arc;
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
    /// Latest finalized block height. `0` means not yet known — the coordinator
    /// writes the real value once it first connects.
    pub current_block: AtomicU32,
    /// Provider's on-chain registration info. `None` until first fetch; updated
    /// whenever a settings or multiaddr-change event lands.
    pub provider_info: RwLock<Option<ProviderInfo>>,
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

    /// Connect to the chain and start the coordinator.
    ///
    /// Returns `Err` on a connection-level failure — callers should log and
    /// continue rather than aborting the server.
    pub async fn start(self) -> Result<ChainStateCoordinatorHandle, ClientError> {
        let mut stream = BlockSubscriberStream::connect(&self.chain_ws_url).await?;

        // A client for re-fetching full provider info on registration.
        let mut client = ProviderClient::new(
            ClientConfig {
                chain_ws_url: self.chain_ws_url.clone(),
                ..Default::default()
            },
            self.provider_account.to_string(),
        )?;
        client.connect().await?;

        let task = tokio::spawn(async move {
            while let Some(block) = stream.next().await {
                let block_hash = H256::from_slice(block.hash().as_ref());
                let block_number = block.number();

                // handle update ChainState in a single call
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

                self.process_provider_info_update(&client, &events, block_hash, block_number)
                    .await;
            }
        });

        Ok(ChainStateCoordinatorHandle { task })
    }

    /// Apply this provider's settings
    async fn process_provider_info_update(
        &self,
        client: &ProviderClient,
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

        for event in parsed {
            match event {
                StorageEvent::ProviderSettingsUpdated {
                    provider,
                    provider_settings,
                    ..
                } if provider == self.provider_account => {
                    if let Some(info) = self.chain_state.provider_info.write().as_mut() {
                        info.price_per_byte = provider_settings.price_per_byte;
                        info.min_duration = provider_settings.min_duration;
                        info.max_duration = provider_settings.max_duration;
                        info.accepting_primary = provider_settings.accepting_primary;
                        info.replica_sync_price = provider_settings.replica_sync_price;
                        info.accepting_extensions = provider_settings.accepting_extensions;
                        info.max_capacity = provider_settings.max_capacity;
                    }
                }
                StorageEvent::ProviderMultiaddrUpdated {
                    provider,
                    multiaddr,
                    ..
                } if provider == self.provider_account => {
                    if let Some(info) = self.chain_state.provider_info.write().as_mut() {
                        info.multiaddr = multiaddr;
                    }
                }
                StorageEvent::ProviderRegistered { provider, .. }
                    if provider == self.provider_account =>
                {
                    match client.get_provider_info(&self.provider_account).await {
                        Ok(info) => *self.chain_state.provider_info.write() = info,
                        Err(e) => tracing::warn!(
                            "chain-state coordinator: failed to fetch provider_info after registration: {e}"
                        ),
                    }
                }
                _ => {}
            }
        }
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
        self.task.abort();
        let _ = self.task.await;
    }
}

/// Filter a block's events down to a single pallet and parse them with `P`.
///
/// ```ignore
/// let pallet_storage_provider_events = parse_pallet_events::<StorageEvent, StorageProviderEventParser>(&events, storage_client::substrate::PALLET_NAME, hash, num);
/// let pallet_s3_registry_events = parse_pallet_events::<S3Event, S3EventParser>(&events, s3_client::substrate::PALLET_NAME, hash, num);
/// ```
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
        }
    }

    #[test]
    fn chain_state_defaults_to_unknown() {
        let cs = ChainState::default();
        assert_eq!(cs.current_block.load(Ordering::Relaxed), 0);
        assert!(cs.provider_info.read().is_none());
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
}
