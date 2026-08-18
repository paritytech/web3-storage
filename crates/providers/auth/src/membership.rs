// SPDX-License-Identifier: Apache-2.0

//! Bucket membership and the role ladder. Resolution is left to
//! [`MembershipResolver`] and change notification to [`MembershipInvalidations`],
//! so this crate needs no chain dependency.

use crate::error::MembershipError;
use dashmap::DashMap;
use sp_core::crypto::AccountId32;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant};
use storage_primitives::{BucketId, Role};

/// A bucket member and the role they hold.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Member {
    /// The member's on-chain account.
    pub account: AccountId32,
    /// What that account may do in the bucket.
    pub role: Role,
}

impl From<(AccountId32, Role)> for Member {
    fn from((account, role): (AccountId32, Role)) -> Self {
        Self { account, role }
    }
}

/// Required role for an endpoint.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RequiredRole {
    /// Any member of the bucket.
    Reader,
    /// `Writer` or `Admin`.
    Writer,
    /// `Admin` only.
    Admin,
}

impl RequiredRole {
    /// Matched over the pair, not on `self` alone, so a new [`Role`] fails to
    /// compile here until it is given an explicit answer.
    pub fn is_satisfied_by(self, granted: Role) -> bool {
        match (self, granted) {
            // Any member of the bucket can read.
            (RequiredRole::Reader, Role::Reader | Role::Writer | Role::Admin) => true,
            (RequiredRole::Writer, Role::Writer | Role::Admin) => true,
            (RequiredRole::Writer, Role::Reader) => false,
            (RequiredRole::Admin, Role::Admin) => true,
            (RequiredRole::Admin, Role::Reader | Role::Writer) => false,
        }
    }
}

/// Cached membership entry for a bucket.
#[derive(Debug, Clone)]
struct CachedMembership {
    members: Vec<Member>,
    fetched_at: Instant,
}

impl CachedMembership {
    fn new(members: Vec<Member>) -> Self {
        Self {
            members,
            fetched_at: Instant::now(),
        }
    }

    /// How long ago this entry was fetched.
    fn age(&self) -> Duration {
        self.fetched_at.elapsed()
    }

    fn is_fresh(&self, ttl: Duration) -> bool {
        self.age() < ttl
    }

    fn role_of(&self, account: &AccountId32) -> Option<Role> {
        self.members
            .iter()
            .find(|m| &m.account == account)
            .map(|m| m.role)
    }
}

/// Trait for resolving bucket membership (enables mocking in tests).
#[async_trait::async_trait]
pub trait MembershipResolver: Send + Sync {
    /// Every member of `bucket_id`. An empty set means nobody is a member —
    /// either the bucket does not exist, or it holds no members.
    async fn fetch_members(&self, bucket_id: BucketId) -> Result<Vec<Member>, MembershipError>;
}

/// So a boxed resolver still satisfies the `impl MembershipResolver` bound.
#[async_trait::async_trait]
impl<T: MembershipResolver + ?Sized> MembershipResolver for Box<T> {
    async fn fetch_members(&self, bucket_id: BucketId) -> Result<Vec<Member>, MembershipError> {
        (**self).fetch_members(bucket_id).await
    }
}

/// A [`MembershipResolver`] that returns a fixed member set for every bucket.
/// Used by integration tests across crates.
pub struct StaticMembershipResolver(pub Vec<Member>);

#[async_trait::async_trait]
impl MembershipResolver for StaticMembershipResolver {
    async fn fetch_members(&self, _bucket_id: BucketId) -> Result<Vec<Member>, MembershipError> {
        Ok(self.0.clone())
    }
}

/// What a drain of a [`MembershipInvalidations`] feed found.
pub enum Invalidation {
    /// Nothing changed since the last drain — the common case, allocation-free.
    None,
    /// Exactly these buckets changed.
    Buckets(Vec<BucketId>),
    /// *Which* buckets changed is unknown: the feed lagged or restarted, so
    /// every cached member set must be dropped.
    All,
}

