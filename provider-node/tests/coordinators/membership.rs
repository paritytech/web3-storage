// SPDX-License-Identifier: GPL-3.0-only

//! Integration tests for membership invalidation.
//!
//! The chain-state coordinator no longer pushes into the `Authenticator`; it
//! only broadcasts `BlockEvent::BucketMembershipChanged` /
//! `BlockEvent::Resubscribed` on the same fan-out the other coordinators
//! consume. The membership cache pulls from that feed itself, via
//! [`BlockEventInvalidations`], at the top of every lookup — so these tests
//! drive a `broadcast::Sender` directly (standing in for the coordinator) and
//! assert on the resulting cache behaviour, with no coordinator or `wait_for`
//! involved: the drain is synchronous on the request path.

use provider_auth::{
    build_auth_header, Authenticator, Member, MembershipError, MembershipResolver, RequiredRole,
};
use provider_chain::chain_events::BlockEvent;
use sp_core::{sr25519, Pair};
use sp_runtime::AccountId32;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::Duration;
use storage_primitives::{BucketId, Role};
use storage_provider_node::membership::BlockEventInvalidations;

/// Resolver that counts calls, so tests can tell whether a lookup hit the cache
/// or went to the resolver. Every bucket resolves to `account` as an `Admin`,
/// so an authorized request only fails when the resolver itself is bypassed.
struct CountingMembershipResolver {
    account: AccountId32,
    calls: Arc<AtomicUsize>,
}

#[async_trait::async_trait]
impl MembershipResolver for CountingMembershipResolver {
    async fn fetch_members(&self, _bucket_id: BucketId) -> Result<Vec<Member>, MembershipError> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        Ok(vec![(self.account.clone(), Role::Admin).into()])
    }
}

/// An [`Authenticator`] over [`CountingMembershipResolver`], fed by a
/// [`BlockEventInvalidations`] over the returned sender, plus the call counter
/// and the keypair whose requests it authorizes. The 300s TTL is long enough
/// that only an explicit event can cause a re-fetch within a test.
fn counting_authenticator() -> (
    Authenticator,
    Arc<AtomicUsize>,
    sr25519::Pair,
    tokio::sync::broadcast::Sender<BlockEvent>,
) {
    let keypair = sr25519::Pair::from_string("//Alice", None).expect("//Alice is a valid SURI");
    let calls = Arc::new(AtomicUsize::new(0));
    let (tx, rx) = tokio::sync::broadcast::channel(16);
    let auth = Authenticator::new(CountingMembershipResolver {
        account: AccountId32::new(keypair.public().0),
        calls: calls.clone(),
    })
    .with_ttl(Duration::from_secs(300))
    .with_invalidations(BlockEventInvalidations::new(rx));
    (auth, calls, keypair, tx)
}

/// Drive one authorized read of `bucket_id` through `auth`, the way an HTTP
/// handler does — so the assertions below observe the real cache path rather
/// than a test-only accessor.
async fn authorized_read(auth: &Authenticator, keypair: &sr25519::Pair, bucket_id: BucketId) {
    let timestamp = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .expect("system clock is after the Unix epoch")
        .as_secs();
    let header = build_auth_header(&keypair.public().0, "GET", bucket_id, timestamp, |msg| {
        keypair.sign(msg).0
    });
    auth.require_role(Some(&header), "GET", bucket_id, RequiredRole::Reader)
        .await
        .expect("the signer is an Admin of every bucket here");
}

#[tokio::test]
async fn membership_event_forces_the_next_lookup_to_re_resolve() {
    let (auth, calls, keypair, tx) = counting_authenticator();

    authorized_read(&auth, &keypair, 1).await;
    assert_eq!(calls.load(Ordering::SeqCst), 1);

    // Well inside the TTL, so this is served from cache.
    authorized_read(&auth, &keypair, 1).await;
    assert_eq!(
        calls.load(Ordering::SeqCst),
        1,
        "second read must be cached"
    );

    // A finalized block changed bucket 1's member set: the cached role can no
    // longer be trusted, TTL notwithstanding.
    let _ = tx.send(BlockEvent::BucketMembershipChanged { bucket_id: 1 });

    authorized_read(&auth, &keypair, 1).await;
    assert_eq!(
        calls.load(Ordering::SeqCst),
        2,
        "a membership change for the cached bucket must force a re-resolve"
    );
}

