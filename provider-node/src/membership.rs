// SPDX-License-Identifier: GPL-3.0-only

//! Chain-backed [`MembershipResolver`] and [`MembershipInvalidations`].

use provider_auth::{
    Invalidation, Member, MembershipError, MembershipInvalidations, MembershipResolver,
};
use provider_chain::chain_connection::{self, ChainWatch};
use provider_chain::{BlockEvent, BlockEventRx};
use sp_core::crypto::AccountId32;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;
use storage_primitives::BucketId;
use storage_subxt::api::runtime_types::pallet_storage_provider::pallet::Member as RuntimeMember;
use subxt::{OnlineClient, PolkadotConfig};
use tokio::sync::broadcast::error::{RecvError, TryRecvError};

/// Membership resolver over the node's shared chain connection, so lookups
/// follow reconnects instead of pinning their own socket.
pub struct ChainMembershipResolver {
    chain_rx: ChainWatch,
    /// Never read directly: each lookup `resubscribe`s, so it only sees events
    /// published after that lookup started.
    events: BlockEventRx,
    finality_grace: Duration,
}

impl ChainMembershipResolver {
    /// `finality_grace` bounds how long a bucket unknown at the connection's
    /// finalized head waits for its creation event before it counts as
    /// nonexistent; zero answers at once.
    pub fn new(chain_rx: ChainWatch, events: BlockEventRx, finality_grace: Duration) -> Self {
        Self {
            chain_rx,
            events,
            finality_grace,
        }
    }

    /// Resolved per lookup so reconnects are picked up.
    fn api(&self) -> Result<OnlineClient<PolkadotConfig>, MembershipError> {
        chain_connection::current_api(&self.chain_rx)
            .map_err(|e| MembershipError::Unavailable(e.to_string()))
    }

    /// The bucket at the connection's finalized head.
    async fn lookup(&self, bucket_id: BucketId) -> Result<Lookup, MembershipError> {
        let api = self.api()?;

        // `unvalidated`: see the `storage-subxt` crate docs.
        let storage_address = storage_subxt::api::storage()
            .storage_provider()
            .buckets()
            .unvalidated();

        let at = api
            .at_current_block()
            .await
            .map_err(|e| MembershipError::Unavailable(e.to_string()))?;
        let result = at
            .storage()
            .try_fetch(storage_address, (bucket_id,))
            .await
            .map_err(|e| MembershipError::Unavailable(e.to_string()))?;

        // Absent. An id the chain has not handed out yet, close enough to the
        // counter to have been handed out in blocks we have not seen, may
        // still be on its way; anything else is deleted, or a probe.
        let Some(bucket_value) = result else {
            let next_id: BucketId = match at
                .storage()
                .try_fetch(
                    storage_subxt::api::storage()
                        .storage_provider()
                        .next_bucket_id()
                        .unvalidated(),
                    (),
                )
                .await
                .map_err(|e| MembershipError::Unavailable(e.to_string()))?
            {
                Some(value) => value.decode().map_err(|e| MembershipError::Decode {
                    bucket_id,
                    reason: format!("NextBucketId: {e}"),
                })?,
                None => 0,
            };
            let pending =
                bucket_id >= next_id && bucket_id.saturating_sub(next_id) < PENDING_ID_WINDOW;
            return Ok(Lookup::Absent { pending });
        };

        let bucket = bucket_value.decode().map_err(|e| MembershipError::Decode {
            bucket_id,
            reason: e.to_string(),
        })?;

        let members = member_roles(bucket.members.0);

        // `create_bucket` seeds an admin and `remove_member` refuses to drop the
        // last one, so zero members means something changed chain-side. The
        // caller reads it as "not a member".
        if members.is_empty() {
            tracing::warn!(bucket_id, "auth: bucket decoded with zero members");
        } else {
            tracing::debug!(bucket_id, count = members.len(), "auth: resolved members");
        }

        Ok(Lookup::Found(members))
    }
}

/// How far above the chain's `NextBucketId` an absent bucket id may lie and
/// still be waited for: ids the chain could have allocated in the finality
/// step this node has not seen yet. Beyond it, a miss is a probe and gets no
/// grace, which caps how many connections a signed id scan can park.
const PENDING_ID_WINDOW: BucketId = 64;