/// Feed telling the cache that bucket membership changed, drained before every
/// lookup.
///
/// Abstracted exactly like [`MembershipResolver`] and for the same reason: the
/// cache needs to know *that* a member set changed, not how that news travels,
/// so this crate needs no chain dependency. Draining is synchronous — it runs
/// on the authorization path, before any `.await`.
pub trait MembershipInvalidations: Send + Sync {
    /// Every invalidation observed since the last call. Must not block.
    fn drain(&self) -> Invalidation;
}

/// A feed that never invalidates anything, leaving the TTL as the only bound.
/// The default a cache is built with until [`MembershipCache::with_invalidations`]
/// supplies a real one.
struct NoInvalidations;

impl MembershipInvalidations for NoInvalidations {
    fn drain(&self) -> Invalidation {
        Invalidation::None
    }
}

/// TTL cache in front of a [`MembershipResolver`].
pub(crate) struct MembershipCache {
    cache: DashMap<BucketId, CachedMembership>,
    /// How many invalidations this cache has processed. A lookup records it
    /// before calling the resolver and re-checks it, under the entry's shard
    /// lock, before persisting: if it moved, an invalidation landed while the
    /// fetch was in flight and that result is already stale. Without it a
    /// fetch started just before a revocation could write the pre-revocation
    /// members back *after* the invalidation ran, resurrecting revoked access
    /// for a full TTL.
    ///
    /// One counter for the whole cache, not one per bucket: invalidating
    /// bucket B therefore also discards an in-flight fetch of unrelated
    /// bucket A. That costs A's next request a re-resolve and can never
    /// produce a wrong answer, which is the right trade for zero per-bucket
    /// storage.
    epoch: AtomicU64,
    /// How long an entry is served before the chain is rechecked.
    ttl: Duration,
    /// Explicit stale-if-error ceiling. `None` derives one from `ttl`, resolved
    /// on read so the builders can be called in any order.
    max_stale: Option<Duration>,
    resolver: Box<dyn MembershipResolver>,
    invalidations: Box<dyn MembershipInvalidations>,
}

/// TTL for callers that don't set one. Matches `--auth-cache-ttl`'s default.
const DEFAULT_TTL: Duration = Duration::from_secs(30);

/// Derives the stale-if-error ceiling from the TTL when none is set, keeping it
/// proportional to how fresh the caller wanted membership in the first place.
const MAX_STALE_TTL_MULTIPLE: u32 = 10;

impl MembershipCache {
    pub(crate) fn new(resolver: impl MembershipResolver + 'static) -> Self {
        Self {
            cache: DashMap::new(),
            epoch: AtomicU64::new(0),
            ttl: DEFAULT_TTL,
            max_stale: None,
            resolver: Box::new(resolver),
            invalidations: Box::new(NoInvalidations),
        }
    }

    /// Replace the invalidation feed. Without one, the TTL is the only bound.
    pub(crate) fn with_invalidations(
        mut self,
        feed: impl MembershipInvalidations + 'static,
    ) -> Self {
        self.invalidations = Box::new(feed);
        self
    }

    /// How long a resolved member set is served before the chain is rechecked.
    pub(crate) fn with_ttl(mut self, ttl: Duration) -> Self {
        self.ttl = ttl;
        self
    }

    /// Override the ceiling otherwise derived from the TTL.
    pub(crate) fn with_max_stale(mut self, max_stale: Duration) -> Self {
        self.max_stale = Some(max_stale);
        self
    }

    fn max_stale(&self) -> Duration {
        self.max_stale
            .unwrap_or_else(|| self.ttl.saturating_mul(MAX_STALE_TTL_MULTIPLE))
    }