#[tokio::test]
async fn membership_event_leaves_other_buckets_cached() {
    let (auth, calls, keypair, tx) = counting_authenticator();

    authorized_read(&auth, &keypair, 1).await;
    assert_eq!(calls.load(Ordering::SeqCst), 1);

    // Invalidation is per bucket: a change in bucket 2 says nothing about
    // bucket 1.
    let _ = tx.send(BlockEvent::BucketMembershipChanged { bucket_id: 2 });

    authorized_read(&auth, &keypair, 1).await;
    assert_eq!(
        calls.load(Ordering::SeqCst),
        1,
        "another bucket's membership change must leave bucket 1 cached"
    );
}

#[tokio::test]
async fn events_for_uncached_buckets_are_a_noop() {
    let (auth, calls, keypair, tx) = counting_authenticator();

    // Nothing cached yet — an event for a bucket nobody has looked up must
    // not panic and must not itself cost a resolve.
    let _ = tx.send(BlockEvent::BucketMembershipChanged { bucket_id: 42 });

    // The next lookup for that same bucket drains the event first (calling
    // `invalidate` on an already-absent entry, which must be a no-op) and
    // then does its own ordinary resolve — exactly one call, not zero and
    // not two, proving the event was actually drained rather than never
    // reached at all.
    authorized_read(&auth, &keypair, 42).await;
    assert_eq!(
        calls.load(Ordering::SeqCst),
        1,
        "an event for a never-cached bucket must cost exactly the lookup's own resolve, not more"
    );
}

#[tokio::test]
async fn resubscribed_invalidates_every_cached_bucket() {
    let (auth, calls, keypair, tx) = counting_authenticator();

    authorized_read(&auth, &keypair, 1).await;
    authorized_read(&auth, &keypair, 2).await;
    assert_eq!(calls.load(Ordering::SeqCst), 2);

    // The block stream reconnected: events since the last connection may have
    // been missed, so every cached bucket must be distrusted, not just the
    // one a targeted event would have named. Before this change nothing
    // invalidated membership on reconnect at all — a `MemberRemoved` landing
    // during the gap survived until the TTL expired.
    let _ = tx.send(BlockEvent::Resubscribed { at_block: 100 });

    authorized_read(&auth, &keypair, 1).await;
    authorized_read(&auth, &keypair, 2).await;
    assert_eq!(
        calls.load(Ordering::SeqCst),
        4,
        "Resubscribed must force every cached bucket to re-resolve, not just one"
    );
}

#[tokio::test]
async fn membership_scope_unknown_invalidates_every_cached_bucket() {
    let (auth, calls, keypair, tx) = counting_authenticator();

    authorized_read(&auth, &keypair, 1).await;
    authorized_read(&auth, &keypair, 2).await;
    assert_eq!(calls.load(Ordering::SeqCst), 2);

    // A membership event whose fields failed to decode carries no bucket id
    // to invalidate, so it must fall back to distrusting every cached
    // bucket - the same reaction as `Resubscribed`, but without implying the
    // follower reconnected.
    let _ = tx.send(BlockEvent::MembershipScopeUnknown { at_block: 100 });

    authorized_read(&auth, &keypair, 1).await;
    authorized_read(&auth, &keypair, 2).await;
    assert_eq!(
        calls.load(Ordering::SeqCst),
        4,
        "MembershipScopeUnknown must force every cached bucket to re-resolve, not just one"
    );
}

#[tokio::test]
async fn unrelated_events_leave_the_cache_alone() {
    let (auth, calls, keypair, tx) = counting_authenticator();

    authorized_read(&auth, &keypair, 1).await;
    assert_eq!(calls.load(Ordering::SeqCst), 1);

    // A checkpoint event carries no membership information and must not evict
    // anything.
    let _ = tx.send(BlockEvent::BucketCheckpointed { bucket_id: 1 });

    authorized_read(&auth, &keypair, 1).await;
    assert_eq!(
        calls.load(Ordering::SeqCst),
        1,
        "an unrelated event must not force a re-resolve"
    );
}
