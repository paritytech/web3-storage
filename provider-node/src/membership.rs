// SPDX-License-Identifier: GPL-3.0-only

//! Chain-backed [`MembershipResolver`] and [`MembershipInvalidations`].

use provider_auth::{
    BucketAccess, Invalidation, Member, MembershipError, MembershipInvalidations,
    MembershipResolver,
};
use provider_chain::chain_connection::{self, ChainWatch};
use provider_chain::{BlockEvent, BlockEventRx};
use sp_core::crypto::AccountId32;
use std::sync::atomic::{AtomicBool, Ordering};
use storage_primitives::BucketId;
use storage_subxt::api::runtime_types::pallet_storage_provider::pallet::Member as RuntimeMember;
use subxt::{OnlineClient, PolkadotConfig};
use tokio::sync::broadcast::error::TryRecvError;

/// Membership resolver over the node's shared chain connection, so lookups
/// follow reconnects instead of pinning their own socket.
pub struct ChainMembershipResolver {
    chain_rx: ChainWatch,
}

impl ChainMembershipResolver {
    pub fn new(chain_rx: ChainWatch) -> Self {
        Self { chain_rx }
    }

    /// Resolved per lookup so reconnects are picked up.
    fn api(&self) -> Result<OnlineClient<PolkadotConfig>, MembershipError> {
        chain_connection::current_api(&self.chain_rx)
            .map_err(|e| MembershipError::Unavailable(e.to_string()))
    }
}

#[async_trait::async_trait]
impl MembershipResolver for ChainMembershipResolver {
    async fn fetch_access(&self, bucket_id: BucketId) -> Result<BucketAccess, MembershipError> {
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

        // No such bucket: memberless and member-only, so nobody gets in.
        // Distinct from a bucket we cannot decode, below.
        let Some(bucket_value) = result else {
            return Ok(BucketAccess::private(Vec::new()));
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

        Ok(BucketAccess {
            members,
            visibility: bucket.visibility.into(),
        })
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
}