enum Lookup {
    Found(Vec<Member>),
    /// No such bucket at the finalized head; `pending` when it may still be
    /// created in blocks this node has not seen.
    Absent {
        pending: bool,
    },
}

#[async_trait::async_trait]
impl MembershipResolver for ChainMembershipResolver {
    async fn fetch_members(&self, bucket_id: BucketId) -> Result<Vec<Member>, MembershipError> {
        // Subscribed before the read, so a creation event landing between the
        // read and the wait below cannot be missed.
        let events = self.events.resubscribe();
        match self.lookup(bucket_id).await? {
            Lookup::Found(members) => return Ok(members),
            Lookup::Absent { pending: false } => return Ok(vec![]),
            Lookup::Absent { pending: true } if self.finality_grace.is_zero() => {
                return Ok(vec![]);
            }
            Lookup::Absent { pending: true } => {}
        }
        // A client that has just watched its `create_bucket` finalize can be a
        // finality step ahead of us: the embedded light client learns finality
        // seconds after a full node reports it. Give the bucket's creation
        // event that long to arrive before answering "not a member".
        await_bucket_event(events, bucket_id, self.finality_grace).await;
        match self.lookup(bucket_id).await? {
            Lookup::Found(members) => Ok(members),
            Lookup::Absent { .. } => Ok(vec![]),
        }
    }
}

/// Block until an event that could make `bucket_id` resolvable arrives - its
/// own membership change, or a wholesale re-read / lag that may have carried
/// it - or `grace` elapses. A closed feed returns at once. The caller re-reads
/// either way, so returning early is never wrong, only cheaper.
async fn await_bucket_event(mut events: BlockEventRx, bucket_id: BucketId, grace: Duration) {
    let deadline = tokio::time::Instant::now() + grace;
    loop {
        match tokio::time::timeout_at(deadline, events.recv()).await {
            Ok(Ok(BlockEvent::BucketMembershipChanged { bucket_id: changed }))
                if changed == bucket_id =>
            {
                return;
            }
            Ok(Ok(BlockEvent::Resubscribed { .. } | BlockEvent::MembershipScopeUnknown { .. })) => {
                return;
            }
            Ok(Ok(_)) => {}
            Ok(Err(RecvError::Lagged(_) | RecvError::Closed)) | Err(_) => return,
        }
    }
}

fn member_roles(members: Vec<RuntimeMember>) -> Vec<Member> {
    members
        .into_iter()
        .map(|m| (AccountId32::new(m.account.0), m.role.into()).into())
        .collect()
}

/// [`MembershipInvalidations`] over the chain-state coordinator's per-block
/// fan-out.
///
/// `Mutex` rather than requiring `&mut self`, because the cache drains
/// through a shared reference; `try_recv` is synchronous, so no guard is ever
/// held across an `.await`.
pub struct BlockEventInvalidations {
    events: parking_lot::Mutex<BlockEventRx>,
    /// Set once a closed feed has been logged, so a dead follower doesn't
    /// spam a warning on every subsequent authenticated request.
    closed_warned: AtomicBool,
}

impl BlockEventInvalidations {
    pub fn new(events: BlockEventRx) -> Self {
        Self {
            events: parking_lot::Mutex::new(events),
            closed_warned: AtomicBool::new(false),
        }
    }
}

