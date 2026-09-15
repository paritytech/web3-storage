// SPDX-License-Identifier: GPL-3.0-only

//! Subxt-backed [`ChainFollower`]/[`ChainSession`]/[`FinalizedBlocks`] for the
//! chain-state coordinator, plus the [`ChainStateChainClient`] reads it drives
//! through them.
//!
//! [`SubxtChainFollower`] owns the node's single chain connection: the
//! transport, and the watch sender every other chain consumer reads the
//! connection from. It publishes each new connection only after its block
//! stream is confirmed up (see [`SubxtChainSession::subscribe`]), so
//! consumers never observe a handle whose backend failed immediately.

use provider_chain::chain_connection::{self, ChainHandle, ChainTransport};
use provider_chain::decode_block_events;
use provider_coordinator::{
    BlockUpdate, ChainFollower, ChainSession, ChainStateChainClient, Error, FinalizedBlock,
    FinalizedBlocks, ProviderLifecycleEvent,
};
use provider_types::{ProviderInfo, ProviderSettings, ProviderStats};
use sp_runtime::AccountId32;
use std::sync::Arc;
use storage_subxt::api::runtime_types::pallet_storage_provider::pallet::ProviderInfo as RuntimeProviderInfo;
use subxt::{OnlineClient, PolkadotConfig};
use tokio::sync::watch;

/// Pallet whose storage, constants, and events the coordinator follows.
const PALLET_NAME: &str = "StorageProvider";

// ── anchor block ──────────────────────────────────────────────────────────────

/// Query the pallet's `StorageProviderApi::current_anchor_block` runtime API —
/// the block every on-chain duration (timeouts, expiries, `valid_until`, nonce
/// age) is measured against. Reading it through the runtime API keeps the
/// provider agnostic to whether the anchor is a relay, parachain, or other
/// block number: the pallet decides via its `BlockNumberProvider`, and the
/// provider no longer reaches into a specific storage item.
pub(crate) async fn fetch_current_anchor_block<C>(
    at: &subxt::client::ClientAtBlock<PolkadotConfig, C>,
) -> Result<u32, Error>
where
    C: subxt::client::OnlineClientAtBlockT<PolkadotConfig>,
{
    // `unvalidated`: see the `storage-subxt` crate docs.
    at.runtime_apis()
        .call(
            storage_subxt::api::runtime_apis()
                .storage_provider_api()
                .current_anchor_block()
                .unvalidated(),
        )
        .await
        .map_err(|e| Error::Internal(format!("current_anchor_block runtime API call failed: {e}")))
}

/// Convert the runtime's `ProviderInfo` into the node's view of it.
///
/// Mirrors the runtime struct field for field (see
/// [`provider_types::ProviderInfo`]), resolving the runtime's bounded byte
/// vectors into the node's `String`/`Vec<u8>`.
///
/// A free function rather than a `From` impl: both types are foreign here
/// (`RuntimeProviderInfo` comes from `storage-subxt`'s generated bindings,
/// `ProviderInfo` from `provider-types`), so the orphan rule rules out the
/// trait impl.
fn provider_info_from_runtime(info: RuntimeProviderInfo) -> ProviderInfo {
    ProviderInfo {
        multiaddr: String::from_utf8_lossy(&info.multiaddr.0).into_owned(),
        public_key: info.public_key.0,
        stake: info.stake,
        committed_bytes: info.committed_bytes,
        settings: ProviderSettings {
            min_duration: info.settings.min_duration,
            max_duration: info.settings.max_duration,
            price_per_byte: info.settings.price_per_byte,
            accepting_primary: info.settings.accepting_primary,
            replica_sync_price: info.settings.replica_sync_price,
            accepting_extensions: info.settings.accepting_extensions,
            max_capacity: info.settings.max_capacity,
        },
        stats: ProviderStats {
            registered_at: info.stats.registered_at,
            agreements_total: info.stats.agreements_total,
            agreements_extended: info.stats.agreements_extended,
            agreements_not_extended: info.stats.agreements_not_extended,
            agreements_burned: info.stats.agreements_burned,
            total_bytes_committed: info.stats.total_bytes_committed,
            challenges_received_authorized: info.stats.challenges_received_authorized,
            challenges_received_public: info.stats.challenges_received_public,
            challenges_failed: info.stats.challenges_failed,
        },
        deregister_at: info.deregister_at,
    }
}

