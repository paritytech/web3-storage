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

use crate::chain_connection::{self, ChainHandle, ChainTransport};
use crate::event_decoding::decode_block_events;
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

/// Decode a finalized block's events down to the provider-lifecycle events, via
/// the generated `storage-subxt` event types.
fn parse_provider_lifecycle_events(
    events: &subxt::events::Events<PolkadotConfig>,
) -> Vec<ProviderLifecycleEvent> {
    events
        .iter()
        .filter_map(|event| event.ok())
        .filter_map(|event| lifecycle_event(&event))
        .collect()
}

/// Match `event` against the `StorageProvider` events that affect
/// [`ProviderLifecycleEvent`] and decode it against whichever one it is.
///
/// A shape mismatch (a runtime whose event fields drifted from the bindings)
/// is logged and skipped. For most of these events that is recoverable: the
/// next relevant event still triggers a fresh `refresh_provider_state`. A
/// missed `ProviderDeregistered` is the exception - a deregistered provider
/// emits nothing further, so only the next reconnect's bootstrap refresh
/// corrects it. The persisted nonce watermark is unaffected either way: its
/// reset is gated on a successfully decoded `Deregistered` in
/// `refresh_if_relevant_event`, so a missed decode simply leaves it as-is.
fn lifecycle_event(
    event: &subxt::events::Event<'_, PolkadotConfig>,
) -> Option<ProviderLifecycleEvent> {
    use storage_subxt::api::storage_provider::events::{
        DeregisterAnnounced, DeregisterCancelled, ProviderDeregistered, ProviderMultiaddrUpdated,
        ProviderRegistered, ProviderSettingsUpdated,
    };

    macro_rules! try_decode {
        ($ty:ty, $deregistered:expr) => {
            if let Some(result) = event.decode_fields_as::<$ty>() {
                return match result {
                    Ok(decoded) => Some(provider_lifecycle_event($deregistered, decoded.provider)),
                    Err(e) => {
                        tracing::warn!(
                            "chain-state coordinator: failed to decode {}::{} against the \
                             static bindings: {e}",
                            event.pallet_name(),
                            event.event_name(),
                        );
                        None
                    }
                };
            }
        };
    }

    try_decode!(ProviderDeregistered, true);
    try_decode!(ProviderRegistered, false);
    try_decode!(ProviderSettingsUpdated, false);
    try_decode!(ProviderMultiaddrUpdated, false);
    try_decode!(DeregisterAnnounced, false);
    try_decode!(DeregisterCancelled, false);
    None
}