impl MembershipInvalidations for BlockEventInvalidations {
    fn drain(&self) -> Invalidation {
        let mut events = self.events.lock();
        let mut buckets = Vec::new();
        let mut all = false;
        loop {
            match events.try_recv() {
                Ok(BlockEvent::BucketMembershipChanged { bucket_id }) if !all => {
                    buckets.push(bucket_id)
                }
                // The follower re-read chain state wholesale, or this task
                // fell behind the fan-out — either way, events before this
                // point were missed for good. Keep draining rather than
                // returning here, so the backlog actually clears instead of
                // leaving the feed permanently lagged.
                Ok(BlockEvent::Resubscribed { .. }) => all = true,
                // A membership event's bucket id could not be attributed to
                // a specific bucket (decode failure or a dropped block) - the
                // same "trust nothing cached" reaction as Resubscribed, but
                // it does not imply anything about the other event kinds.
                Ok(BlockEvent::MembershipScopeUnknown { .. }) => all = true,
                Ok(_) => {}
                Err(TryRecvError::Lagged(_)) => all = true,
                Err(TryRecvError::Empty) => break,
                // The follower is gone. Degrade to TTL-only expiry rather than
                // failing authorization closed — a dead follower must not
                // take the node's auth path down with it.
                Err(TryRecvError::Closed) => {
                    if !self.closed_warned.swap(true, Ordering::Relaxed) {
                        tracing::warn!(
                            "membership invalidation feed closed; falling back to TTL-only expiry"
                        );
                    }
                    break;
                }
            }
        }
        if all {
            Invalidation::All
        } else if buckets.is_empty() {
            Invalidation::None
        } else {
            Invalidation::Buckets(buckets)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use storage_primitives::Role;

    #[tokio::test]
    async fn chain_resolver_fails_cleanly_before_first_connect() {
        // Before the chain-state coordinator publishes a connection, auth
        // lookups must surface a retryable error rather than panic or hang.
        let (_tx, rx) = tokio::sync::watch::channel(None);
        let (_events_tx, events_rx) = tokio::sync::broadcast::channel(1);
        let resolver = ChainMembershipResolver::new(rx, events_rx, Duration::ZERO);
        let err = resolver
            .fetch_members(1)
            .await
            .expect_err("no connection published yet");
        // Retryable, not a decode bug — the node maps this onto a 503.
        assert!(
            matches!(err, MembershipError::Unavailable(_)),
            "unexpected error: {err}"
        );
    }

    /// Pins the generated-type -> primitives conversion for every role.
    #[test]
    fn member_roles_converts_accounts_and_roles() {
        use storage_subxt::api::runtime_types::storage_primitives::Role as RuntimeRole;

        let member = |byte: u8, role: RuntimeRole| RuntimeMember {
            account: subxt::utils::AccountId32([byte; 32]),
            role,
        };

        let expected: Vec<Member> = vec![
            (AccountId32::new([1u8; 32]), Role::Admin).into(),
            (AccountId32::new([2u8; 32]), Role::Writer).into(),
            (AccountId32::new([3u8; 32]), Role::Reader).into(),
        ];
        assert_eq!(
            member_roles(vec![
                member(1, RuntimeRole::Admin),
                member(2, RuntimeRole::Writer),
                member(3, RuntimeRole::Reader),
            ]),
            expected
        );
    }

    // ── BlockEventInvalidations ─────────────────────────────────────────────

    use tokio::sync::broadcast;

    #[test]
    fn a_lagged_feed_invalidates_everything() {
        // Overflow the small buffer without ever draining, so the receiver's
        // first read comes back `Lagged` rather than `Ok`.
        let (tx, rx) = broadcast::channel(2);
        for bucket_id in 0..5 {
            let _ = tx.send(BlockEvent::BucketMembershipChanged { bucket_id });
        }

        let feed = BlockEventInvalidations::new(rx);
        assert_eq!(feed.drain(), Invalidation::All);
    }

    #[test]
    fn drain_clears_the_backlog_past_a_lag() {
        let (tx, rx) = broadcast::channel(2);
        for bucket_id in 0..5 {
            let _ = tx.send(BlockEvent::BucketMembershipChanged { bucket_id });
        }
        let feed = BlockEventInvalidations::new(rx);
        assert_eq!(feed.drain(), Invalidation::All);

        // The first drain must have consumed the messages still buffered past
        // the lag, not just flagged `All` and left them queued — otherwise a
        // second drain with nothing new sent would still find them.
        assert_eq!(feed.drain(), Invalidation::None);
    }

    #[test]
    fn a_closed_feed_degrades_to_ttl_only() {
        let (tx, rx) = broadcast::channel::<BlockEvent>(4);
        drop(tx);

        // A dead follower must not fail authorization closed: the feed simply
        // has nothing more to report, leaving the TTL as the only bound.
        let feed = BlockEventInvalidations::new(rx);
        assert_eq!(feed.drain(), Invalidation::None);
    }

    // ── finality grace ──────────────────────────────────────────────────────

    #[tokio::test(start_paused = true)]
    async fn grace_wait_ends_on_the_buckets_own_event_only() {
        let (tx, rx) = broadcast::channel(8);
        let waited = tokio::spawn(await_bucket_event(rx, 7, Duration::from_secs(60)));
        // Another bucket's change must not release the wait.
        tx.send(BlockEvent::BucketMembershipChanged { bucket_id: 9 })
            .unwrap();
        tokio::task::yield_now().await;
        assert!(!waited.is_finished());
        tx.send(BlockEvent::BucketMembershipChanged { bucket_id: 7 })
            .unwrap();
        waited.await.unwrap();
        // Released by the event, not by the deadline.
        assert!(tokio::time::Instant::now().elapsed() < Duration::from_secs(60));
    }

    #[tokio::test(start_paused = true)]
    async fn grace_wait_ends_on_a_wholesale_reread_or_a_closed_feed() {
        let (tx, rx) = broadcast::channel(8);
        tx.send(BlockEvent::Resubscribed { at_block: 3 }).unwrap();
        await_bucket_event(rx, 7, Duration::from_secs(60)).await;

        let (tx, rx) = broadcast::channel::<BlockEvent>(8);
        drop(tx);
        await_bucket_event(rx, 7, Duration::from_secs(60)).await;
        assert!(tokio::time::Instant::now().elapsed() < Duration::from_secs(60));
    }

    #[tokio::test(start_paused = true)]
    async fn grace_wait_gives_up_at_the_deadline() {
        let (_tx, rx) = broadcast::channel::<BlockEvent>(8);
        let start = tokio::time::Instant::now();
        await_bucket_event(rx, 7, Duration::from_secs(12)).await;
        assert_eq!(start.elapsed(), Duration::from_secs(12));
    }

    /// The resolver over a real `OnlineClient`, backed by a mock RPC that
    /// serves the tracked runtime metadata and a bucket that "appears" on cue.
    mod real_client {
        use super::*;
        use provider_chain::chain_connection::ChainHandle;
        use std::sync::atomic::AtomicUsize;
        use std::sync::Arc;
        use storage_subxt::api::runtime_types::bounded_collections::bounded_vec::BoundedVec;
        use storage_subxt::api::runtime_types::pallet_storage_provider::pallet::Bucket;
        use storage_subxt::api::runtime_types::storage_primitives::Role as RuntimeRole;
        use subxt::backend::LegacyBackend;
        use subxt_rpcs::client::mock_rpc_client::Json;
        use subxt_rpcs::client::{MockRpcClient, RpcClient};

        /// Tracked runtime metadata snapshot (shared with the PAPI codegen).
        const METADATA: &[u8] = include_bytes!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../packages/papi/.papi/metadata/parachain.scale"
        ));
        const GENESIS_HASH: &str =
            "0x1111111111111111111111111111111111111111111111111111111111111111";
        const FINALIZED_HASH: &str =
            "0x2222222222222222222222222222222222222222222222222222222222222222";
        const ADMIN: [u8; 32] = [7u8; 32];
        const BUCKET: BucketId = 7;

        fn storage_prefix(entry: &str) -> String {
            let mut key = sp_crypto_hashing::twox_128(b"StorageProvider").to_vec();
            key.extend(sp_crypto_hashing::twox_128(entry.as_bytes()));
            format!("0x{}", hex::encode(key))
        }

        /// SCALE-encoded `Buckets` value with a single admin, as the node
        /// would serve it. The generated types only implement `EncodeAsType`,
        /// so the encoding goes through the metadata's type registry.
        fn bucket_with_admin() -> String {
            use codec::Decode;
            use subxt::ext::scale_encode::EncodeAsType;
            let md = subxt::Metadata::decode(&mut &METADATA[..]).expect("tracked metadata decodes");
            let value_ty = md
                .pallet_by_name("StorageProvider")
                .expect("pallet in metadata")
                .storage()
                .expect("pallet has storage")
                .entry_by_name("Buckets")
                .expect("entry in metadata")
                .value_ty();
            let bucket = Bucket {
                members: BoundedVec(vec![RuntimeMember {
                    account: subxt::utils::AccountId32(ADMIN),
                    role: RuntimeRole::Admin,
                }]),
                frozen_start_seq: None,
                min_providers: 1,
                primary_providers: BoundedVec(vec![]),
                snapshot: None,
                historical_roots: [(0, subxt::utils::H256([0u8; 32])); 6],
                total_snapshots: 0,
            };
            let bytes = bucket
                .encode_as_type(value_ty, md.types())
                .expect("bucket encodes as its runtime type");
            format!("0x{}", hex::encode(bytes))
        }

        /// A real `OnlineClient` over a mock node. `Buckets(BUCKET)` reads as
        /// absent until `created` is set and `NextBucketId` reads as `next_id`;
        /// `reads` counts the `Buckets` reads.
        async fn mock_api(
            created: Arc<AtomicBool>,
            reads: Arc<AtomicUsize>,
            next_id: BucketId,
        ) -> OnlineClient<PolkadotConfig> {
            let metadata_hex = format!("0x{}", hex::encode(METADATA));
            let value_hex = bucket_with_admin();
            let next_id_hex = {
                use codec::Encode;
                format!("0x{}", hex::encode(next_id.encode()))
            };
            let buckets_prefix = storage_prefix("Buckets");
            let next_id_prefix = storage_prefix("NextBucketId");
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
                        "Metadata_metadata_versions" => vec![u32::from(METADATA[4])].encode(),
                        "Metadata_metadata_at_version" => Some(METADATA.to_vec()).encode(),
                        "Metadata_metadata" => METADATA.to_vec().encode(),
                        // sp_version::RuntimeVersion, field by field.
                        "Core_version" => (
                            "test".to_string(),
                            "test".to_string(),
                            1u32,
                            1u32,
                            1u32,
                            Vec::<([u8; 8], u32)>::new(),
                            1u32,
                            1u8,
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
                    Json(FINALIZED_HASH.to_string())
                })
                .method_handler("chain_getHeader", |_params| async {
                    Json(serde_json::json!({
                        "parentHash": GENESIS_HASH,
                        "number": "0x2a",
                        "stateRoot": GENESIS_HASH,
                        "extrinsicsRoot": GENESIS_HASH,
                        "digest": { "logs": [] }
                    }))
                })
                .method_handler("state_getRuntimeVersion", |_params| async {
                    Json(serde_json::json!({
                        "specName": "test",
                        "implName": "test",
                        "authoringVersion": 1,
                        "specVersion": 1,
                        "implVersion": 1,
                        "apis": [],
                        "transactionVersion": 1,
                        "stateVersion": 1
                    }))
                })
                .method_handler("state_getStorage", move |params| {
                    let created = created.clone();
                    let reads = reads.clone();
                    let value_hex = value_hex.clone();
                    let next_id_hex = next_id_hex.clone();
                    let buckets_prefix = buckets_prefix.clone();
                    let next_id_prefix = next_id_prefix.clone();
                    async move {
                        let raw = params.map(|p| p.get().to_string()).unwrap_or_default();
                        let key: String = serde_json::from_str::<Vec<serde_json::Value>>(&raw)
                            .ok()
                            .and_then(|p| p.first().and_then(|k| k.as_str().map(str::to_string)))
                            .unwrap_or_default();
                        if key.starts_with(&buckets_prefix) {
                            reads.fetch_add(1, Ordering::Relaxed);
                            Json(created.load(Ordering::Relaxed).then_some(value_hex))
                        } else if key.starts_with(&next_id_prefix) {
                            Json(Some(next_id_hex))
                        } else {
                            panic!("mock RPC: unexpected storage key {key}");
                        }
                    }
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
            OnlineClient::<PolkadotConfig>::from_backend(Arc::new(backend))
                .await
                .expect("client over mock RPC")
        }

        struct Harness {
            resolver: ChainMembershipResolver,
            created: Arc<AtomicBool>,
            reads: Arc<AtomicUsize>,
            events: broadcast::Sender<BlockEvent>,
            // Keeps the published connection alive for the resolver's watch.
            _chain: tokio::sync::watch::Sender<Option<ChainHandle>>,
        }

        /// `next_id` is the chain's `NextBucketId` at the finalized head.
        async fn harness(created: bool, next_id: BucketId, grace: Duration) -> Harness {
            let created = Arc::new(AtomicBool::new(created));
            let reads = Arc::new(AtomicUsize::new(0));
            let api = mock_api(created.clone(), reads.clone(), next_id).await;
            let (chain_tx, chain_rx) =
                tokio::sync::watch::channel(Some(ChainHandle::from_api(api)));
            let (events, events_rx) = broadcast::channel(8);
            Harness {
                resolver: ChainMembershipResolver::new(chain_rx, events_rx, grace),
                created,
                reads,
                events,
                _chain: chain_tx,
            }
        }

        fn admin_only() -> Vec<Member> {
            vec![(AccountId32::new(ADMIN), Role::Admin).into()]
        }

        #[tokio::test(start_paused = true)]
        async fn a_known_bucket_resolves_on_the_first_read() {
            let h = harness(true, BUCKET + 1, Duration::from_secs(12)).await;
            assert_eq!(
                h.resolver.fetch_members(BUCKET).await.unwrap(),
                admin_only()
            );
            assert_eq!(h.reads.load(Ordering::Relaxed), 1);
        }

        #[tokio::test(start_paused = true)]
        async fn an_unknown_bucket_waits_for_its_creation_event_then_resolves() {
            let h = harness(false, BUCKET, Duration::from_secs(12)).await;
            let created = h.created.clone();
            let events = h.events.clone();
            // The bucket's block reaches this node's finalized view 2s later
            // than the client's: state flips, then the follower fans the
            // creation event out.
            tokio::spawn(async move {
                tokio::time::sleep(Duration::from_secs(2)).await;
                created.store(true, Ordering::Relaxed);
                events
                    .send(BlockEvent::BucketMembershipChanged { bucket_id: BUCKET })
                    .unwrap();
            });
            let start = tokio::time::Instant::now();
            assert_eq!(
                h.resolver.fetch_members(BUCKET).await.unwrap(),
                admin_only()
            );
            assert_eq!(h.reads.load(Ordering::Relaxed), 2);
            assert!(
                start.elapsed() < Duration::from_secs(12),
                "released by the event"
            );
        }

        #[tokio::test(start_paused = true)]
        async fn an_unknown_bucket_is_empty_once_the_grace_runs_out() {
            let h = harness(false, BUCKET, Duration::from_secs(12)).await;
            let start = tokio::time::Instant::now();
            assert!(h.resolver.fetch_members(BUCKET).await.unwrap().is_empty());
            // Re-read after the grace, in case the event was simply not fanned out.
            assert_eq!(h.reads.load(Ordering::Relaxed), 2);
            assert_eq!(start.elapsed(), Duration::from_secs(12));
        }

        #[tokio::test(start_paused = true)]
        async fn zero_grace_answers_unknown_buckets_at_once() {
            let h = harness(false, BUCKET, Duration::ZERO).await;
            assert!(h.resolver.fetch_members(BUCKET).await.unwrap().is_empty());
            assert_eq!(h.reads.load(Ordering::Relaxed), 1);
        }

        #[tokio::test(start_paused = true)]
        async fn a_deleted_bucket_gets_no_grace() {
            // The chain has moved past BUCKET, so its absence is final.
            let h = harness(false, BUCKET + 20, Duration::from_secs(12)).await;
            let start = tokio::time::Instant::now();
            assert!(h.resolver.fetch_members(BUCKET).await.unwrap().is_empty());
            assert_eq!(h.reads.load(Ordering::Relaxed), 1);
            assert_eq!(start.elapsed(), Duration::ZERO);
        }

        #[tokio::test(start_paused = true)]
        async fn an_id_scan_far_above_the_counter_gets_no_grace() {
            let h = harness(false, 1, Duration::from_secs(12)).await;
            let start = tokio::time::Instant::now();
            assert!(h
                .resolver
                .fetch_members(1_000_007)
                .await
                .unwrap()
                .is_empty());
            // First id past the window: no grace either.
            assert!(h
                .resolver
                .fetch_members(1 + PENDING_ID_WINDOW)
                .await
                .unwrap()
                .is_empty());
            assert_eq!(h.reads.load(Ordering::Relaxed), 2);
            assert_eq!(start.elapsed(), Duration::ZERO);
        }
    }
}