    /// Apply everything the feed has seen since the last lookup. Runs before
    /// the cache is consulted, so a change broadcast before this request
    /// arrived is honoured by *this* request rather than the next one.
    fn drain_invalidations(&self) {
        match self.invalidations.drain() {
            Invalidation::None => {}
            Invalidation::Buckets(ids) => ids.into_iter().for_each(|id| self.invalidate(id)),
            Invalidation::All => self.invalidate_all(),
        }
    }

    /// Look up a caller's role in a bucket.
    /// Returns None if the caller is not a member.
    pub(crate) async fn get_role(
        &self,
        bucket_id: BucketId,
        account: &AccountId32,
    ) -> Result<Option<Role>, MembershipError> {
        self.drain_invalidations();

        if let Some(entry) = self.cache.get(&bucket_id) {
            if entry.is_fresh(self.ttl) {
                return Ok(entry.role_of(account));
            }
        } // guard dropped here, before the await below

        // Recorded before the await, so the persist below can tell whether an
        // invalidation landed while the resolver call was in flight.
        let epoch_before = self.epoch.load(Ordering::SeqCst);

        // Cache miss or stale — fetch from chain
        match self.resolver.fetch_members(bucket_id).await {
            Ok(members) => {
                let entry = CachedMembership::new(members);
                let role = entry.role_of(account);
                // `entry()` first: its shard lock has to already be held when
                // the epoch is read, or the check and the write can be split
                // by an invalidation. Swapping these two lines reopens the
                // race — see [`Self::epoch`].
                let slot = self.cache.entry(bucket_id);
                if self.epoch.load(Ordering::SeqCst) == epoch_before {
                    slot.insert(entry);
                }
                Ok(role)
            }
            Err(e) => {
                // stale-if-error, not stale-while-revalidate: stale data is
                // served only because a refetch failed, and only up to
                // max_stale - never merely because the TTL lapsed.
                if let Some(entry) = self.cache.get(&bucket_id) {
                    let (age, max_stale) = (entry.age(), self.max_stale());
                    if age <= max_stale {
                        tracing::warn!(
                            "Chain unreachable for bucket {} membership (age {:?}), serving stale data: {}",
                            bucket_id,
                            age,
                            e
                        );
                        return Ok(entry.role_of(account));
                    }
                    tracing::error!(
                        "Cached membership for bucket {} is {:?} old (max_stale {:?}); refusing request: {}",
                        bucket_id,
                        age,
                        max_stale,
                        e
                    );
                }
                Err(e)
            }
        }
    }

    /// Drop the cached membership for `bucket_id`, if any, so the next lookup
    /// re-resolves it.
    ///
    /// The epoch is bumped *before* the removal, not after: `get_role`
    /// re-reads it from inside `cache.entry(bucket_id)`'s shard lock, so
    /// bumping first leaves only two safe orderings — either that lookup sees
    /// the new epoch and discards its result, or it wins the lock first and
    /// this removal then deletes what it just wrote. Bumping afterwards would
    /// open a window where a lookup reads the old epoch and writes after the
    /// removal has already run, resurrecting the invalidated membership.
    fn invalidate(&self, bucket_id: BucketId) {
        self.epoch.fetch_add(1, Ordering::SeqCst);
        self.cache.remove(&bucket_id);
    }

