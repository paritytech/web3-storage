// SPDX-License-Identifier: GPL-3.0-only

//! Chain-backed [`MembershipResolver`] and [`MembershipInvalidations`].

use provider_auth::{
    BucketAccess, Invalidation, Member, MembershipError, MembershipInvalidations,
    MembershipResolver,
};
use provider_chain::chain_connection::{BestBlock, ChainHandle, ChainWatch};
use provider_chain::{BlockEvent, BlockEventRx};
use sp_core::crypto::AccountId32;
use std::sync::atomic::{AtomicBool, Ordering};
use storage_primitives::BucketId;
use storage_subxt::api::runtime_types::pallet_storage_provider::pallet::Member as RuntimeMember;
use subxt::client::{ClientAtBlock, OnlineClientAtBlockImpl};
use subxt::{OnlineClient, PolkadotConfig};
use tokio::sync::broadcast::error::TryRecvError;

/// Membership resolver over the node's shared chain connection, so lookups
/// follow reconnects instead of pinning their own socket.
pub struct ChainMembershipResolver {
    chain_rx: ChainWatch,
}

type AtBlock = ClientAtBlock<PolkadotConfig, OnlineClientAtBlockImpl<PolkadotConfig>>;

impl ChainMembershipResolver {
    pub fn new(chain_rx: ChainWatch) -> Self {
        Self { chain_rx }
    }

    /// Resolved per lookup so reconnects are picked up.
    fn connection(&self) -> Result<ChainHandle, MembershipError> {
        self.chain_rx
            .borrow()
            .clone()
            .ok_or_else(|| MembershipError::unavailable(provider_chain::Error::NotConnected))
    }
}

async fn read_at_best(
    api: &OnlineClient<PolkadotConfig>,
    block: &BestBlock,
    bucket_id: BucketId,
) -> Result<Option<BucketAccess>, MembershipError> {
    let at = api
        .at_block_hash_and_number(block.hash(), block.number())
        .await
        .map_err(MembershipError::unavailable)?;
    read_membership(&at, bucket_id).await
}

async fn read_at_finalized_head(
    api: &OnlineClient<PolkadotConfig>,
    bucket_id: BucketId,
) -> Result<Option<BucketAccess>, MembershipError> {
    let at = api
        .at_current_block()
        .await
        .map_err(MembershipError::unavailable)?;
    read_membership(&at, bucket_id).await
}