// ── chain reads ───────────────────────────────────────────────────────────────

/// Production [`ChainStateChainClient`] running typed storage queries —
/// through the generated `storage-subxt` bindings — on the coordinator's own
/// subxt connection (shared with the block subscription).
struct SubxtChainStateClient {
    api: OnlineClient<PolkadotConfig>,
}

/// Convert an account from the `sp_runtime` representation the node uses into
/// the `subxt` one the generated bindings expect. Same 32 bytes either way.
fn subxt_account(who: &AccountId32) -> subxt::utils::AccountId32 {
    subxt::utils::AccountId32(*<AccountId32 as AsRef<[u8; 32]>>::as_ref(who))
}

#[async_trait::async_trait]
impl ChainStateChainClient for SubxtChainStateClient {
    async fn get_provider_info(&self, who: &AccountId32) -> Result<Option<ProviderInfo>, Error> {
        // `unvalidated`: see the `storage-subxt` crate docs.
        let addr = storage_subxt::api::storage()
            .storage_provider()
            .providers()
            .unvalidated();
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
        Ok(Some(provider_info_from_runtime(info)))
    }

    async fn fetch_replay_hsn(&self, who: &AccountId32) -> Result<Option<u64>, Error> {
        // `unvalidated`: see the `storage-subxt` crate docs.
        let addr = storage_subxt::api::storage()
            .storage_provider()
            .provider_replay_states()
            .unvalidated();
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

        // `unvalidated`: see the `storage-subxt` crate docs.
        match at.constants().entry(
            storage_subxt::api::constants()
                .storage_provider()
                .request_timeout()
                .unvalidated(),
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

/// Names of the `StorageProvider` events that affect [`ProviderLifecycleEvent`],
/// paired with whether the event confirms a deregistration. Every one of these
/// carries a named `provider` field decodable as [`LifecycleProvider`].
const LIFECYCLE_EVENT_NAMES: &[(&str, bool)] = &[
    ("ProviderDeregistered", true),
    ("ProviderRegistered", false),
    ("ProviderSettingsUpdated", false),
    ("ProviderMultiaddrUpdated", false),
    ("DeregisterAnnounced", false),
    ("DeregisterCancelled", false),
];

/// The only field the coordinator reads out of a provider-lifecycle event.
///
/// Decoding just this field, rather than the whole generated event struct,
/// keeps the coordinator working across runtime changes that reshape fields it
/// never reads. `ProviderSettingsUpdated` in particular carries the full
/// `ProviderSettings`, so decoding it whole would break on any change to that
/// struct.
#[derive(subxt::ext::scale_decode::DecodeAsType)]
#[decode_as_type(crate_path = "::subxt::ext::scale_decode")]
struct LifecycleProvider {
    provider: subxt::utils::AccountId32,
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
            let deregistered = LIFECYCLE_EVENT_NAMES
                .iter()
                .find(|(name, _)| *name == event.event_name())
                .map(|(_, deregistered)| *deregistered)?;
            let provider = decode_provider(&event)?;
            Some(if deregistered {
                ProviderLifecycleEvent::Deregistered { provider }
            } else {
                ProviderLifecycleEvent::Updated { provider }
            })
        })
        .collect()
}

/// Decode the `provider` field out of a `StorageProvider` lifecycle event.
///
/// A shape mismatch (a runtime whose event fields drifted from the bindings)
/// is logged and skipped. For most of these events that is recoverable: the
/// next relevant event still triggers a fresh `refresh_provider_state`. A
/// missed `ProviderDeregistered` is the exception - a deregistered provider
/// emits nothing further, so only the next reconnect's bootstrap refresh
/// corrects it. The persisted nonce watermark is unaffected either way: its
/// reset is gated on a successfully decoded `Deregistered` in
/// `refresh_if_relevant_event`, so a missed decode simply leaves it as-is.
fn decode_provider(event: &subxt::events::Event<'_, PolkadotConfig>) -> Option<AccountId32> {
    match event.decode_fields_unchecked_as::<LifecycleProvider>() {
        Ok(LifecycleProvider { provider }) => Some(AccountId32::new(provider.0)),
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

// ── chain follower ───────────────────────────────────────────────────────────

/// [`ChainFollower`] over a subxt connection: builds a fresh client for
/// `transport`, and publishes it through `chain_tx` once
/// [`SubxtChainSession::subscribe`] confirms its block stream is up.
pub(crate) struct SubxtChainFollower {
    transport: ChainTransport,
    chain_tx: watch::Sender<Option<ChainHandle>>,
}

impl SubxtChainFollower {
    pub(crate) fn new(
        transport: ChainTransport,
        chain_tx: watch::Sender<Option<ChainHandle>>,
    ) -> Self {
        Self {
            transport,
            chain_tx,
        }
    }
}

#[async_trait::async_trait]
impl ChainFollower for SubxtChainFollower {
    async fn connect(&self) -> Result<Box<dyn ChainSession>, Error> {
        let handle = chain_connection::connect(&self.transport)
            .await
            .map_err(Error::from)?;
        Ok(Box::new(SubxtChainSession {
            handle,
            chain_tx: self.chain_tx.clone(),
        }))
    }
}

struct SubxtChainSession {
    handle: ChainHandle,
    chain_tx: watch::Sender<Option<ChainHandle>>,
}

#[async_trait::async_trait]
impl ChainSession for SubxtChainSession {
    async fn subscribe(
        self: Box<Self>,
    ) -> Result<(Box<dyn FinalizedBlocks>, Arc<dyn ChainStateChainClient>), Error> {
        let SubxtChainSession { handle, chain_tx } = *self;
        let api = handle.api.clone();
        let blocks = api
            .stream_blocks()
            .await
            .map_err(|e| Error::Internal(format!("Failed to subscribe to blocks: {e}")))?;

        // Publish the new connection only after the block stream is up, so
        // consumers never observe a handle whose backend failed immediately.
        chain_tx.send_replace(Some(handle));

        let chain: Arc<dyn ChainStateChainClient> = Arc::new(SubxtChainStateClient { api });
        let blocks: Box<dyn FinalizedBlocks> = Box::new(SubxtFinalizedBlocks { blocks });
        Ok((blocks, chain))
    }
}

struct SubxtFinalizedBlocks {
    blocks: subxt::client::Blocks<PolkadotConfig>,
}

#[async_trait::async_trait]
impl FinalizedBlocks for SubxtFinalizedBlocks {
    async fn next(&mut self) -> Option<BlockUpdate> {
        let block = match self.blocks.next().await {
            Some(Ok(block)) => block,
            Some(Err(e)) => {
                tracing::warn!("chain-state coordinator: block subscription error: {e}");
                return None;
            }
            None => return None,
        };
        let number = block.number() as u32;

        tracing::debug!("Finalized block: {}", number);

        // One block-scoped handle drives both reads below.
        let at = match block.at().await {
            Ok(at) => at,
            Err(e) => {
                tracing::warn!(
                    "chain-state coordinator: failed to get block handle for {number}: {e}"
                );
                return Some(BlockUpdate::Unreadable { number });
            }
        };

        // Track the pallet's anchor block (the clock all on-chain durations
        // are measured against) at this finalized block, via its runtime
        // API — so the provider never needs to know which block notion the
        // pallet uses. A failed read keeps the coordinator's previous value.
        let anchor_block = match fetch_current_anchor_block(&at).await {
            Ok(anchor_block) => Some(anchor_block),
            Err(e) => {
                tracing::warn!(
                    "chain-state coordinator: failed to fetch anchor block for block \
                     {number}: {e}; keeping previous value"
                );
                None
            }
        };

        let events = match at.events().fetch().await {
            Ok(events) => events,
            Err(e) => {
                tracing::warn!(
                    "chain-state coordinator: failed to fetch events for block {number}: {e}"
                );
                return Some(BlockUpdate::Unreadable { number });
            }
        };

        Some(BlockUpdate::Block(FinalizedBlock {
            number,
            anchor_block,
            events: decode_block_events(&events, number),
            lifecycle: parse_provider_lifecycle_events(&events),
        }))
    }
}