fn provider_lifecycle_event(
    deregistered: bool,
    provider: subxt::utils::AccountId32,
) -> ProviderLifecycleEvent {
    let provider = AccountId32::new(provider.0);
    if deregistered {
        ProviderLifecycleEvent::Deregistered { provider }
    } else {
        ProviderLifecycleEvent::Updated { provider }
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
            .map_err(|e| Error::Internal(e.to_string()))?;
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

// ── tests ─────────────────────────────────────────────────────────────────────
//
// These drive the real subxt decode paths - the generated `storage-subxt`
// bindings, `parse_provider_lifecycle_events`, `decode_block_events` - through
// a real `OnlineClient` (legacy backend) backed by canned RPC responses, using
// the repo's tracked runtime metadata snapshot. Storage values and events are
// encoded with `scale_value` against the actual runtime types, so a runtime
// upgrade that renames or reshapes a field these read is caught here, not
// just by whichever coordinator happens to call them.
//
// The coordinator's own loop/state logic is tested separately, against a mock
// `ChainFollower`, in `provider-coordinator`'s own test suite.
#[cfg(test)]
mod tests {
    use super::*;
    use provider_events::BlockEvent;
    use subxt::backend::LegacyBackend;
    use subxt::ext::scale_value::scale::encode_as_type;
    use subxt::ext::scale_value::Value;
    use subxt_rpcs::client::mock_rpc_client::Json;
    use subxt_rpcs::client::{MockRpcClient, RpcClient};

    /// Pallet whose storage, constants, and events these tests exercise.
    const PALLET_NAME: &str = "StorageProvider";

    /// Tracked runtime metadata snapshot (shared with the PAPI codegen).
    const METADATA: &[u8] = include_bytes!(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../packages/papi/.papi/metadata/parachain.scale"
    ));
    const BLOCK_HASH: &str = "0x2222222222222222222222222222222222222222222222222222222222222222";
    const GENESIS_HASH: &str = "0x1111111111111111111111111111111111111111111111111111111111111111";

    fn metadata() -> subxt::Metadata {
        use codec::Decode;
        subxt::Metadata::decode(&mut &METADATA[..]).expect("tracked metadata decodes")
    }

    fn provider_account() -> AccountId32 {
        AccountId32::new([7u8; 32])
    }

    /// `0x`-prefixed twox128(pallet) ++ twox128(entry) storage-key prefix.
    fn key_prefix(pallet: &str, entry: &str) -> String {
        let mut key = sp_crypto_hashing::twox_128(pallet.as_bytes()).to_vec();
        key.extend(sp_crypto_hashing::twox_128(entry.as_bytes()));
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
    /// shape: every runtime field must be present for `scale_value` to encode
    /// it against the real type.
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
                    ("challenges_received_authorized", Value::u128(2)),
                    ("challenges_received_public", Value::u128(0)),
                    ("challenges_failed", Value::u128(1)),
                    ("lifetime_revenue", Value::u128(0)),
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

    /// `System::Events` bytes holding a `ProviderRegistered` (exercising the
    /// lifecycle decoding) and a `ChallengeCreated` (exercising the fan-out
    /// decoding) for `provider`, encoded against the real runtime types.
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

    /// The `BucketMembershipChanged` bucket ids [`decode_block_events`]
    /// produces from `events`, in encounter order.
    fn membership_changed_bucket_ids(events: &subxt::events::Events<PolkadotConfig>) -> Vec<u64> {
        decode_block_events(events, 0)
            .into_iter()
            .filter_map(|event| match event {
                BlockEvent::BucketMembershipChanged { bucket_id } => Some(bucket_id),
                _ => None,
            })
            .collect()
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
    /// `storage` maps a storage-key prefix (see [`key_prefix`]) to the hex
    /// value served for reads under it; unmapped keys read as absent. Every
    /// mocked header carries block number 42; the runtime API always answers
    /// `4242` for the anchor block, so a test can tell the two apart.
    async fn mock_api(storage: Vec<(String, String)>) -> subxt::OnlineClient<PolkadotConfig> {
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
                    // Anchor block the follower reads per finalized block.
                    // Deliberately distinct from the mocked header number (42)
                    // so a test fails if a decode ever regresses to reading
                    // the parachain height instead of the runtime API.
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
        subxt::OnlineClient::<PolkadotConfig>::from_backend(Arc::new(backend))
            .await
            .expect("client over mock RPC")
    }

    #[tokio::test]
    async fn request_timeout_constant_reads_from_real_metadata() {
        let md = metadata();
        let client = SubxtChainStateClient {
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
        let client = SubxtChainStateClient {
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
    /// `replica_sync_price`, `deregister_at` and the two stats counters on a
    /// field miss, which let a runtime mismatch degrade quietly.
    #[tokio::test]
    async fn provider_info_decode_failure_is_an_error() {
        let client = SubxtChainStateClient {
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
    /// error, not `Ok(None)`. Collapsing it to `None` would look identical to
    /// "provider has never signed", which seeds the nonce counter
    /// differently.
    #[tokio::test]
    async fn replay_hsn_decode_failure_is_an_error() {
        let client = SubxtChainStateClient {
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

        let client = SubxtChainStateClient {
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
        assert_eq!(info.settings.max_capacity, 10_000);
        assert_eq!(info.settings.replica_sync_price, Some(7));
        // Every `stats` counter is carried across, not just the two the node
        // reads today - a dropped field here is a silent zero.
        assert_eq!(info.stats.registered_at, 1);
        assert_eq!(info.stats.agreements_total, 3);
        assert_eq!(info.stats.total_bytes_committed, 500);
        assert_eq!(info.stats.challenges_received_authorized, 2);
        assert_eq!(info.stats.challenges_failed, 1);
        assert_eq!(info.deregister_at, Some(42));
    }

    /// A block carrying two different lifecycle events - one that only
    /// updates the provider, one that confirms deregistration - must decode
    /// each into the right [`ProviderLifecycleEvent`] variant, matched
    /// against its own generated event type.
    #[tokio::test]
    async fn lifecycle_events_decode_to_their_matching_variant() {
        let md = metadata();
        let account = provider_account();
        let account_bytes = <AccountId32 as AsRef<[u8]>>::as_ref(&account);

        let settings_updated = event_record(Value::named_variant(
            "ProviderSettingsUpdated",
            [
                ("provider", Value::from_bytes(account_bytes)),
                (
                    "settings",
                    Value::named_composite([
                        ("min_duration", Value::u128(10)),
                        ("max_duration", Value::u128(100)),
                        ("price_per_byte", Value::u128(5)),
                        ("accepting_primary", Value::bool(true)),
                        (
                            "replica_sync_price",
                            Value::unnamed_variant("None", Vec::<Value>::new()),
                        ),
                        ("accepting_extensions", Value::bool(true)),
                        ("max_capacity", Value::u128(10_000)),
                    ]),
                ),
            ],
        ));
        let deregistered = event_record(Value::named_variant(
            "ProviderDeregistered",
            [
                ("provider", Value::from_bytes(account_bytes)),
                ("stake_returned", Value::u128(1_000)),
            ],
        ));
        let events_ty = storage_value_type(&md, "System", "Events");
        let events_bytes = encode_value(
            &md,
            events_ty,
            &Value::unnamed_composite([settings_updated, deregistered]),
        );

        let api = mock_api(vec![(
            key_prefix("System", "Events"),
            format!("0x{}", hex::encode(events_bytes)),
        )])
        .await;
        let at = api.at_current_block().await.expect("block handle");
        let events = at.events().fetch().await.expect("events fetch");

        assert_eq!(
            parse_provider_lifecycle_events(&events),
            vec![
                ProviderLifecycleEvent::Updated {
                    provider: account.clone()
                },
                ProviderLifecycleEvent::Deregistered { provider: account },
            ]
        );
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

    /// An on-chain `ProviderDeregistered` must decode to the confirmed
    /// variant, not the generic `Updated` one - the coordinator gates
    /// clearing the nonce watermark strictly on `Deregistered`
    /// (`provider_coordinator::refresh_if_relevant_event`).
    #[tokio::test]
    async fn lifecycle_event_decodes_a_confirmed_deregistration() {
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
        let events_bytes = encode_value(&md, events_ty, &Value::unnamed_composite([deregistered]));

        let api = mock_api(vec![(
            key_prefix("System", "Events"),
            format!("0x{}", hex::encode(events_bytes)),
        )])
        .await;
        let at = api.at_current_block().await.expect("block handle");
        let events = at.events().fetch().await.expect("events fetch");

        assert_eq!(
            parse_provider_lifecycle_events(&events),
            vec![ProviderLifecycleEvent::Deregistered { provider: account }]
        );
    }

    /// End-to-end through [`SubxtFinalizedBlocks::next`]: the anchor block
    /// must come from the `StorageProviderApi::current_anchor_block` runtime
    /// API (`4242` in this mock), never the mocked header number (`42`), and
    /// the block's events and lifecycle events must both come through.
    #[tokio::test]
    async fn finalized_blocks_reports_the_anchor_block_and_decoded_updates() {
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
        ])
        .await;

        let mut blocks = SubxtFinalizedBlocks {
            blocks: api.stream_blocks().await.expect("subscribe to blocks"),
        };

        let update = blocks
            .next()
            .await
            .expect("the mock serves exactly one finalized block");
        let block = match update {
            BlockUpdate::Block(block) => block,
            BlockUpdate::Unreadable { number } => {
                panic!("block {number} should have decoded")
            }
        };

        assert_eq!(
            block.anchor_block,
            Some(4242),
            "anchor must come from the runtime API, not the header number (42)"
        );
        assert_eq!(
            block.lifecycle,
            vec![ProviderLifecycleEvent::Updated {
                provider: account.clone()
            }]
        );
        assert!(matches!(
            block.events.as_slice(),
            [BlockEvent::ChallengeCreated {
                deadline: 777,
                index: 3,
                bucket_id: 9,
                ref provider,
            }] if *provider == account
        ));
    }

    #[tokio::test]
    async fn subxt_chain_follower_connect_fails_fast_against_an_unreachable_url() {
        let (chain_tx, _chain_rx) = watch::channel(None);
        let follower = SubxtChainFollower::new(
            ChainTransport::Rpc {
                url: "ws://127.0.0.1:1".to_string(),
            },
            chain_tx,
        );

        let err = match follower.connect().await {
            Err(e) => e,
            Ok(_) => panic!("connect to a closed port must fail, not hang"),
        };
        assert!(
            err.to_string().contains("Failed to connect to chain"),
            "unexpected error: {err}"
        );
    }
}