/// `None` when the bucket does not exist at `at`; `BlockNotKnown` when it
/// does not exist but its id is one the chain allocates next.
async fn read_membership(
    at: &AtBlock,
    bucket_id: BucketId,
) -> Result<Option<BucketAccess>, MembershipError> {
    let storage = storage_subxt::api::storage().storage_provider();
    let result = at
        .storage()
        .try_fetch(storage.buckets().unvalidated(), (bucket_id,))
        .await
        .map_err(MembershipError::unavailable)?;

    let Some(bucket_value) = result else {
        let next_id: BucketId = at
            .storage()
            .fetch(storage.next_bucket_id().unvalidated(), ())
            .await
            .map_err(MembershipError::unavailable)?
            .decode()
            .map_err(|e| MembershipError::Decode {
                bucket_id,
                reason: format!("NextBucketId: {e}"),
            })?;
        if bucket_id >= next_id {
            return Err(MembershipError::BlockNotKnown { bucket_id });
        }
        return Ok(None);
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

    Ok(Some(BucketAccess {
        members,
        visibility: bucket.visibility.into(),
    }))
}

#[async_trait::async_trait]
impl MembershipResolver for ChainMembershipResolver {
    async fn fetch_access(&self, bucket_id: BucketId) -> Result<BucketAccess, MembershipError> {
        let connection = self.connection()?;
        let api = &connection.api;
        let best_block = connection.best_block.borrow().clone();
        let access = match best_block {
            None => read_at_finalized_head(api, bucket_id).await?,
            Some(block) => match read_at_best(api, &block, bucket_id).await {
                Ok(access) => access,
                Err(MembershipError::Unavailable(reason)) => {
                    tracing::debug!(
                        bucket_id,
                        best_block = block.number(),
                        "auth: best block unreadable, reading at the finalized head: {reason}"
                    );
                    read_at_finalized_head(api, bucket_id).await?
                }
                Err(not_known @ MembershipError::BlockNotKnown { .. }) => {
                    match read_at_finalized_head(api, bucket_id).await {
                        Ok(Some(access)) => Some(access),
                        _ => return Err(not_known),
                    }
                }
                Err(other) => return Err(other),
            },
        };
        Ok(access.unwrap_or_else(|| BucketAccess::private(Vec::new())))
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
                // Events before this point were missed for good; keep draining
                // so the backlog clears.
                Ok(BlockEvent::Resubscribed { .. }) => all = true,
                // A membership event with no bucket to attribute it to, or a
                // best-chain fork switch: nothing cached can be trusted.
                Ok(
                    BlockEvent::MembershipScopeUnknown { .. } | BlockEvent::BestForkChanged { .. },
                ) => all = true,
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
    use provider_chain::chain_connection::ChainHandle;
    use provider_chain::mock_node::{header_json, mock_node, FINALIZED_HASH};
    use std::sync::Arc;
    use storage_primitives::Role;
    use storage_subxt::api::runtime_types::bounded_collections::bounded_vec::BoundedVec;
    use storage_subxt::api::runtime_types::pallet_storage_provider::pallet::Bucket;
    use storage_subxt::api::runtime_types::storage_primitives::Role as RuntimeRole;
    use storage_subxt::api::runtime_types::storage_primitives::Visibility as RuntimeVisibility;
    use subxt::backend::LegacyBackend;
    use subxt_rpcs::client::mock_rpc_client::Json;
    use subxt_rpcs::client::RpcClient;

    #[tokio::test]
    async fn chain_resolver_fails_cleanly_before_first_connect() {
        // Before the chain-state coordinator publishes a connection, auth
        // lookups must surface a retryable error rather than panic or hang.
        let (_tx, rx) = tokio::sync::watch::channel(None);
        let resolver = ChainMembershipResolver::new(rx);
        let err = resolver
            .fetch_access(1)
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
    fn a_best_fork_change_invalidates_everything() {
        let (tx, rx) = broadcast::channel(4);
        let _ = tx.send(BlockEvent::BucketMembershipChanged { bucket_id: 1 });
        let _ = tx.send(BlockEvent::BestForkChanged { at_block: 7 });

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

    /// Tracked runtime metadata snapshot (shared with the PAPI codegen).
    const METADATA: &[u8] = include_bytes!(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../packages/papi/.papi/metadata/parachain.scale"
    ));
    const ADMIN: [u8; 32] = [7u8; 32];
    const BUCKET: BucketId = 7;

    fn storage_prefix(entry: &str) -> String {
        let mut key = sp_crypto_hashing::twox_128(b"StorageProvider").to_vec();
        key.extend(sp_crypto_hashing::twox_128(entry.as_bytes()));
        format!("0x{}", hex::encode(key))
    }

    /// SCALE-encoded `Buckets` value with a single admin, as the node would
    /// serve it. The generated types only implement `EncodeAsType`, so the
    /// encoding goes through the metadata's type registry.
    fn bucket_with_admin(md: &subxt::Metadata) -> String {
        use subxt::ext::scale_encode::EncodeAsType;
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
            visibility: RuntimeVisibility::Private,
        };
        let bytes = bucket
            .encode_as_type(value_ty, md.types())
            .expect("bucket encodes as its runtime type");
        format!("0x{}", hex::encode(bytes))
    }

    /// What the mock node serves for `Buckets(BUCKET)` at one block.
    #[derive(Clone, Copy)]
    enum State {
        Present,
        Absent,
        Unreadable,
    }

    /// A resolver over a real `OnlineClient` on a mock node: `Buckets(BUCKET)`
    /// reads per block state, `NextBucketId` reads as `next_id` everywhere.
    /// With `best` set, a best block is published on the connection.
    async fn resolver(
        finalized: State,
        best: Option<State>,
        next_id: BucketId,
    ) -> ChainMembershipResolver {
        use codec::{Decode, Encode};
        use subxt::config::substrate::{DynamicHasher256, SubstrateHeader};
        use subxt::config::Hasher as _;

        let md = subxt::Metadata::decode(&mut &METADATA[..]).expect("tracked metadata decodes");
        let value_hex = bucket_with_admin(&md);
        let next_id_hex = format!("0x{}", hex::encode(next_id.encode()));
        let buckets_prefix = storage_prefix("Buckets");
        let next_id_prefix = storage_prefix("NextBucketId");
        let best_header = header_json(43, FINALIZED_HASH);
        let best_hash = {
            let header: SubstrateHeader<subxt::utils::H256> =
                serde_json::from_value(best_header.clone()).expect("header json decodes");
            format!("{:?}", DynamicHasher256::new(&md).hash(&header.encode()))
        };
        let best_state = best.unwrap_or(State::Absent);

        let mock = mock_node(METADATA, |_function| None)
            .method_handler("state_getStorage", move |params| {
                let best_hash = best_hash.clone();
                let value_hex = value_hex.clone();
                let next_id_hex = next_id_hex.clone();
                let buckets_prefix = buckets_prefix.clone();
                let next_id_prefix = next_id_prefix.clone();
                async move {
                    let (key, at) = key_and_at(params);
                    let state = if at == best_hash {
                        best_state
                    } else {
                        finalized
                    };
                    if key.starts_with(&next_id_prefix) {
                        return Ok(Json(Some(next_id_hex)));
                    }
                    assert!(
                        key.starts_with(&buckets_prefix),
                        "unexpected storage key {key}"
                    );
                    match state {
                        State::Present => Ok(Json(Some(value_hex))),
                        State::Absent => Ok(Json(None)),
                        State::Unreadable => Err(subxt_rpcs::Error::Client(Box::new(
                            std::io::Error::other("no peer knows this block"),
                        ))),
                    }
                }
            })
            .subscription_handler("chain_subscribeNewHeads", move |_params, _unsub| {
                let best_header = best_header.clone();
                async move { vec![Json(best_header)] }
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
        let api = OnlineClient::<PolkadotConfig>::from_backend(Arc::new(backend))
            .await
            .expect("client over mock RPC");

        let (best_tx, best_rx) = tokio::sync::watch::channel(None);
        if best.is_some() {
            let mut best_blocks = api.stream_best_blocks().await.expect("best-block stream");
            let block = best_blocks
                .next()
                .await
                .expect("one best head")
                .expect("best block");
            best_tx.send_replace(Some(block));
        }
        let handle = ChainHandle::from_api(api).with_best_block(best_rx);
        let (_chain_tx, chain_rx) = tokio::sync::watch::channel(Some(handle));
        ChainMembershipResolver::new(chain_rx)
    }

    fn key_and_at(params: Option<Box<serde_json::value::RawValue>>) -> (String, String) {
        let params: Vec<serde_json::Value> = params
            .and_then(|p| serde_json::from_str(p.get()).ok())
            .unwrap_or_default();
        let text = |index: usize| {
            params
                .get(index)
                .and_then(|value| value.as_str())
                .unwrap_or_default()
                .to_string()
        };
        (text(0), text(1))
    }

    fn admin_only() -> BucketAccess {
        BucketAccess::private(vec![(AccountId32::new(ADMIN), Role::Admin).into()])
    }

    #[tokio::test]
    async fn reads_at_the_best_block_when_one_is_published() {
        let resolver = resolver(State::Absent, Some(State::Present), BUCKET + 1).await;
        assert_eq!(resolver.fetch_access(BUCKET).await.unwrap(), admin_only());
    }

    #[tokio::test]
    async fn reads_at_the_finalized_head_without_a_best_block() {
        let resolver = resolver(State::Present, None, BUCKET + 1).await;
        assert_eq!(resolver.fetch_access(BUCKET).await.unwrap(), admin_only());
    }

    #[tokio::test]
    async fn falls_back_to_the_finalized_head_when_the_best_block_is_unreadable() {
        let resolver = resolver(State::Present, Some(State::Unreadable), BUCKET + 1).await;
        assert_eq!(resolver.fetch_access(BUCKET).await.unwrap(), admin_only());
    }

    #[tokio::test]
    async fn a_bucket_absent_at_a_stale_best_block_is_served_from_the_finalized_head() {
        let resolver = resolver(State::Present, Some(State::Absent), BUCKET).await;
        assert_eq!(resolver.fetch_access(BUCKET).await.unwrap(), admin_only());
    }

    #[tokio::test]
    async fn an_absent_bucket_at_or_above_the_counter_is_not_known_yet() {
        let resolver = resolver(State::Absent, Some(State::Absent), BUCKET).await;
        for bucket_id in [BUCKET, BUCKET + 1000] {
            let err = resolver.fetch_access(bucket_id).await.unwrap_err();
            assert!(
                matches!(err, MembershipError::BlockNotKnown { bucket_id: id } if id == bucket_id)
            );
        }
    }

    #[tokio::test]
    async fn an_absent_bucket_below_the_counter_is_a_miss() {
        let resolver = resolver(State::Absent, Some(State::Absent), BUCKET + 1).await;
        let access = resolver.fetch_access(BUCKET).await.unwrap();
        assert_eq!(access, BucketAccess::private(Vec::new()));
    }
}