    /// Drop every cached membership. `clear()` takes one shard lock at a time
    /// rather than all at once, but a bucket lives in exactly one shard, so
    /// the same bump-before-lock ordering that makes [`Self::invalidate`]
    /// safe makes this safe too.
    fn invalidate_all(&self) {
        self.epoch.fetch_add(1, Ordering::SeqCst);
        self.cache.clear();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn privilege_ladder_is_exhaustive() {
        // Every (required, granted) pair, so the ladder cannot be widened
        // without a test failing.
        for (required, granted, expected) in [
            (RequiredRole::Reader, Role::Reader, true),
            (RequiredRole::Reader, Role::Writer, true),
            (RequiredRole::Reader, Role::Admin, true),
            (RequiredRole::Writer, Role::Reader, false),
            (RequiredRole::Writer, Role::Writer, true),
            (RequiredRole::Writer, Role::Admin, true),
            (RequiredRole::Admin, Role::Reader, false),
            (RequiredRole::Admin, Role::Writer, false),
            (RequiredRole::Admin, Role::Admin, true),
        ] {
            assert_eq!(
                required.is_satisfied_by(granted),
                expected,
                "{granted:?} vs required {required:?}"
            );
        }
    }

    #[test]
    fn role_of_finds_each_member_and_rejects_strangers() {
        let alice = AccountId32::new([1u8; 32]);
        let bob = AccountId32::new([2u8; 32]);
        let charlie = AccountId32::new([3u8; 32]);

        let entry = CachedMembership::new(vec![
            (alice.clone(), Role::Admin).into(),
            (bob.clone(), Role::Writer).into(),
            (charlie.clone(), Role::Reader).into(),
        ]);

        assert_eq!(entry.role_of(&alice), Some(Role::Admin));
        assert_eq!(entry.role_of(&bob), Some(Role::Writer));
        assert_eq!(entry.role_of(&charlie), Some(Role::Reader));

        let unknown = AccountId32::new([4u8; 32]);
        assert_eq!(entry.role_of(&unknown), None);
    }

    #[test]
    fn entries_go_stale_once_the_ttl_elapses() {
        let entry = CachedMembership::new(vec![]);
        assert!(entry.is_fresh(Duration::from_secs(60)));
        assert!(!entry.is_fresh(Duration::ZERO));
    }

    // ── MembershipInvalidations ────────────────────────────────────────────

    use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
    use std::sync::{Arc, Mutex};

    /// Resolver that grants `account` an `Admin` role on every bucket and
    /// counts how many times it was actually called, so a test can tell a
    /// cache hit from a re-resolve.
    struct CountingResolver {
        account: AccountId32,
        calls: Arc<AtomicUsize>,
    }

    #[async_trait::async_trait]
    impl MembershipResolver for CountingResolver {
        async fn fetch_members(
            &self,
            _bucket_id: BucketId,
        ) -> Result<Vec<Member>, MembershipError> {
            self.calls.fetch_add(1, Ordering::SeqCst);
            Ok(vec![(self.account.clone(), Role::Admin).into()])
        }
    }

    /// Resolver that succeeds once, then fails every call after - so a test
    /// can seed a fresh cache entry and then force `get_role`'s error arm on
    /// the very next lookup.
    struct FlakyResolver {
        account: AccountId32,
        calls: Arc<AtomicUsize>,
    }

    #[async_trait::async_trait]
    impl MembershipResolver for FlakyResolver {
        async fn fetch_members(
            &self,
            _bucket_id: BucketId,
        ) -> Result<Vec<Member>, MembershipError> {
            let call = self.calls.fetch_add(1, Ordering::SeqCst) + 1;
            if call == 1 {
                Ok(vec![(self.account.clone(), Role::Admin).into()])
            } else {
                Err(MembershipError::Unavailable("chain down".to_string()))
            }
        }
    }

    /// A [`MembershipInvalidations`] a test can load with one value to return
    /// on the next [`drain`](MembershipInvalidations::drain), simulating an
    /// event arriving between two lookups.
    #[derive(Clone)]
    struct QueuedInvalidations(Arc<Mutex<Option<Invalidation>>>);

    impl QueuedInvalidations {
        fn new() -> Self {
            Self(Arc::new(Mutex::new(None)))
        }

        fn queue(&self, invalidation: Invalidation) {
            *self.0.lock().unwrap() = Some(invalidation);
        }
    }

    impl MembershipInvalidations for QueuedInvalidations {
        fn drain(&self) -> Invalidation {
            self.0.lock().unwrap().take().unwrap_or(Invalidation::None)
        }
    }

    #[test]
    fn invalidating_a_never_cached_bucket_leaves_the_map_empty() {
        // `parse_membership_changes` filters chain-wide membership events by
        // pallet and event name only, with no is-this-our-bucket predicate
        // (provider-node/src/chain_state_coordinator.rs), so `invalidate` is
        // routinely called for buckets this cache has never seen. It must be
        // a no-op on the map, not a slot-creating write - otherwise every
        // chain-wide membership event permanently grows the map by one entry
        // for a bucket that will never be looked up.
        let cache = MembershipCache::new(StaticMembershipResolver(vec![]))
            .with_ttl(Duration::from_secs(300));

        cache.invalidate(42);

        assert!(
            cache.cache.is_empty(),
            "invalidating a bucket that was never cached must not create an entry for it"
        );
    }

    #[tokio::test]
    async fn invalidation_applies_to_the_request_that_drains_it() {
        let account = AccountId32::new([3u8; 32]);
        let calls = Arc::new(AtomicUsize::new(0));
        let feed = QueuedInvalidations::new();
        let cache = MembershipCache::new(CountingResolver {
            account: account.clone(),
            calls: calls.clone(),
        })
        .with_ttl(Duration::from_secs(300))
        .with_invalidations(feed.clone());

        // Populate a fresh cache entry.
        cache.get_role(1, &account).await.unwrap();
        assert_eq!(calls.load(Ordering::SeqCst), 1);

        // An event for this bucket arrives after the entry was cached, but
        // before the next lookup — nothing has drained it yet.
        feed.queue(Invalidation::Buckets(vec![1]));

        // The very next lookup must itself honour the event, not the one
        // after: the drain runs before the freshness check, so a change
        // broadcast before a request arrives is applied to that request.
        cache.get_role(1, &account).await.unwrap();
        assert_eq!(
            calls.load(Ordering::SeqCst),
            2,
            "the request that drains the event must itself re-resolve"
        );
    }

    /// Resolver that blocks inside `fetch_members` until told to proceed, so a
    /// test can land an invalidation while a fetch is in flight.
    struct GatedResolver {
        account: AccountId32,
        calls: Arc<AtomicUsize>,
        proceed: Arc<AtomicBool>,
    }

    #[async_trait::async_trait]
    impl MembershipResolver for GatedResolver {
        async fn fetch_members(
            &self,
            _bucket_id: BucketId,
        ) -> Result<Vec<Member>, MembershipError> {
            self.calls.fetch_add(1, Ordering::SeqCst);
            while !self.proceed.load(Ordering::SeqCst) {
                tokio::task::yield_now().await;
            }
            Ok(vec![(self.account.clone(), Role::Admin).into()])
        }
    }

    #[tokio::test]
    async fn invalidate_all_discards_an_in_flight_refetch_of_a_cached_bucket() {
        // `invalidate_all`'s job is to stop an already-cached entry's in-flight
        // refetch from resurrecting pre-invalidation data — the same guarantee
        // `invalidate` gives a single bucket, applied to every cached bucket at
        // once.
        let account = AccountId32::new([9u8; 32]);
        let calls = Arc::new(AtomicUsize::new(0));
        let proceed = Arc::new(AtomicBool::new(true));
        // Zero TTL: every lookup treats the cached entry as stale and refetches,
        // so the second lookup below is a genuine in-flight race rather than a
        // cache hit that never touches the resolver.
        let cache = Arc::new(
            MembershipCache::new(GatedResolver {
                account: account.clone(),
                calls: calls.clone(),
                proceed: proceed.clone(),
            })
            .with_ttl(Duration::ZERO),
        );

        // Seed an existing, cached entry for bucket 1.
        cache.get_role(1, &account).await.unwrap();
        assert_eq!(calls.load(Ordering::SeqCst), 1);

        // The next lookup must refetch (TTL is zero) and this time blocks
        // inside `fetch_members` until `proceed` flips.
        proceed.store(false, Ordering::SeqCst);
        let in_flight = {
            let cache = cache.clone();
            let account = account.clone();
            tokio::spawn(async move { cache.get_role(1, &account).await })
        };

        // Wait until that refetch has actually started before invalidating,
        // so the race is real rather than accidental ordering.
        while calls.load(Ordering::SeqCst) == 1 {
            tokio::task::yield_now().await;
        }

        // An invalidation lands while that refetch is still in flight.
        cache.invalidate_all();

        // Let the refetch complete.
        proceed.store(true, Ordering::SeqCst);
        let role = in_flight.await.unwrap().unwrap();
        assert_eq!(role, Some(Role::Admin));

        // The in-flight refetch's result must not have been persisted: the
        // entry is gone entirely, not just blanked, so nothing is left to
        // resurrect the pre-invalidation membership it fetched.
        assert!(
            cache.cache.get(&1).is_none(),
            "invalidate_all landing mid-refetch must discard that refetch's result"
        );

        // Proof that goes beyond internals: with nothing cached, the next
        // lookup must hit the resolver again rather than reading a
        // resurrected membership back out.
        cache.get_role(1, &account).await.unwrap();
        assert_eq!(
            calls.load(Ordering::SeqCst),
            3,
            "no entry should remain cached after the discarded refetch, so this lookup must re-resolve"
        );
    }

    #[tokio::test]
    async fn invalidate_discards_an_in_flight_refetch_of_a_cached_bucket() {
        // Same race as `invalidate_all`'s test above, but through the
        // per-bucket `invalidate` path: a targeted invalidation must guard an
        // in-flight refetch exactly as a wholesale one does.
        let account = AccountId32::new([10u8; 32]);
        let calls = Arc::new(AtomicUsize::new(0));
        let proceed = Arc::new(AtomicBool::new(true));
        let cache = Arc::new(
            MembershipCache::new(GatedResolver {
                account: account.clone(),
                calls: calls.clone(),
                proceed: proceed.clone(),
            })
            .with_ttl(Duration::ZERO),
        );

        cache.get_role(1, &account).await.unwrap();
        assert_eq!(calls.load(Ordering::SeqCst), 1);

        proceed.store(false, Ordering::SeqCst);
        let in_flight = {
            let cache = cache.clone();
            let account = account.clone();
            tokio::spawn(async move { cache.get_role(1, &account).await })
        };

        while calls.load(Ordering::SeqCst) == 1 {
            tokio::task::yield_now().await;
        }

        // A targeted invalidation for the same bucket lands mid-refetch.
        cache.invalidate(1);

        proceed.store(true, Ordering::SeqCst);
        let role = in_flight.await.unwrap().unwrap();
        assert_eq!(role, Some(Role::Admin));

        assert!(
            cache.cache.get(&1).is_none(),
            "invalidate landing mid-refetch must discard that refetch's result"
        );

        cache.get_role(1, &account).await.unwrap();
        assert_eq!(
            calls.load(Ordering::SeqCst),
            3,
            "no entry should remain cached after the discarded refetch, so this lookup must re-resolve"
        );
    }

    #[tokio::test]
    async fn invalidate_discards_an_in_flight_fetch_of_a_bucket_that_was_never_cached() {
        // The case a presence-only guard cannot cover: nothing was ever
        // cached for this bucket, so a vacant entry after the fetch cannot be
        // told apart from an ordinary first-time miss unless something else
        // records that an invalidation happened while the fetch was in
        // flight. A long TTL rules out the alternative explanation that the
        // entry simply went stale on its own.
        let account = AccountId32::new([11u8; 32]);
        let calls = Arc::new(AtomicUsize::new(0));
        let proceed = Arc::new(AtomicBool::new(false));
        let cache = Arc::new(
            MembershipCache::new(GatedResolver {
                account: account.clone(),
                calls: calls.clone(),
                proceed: proceed.clone(),
            })
            .with_ttl(Duration::from_secs(300)),
        );

        // No seed lookup: bucket 1 has never been cached.
        let in_flight = {
            let cache = cache.clone();
            let account = account.clone();
            tokio::spawn(async move { cache.get_role(1, &account).await })
        };

        // Wait until the first-ever fetch has actually started.
        while calls.load(Ordering::SeqCst) == 0 {
            tokio::task::yield_now().await;
        }

        // An invalidation for this bucket lands while its first-ever fetch is
        // still in flight.
        cache.invalidate(1);

        proceed.store(true, Ordering::SeqCst);
        let role = in_flight.await.unwrap().unwrap();
        assert_eq!(role, Some(Role::Admin));

        assert!(
            cache.cache.get(&1).is_none(),
            "an invalidation landing mid-fetch of a never-cached bucket must still discard that fetch's result"
        );

        cache.get_role(1, &account).await.unwrap();
        assert_eq!(
            calls.load(Ordering::SeqCst),
            2,
            "the discarded first-ever fetch must not have been cached, so this lookup must re-resolve"
        );
    }

    // ── max_stale (stale-if-error bound, issue #347) ────────────────────────

    #[tokio::test]
    async fn stale_membership_within_max_stale_is_still_served() {
        // Today's baseline: when the chain fails, a cached entry younger
        // than `max_stale` is still served. Must not regress once the error
        // arm starts consulting the bound - only the past-the-bound case
        // (below) should start refusing.
        let account = AccountId32::new([20u8; 32]);
        let calls = Arc::new(AtomicUsize::new(0));
        let cache = MembershipCache::new(FlakyResolver {
            account: account.clone(),
            calls: calls.clone(),
        })
        .with_ttl(Duration::ZERO) // force every lookup to attempt a refetch
        .with_max_stale(Duration::from_secs(300)); // generously within bound

        // Seed a cached entry via the resolver's one successful call.
        let role = cache.get_role(1, &account).await.unwrap();
        assert_eq!(role, Some(Role::Admin));
        assert_eq!(calls.load(Ordering::SeqCst), 1);

        // Second lookup: TTL is zero, so this attempts a refetch, which
        // fails. The cached entry is a moment old - comfortably within
        // max_stale - so it must still be served.
        let role = cache.get_role(1, &account).await.unwrap();
        assert_eq!(
            role,
            Some(Role::Admin),
            "an entry within max_stale must still be served when the chain fails"
        );
        assert_eq!(calls.load(Ordering::SeqCst), 2);
    }

    #[tokio::test]
    async fn stale_membership_past_max_stale_is_refused() {
        let account = AccountId32::new([21u8; 32]);
        let calls = Arc::new(AtomicUsize::new(0));
        let cache = MembershipCache::new(FlakyResolver {
            account: account.clone(),
            calls: calls.clone(),
        })
        .with_ttl(Duration::ZERO)
        .with_max_stale(Duration::ZERO); // any age at all exceeds this

        let role = cache.get_role(1, &account).await.unwrap();
        assert_eq!(role, Some(Role::Admin));
        assert_eq!(calls.load(Ordering::SeqCst), 1);

        // Second lookup refetches (TTL zero), the refetch fails, and the
        // cached entry is already older than max_stale (zero) by the time
        // it's consulted - the request must be refused rather than served
        // stale.
        let result = cache.get_role(1, &account).await;
        assert!(
            matches!(result, Err(MembershipError::Unavailable(_))),
            "an entry older than max_stale must not be served on chain failure, got {result:?}"
        );
        assert_eq!(calls.load(Ordering::SeqCst), 2);

        // Refused, not evicted: the entry must still be in the map so a
        // future successful refetch can overwrite it, rather than forcing a
        // cold miss on top of an already-unavailable chain.
        assert!(
            cache.cache.get(&1).is_some(),
            "a refused entry must be left in the map, not evicted"
        );
    }
}
