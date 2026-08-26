// SPDX-License-Identifier: Apache-2.0

//! Bucket membership and the role ladder. Resolution is left to
//! [`MembershipResolver`] and change notification to [`MembershipInvalidations`],
//! so this crate needs no chain dependency.

use crate::error::MembershipError;
use moka::future::{Cache, CacheBuilder};
use moka::Expiry;
use sp_core::crypto::AccountId32;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
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
#[derive(Debug, Clone, PartialEq, Eq)]
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

/// How long a resident entry survives, measured from its most recent fetch —
/// so a bucket refreshed on every lookup keeps its full residency instead of
/// counting down from whenever it first entered the cache.
///
/// An entry is dropped once nothing can read it again: a non-empty set at
/// `max_stale`, past which it is too old to be fresh and too old to serve on
/// error; an empty one at `ttl`, since a refetch is due then anyway and an
/// empty set only ever denies, so it needs no stale-if-error grace.
struct MembershipExpiry {
    ttl: Duration,
    max_stale: Duration,
}

impl MembershipExpiry {
    fn residency(&self, value: &Arc<CachedMembership>) -> Duration {
        if value.members.is_empty() {
            self.ttl
        } else {
            self.max_stale
        }
    }
}

impl Expiry<BucketId, Arc<CachedMembership>> for MembershipExpiry {
    fn expire_after_create(
        &self,
        _key: &BucketId,
        value: &Arc<CachedMembership>,
        _created_at: Instant,
    ) -> Option<Duration> {
        Some(self.residency(value))
    }

    // Without this, moka's default keeps a refreshed entry's *original*
    // expiration instant (see `Expiry::expire_after_update`'s default:
    // `duration_until_expiry`, unchanged) - so a bucket looked up more than
    // once would count residency down from its first insert while
    // `CachedMembership::fetched_at` resets on every insert, and the two
    // clocks would silently diverge.
    fn expire_after_update(
        &self,
        _key: &BucketId,
        value: &Arc<CachedMembership>,
        _updated_at: Instant,
        _duration_until_expiry: Option<Duration>,
    ) -> Option<Duration> {
        Some(self.residency(value))
    }
}

/// TTL cache in front of a [`MembershipResolver`].
pub(crate) struct MembershipCache {
    /// Bounded by `max_entries`, with per-entry expiry from
    /// [`MembershipExpiry`]. Values are `Arc`-wrapped because moka clones the
    /// value on every `get` - without it, a cache hit would clone the whole
    /// member list on the auth hot path.
    cache: Cache<BucketId, Arc<CachedMembership>>,
    /// Invalidations processed so far. A lookup samples it before resolving and
    /// re-checks it after persisting: if it moved, the fetch raced an
    /// invalidation and the insert is undone rather than resurrecting revoked
    /// access for a full TTL.
    ///
    /// It is cache-wide, so invalidating one bucket also discards in-flight
    /// fetches for others - an extra re-resolve, never a wrong answer.
    epoch: AtomicU64,
    /// How long an entry is served before the chain is rechecked. Also bounds
    /// an empty entry's residency - see [`MembershipExpiry`].
    ttl: Duration,
    /// Stale-if-error ceiling: how old a cached entry may be and still be
    /// served when a refetch fails. Also bounds a non-empty entry's
    /// residency - see [`MembershipExpiry`].
    max_stale: Duration,
    /// Upper bound on resident entries, so an attacker walking bucket ids
    /// cannot grow the cache without limit.
    max_entries: u64,
    resolver: Box<dyn MembershipResolver>,
    invalidations: Box<dyn MembershipInvalidations>,
}

/// TTL for callers that don't set one. Matches `--auth-cache-ttl`'s default.
const DEFAULT_TTL: Duration = Duration::from_secs(30);

/// Stale-if-error ceiling for callers that don't set one. Matches
/// `--auth-max-stale`'s default.
const DEFAULT_MAX_STALE: Duration = Duration::from_secs(300);

/// Entry-count ceiling for callers that don't set one. Matches
/// `--auth-cache-max-entries`'s default.
const DEFAULT_MAX_ENTRIES: u64 = 10_000;

/// Rebuilt by `new()` and by every `with_*` that affects residency, since all
/// three are baked into the `Cache` at construction. Callers chain the
/// builders before the first lookup, so rebuilding an empty cache is free.
fn build_cache(
    max_entries: u64,
    ttl: Duration,
    max_stale: Duration,
) -> Cache<BucketId, Arc<CachedMembership>> {
    CacheBuilder::new(max_entries)
        .expire_after(MembershipExpiry { ttl, max_stale })
        .build()
}

impl MembershipCache {
    pub(crate) fn new(resolver: impl MembershipResolver + 'static) -> Self {
        Self {
            cache: build_cache(DEFAULT_MAX_ENTRIES, DEFAULT_TTL, DEFAULT_MAX_STALE),
            epoch: AtomicU64::new(0),
            ttl: DEFAULT_TTL,
            max_stale: DEFAULT_MAX_STALE,
            max_entries: DEFAULT_MAX_ENTRIES,
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
    /// Defaults to 30 seconds.
    pub(crate) fn with_ttl(mut self, ttl: Duration) -> Self {
        self.ttl = ttl;
        self.cache = build_cache(self.max_entries, self.ttl, self.max_stale);
        self
    }

    /// Override the default stale-if-error ceiling. Defaults to 5 minutes.
    pub(crate) fn with_max_stale(mut self, max_stale: Duration) -> Self {
        self.max_stale = max_stale;
        self.cache = build_cache(self.max_entries, self.ttl, self.max_stale);
        self
    }

    /// Override the default entry-count ceiling. Defaults to 10,000.
    pub(crate) fn with_max_entries(mut self, max_entries: u64) -> Self {
        self.max_entries = max_entries;
        self.cache = build_cache(self.max_entries, self.ttl, self.max_stale);
        self
    }

    /// Apply everything the feed has seen since the last lookup. Runs before
    /// the cache is consulted, so a change broadcast before this request
    /// arrived is honoured by *this* request rather than the next one.
    async fn drain_invalidations(&self) {
        match self.invalidations.drain() {
            Invalidation::None => {}
            Invalidation::Buckets(ids) => {
                for id in ids {
                    self.invalidate(id).await;
                }
            }
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
        self.drain_invalidations().await;

        if let Some(entry) = self.cache.get(&bucket_id).await {
            if entry.is_fresh(self.ttl) {
                return Ok(entry.role_of(account));
            }
        }

        // Recorded before the await, so the persist below can tell whether an
        // invalidation landed while the resolver call was in flight.
        let epoch_before = self.epoch.load(Ordering::SeqCst);

        // Cache miss or stale — fetch from chain
        match self.resolver.fetch_members(bucket_id).await {
            Ok(members) => {
                let entry = Arc::new(CachedMembership::new(members));
                let role = entry.role_of(account);
                // Cache it only if no invalidation landed during the fetch,
                // then check again: no lock spans the insert, so a bump can
                // still slip between the two lines. `invalidate` bumps
                // *before* removing and this reads *after* inserting, so of
                // the two, at least one always fires - either its removal
                // takes our entry, or we see its bump and undo ourselves.
                // Checking only once, either side, leaves the other ordering
                // free to resurrect the revoked member set for a full
                // `max_stale`.
                if self.epoch.load(Ordering::SeqCst) == epoch_before {
                    self.cache.insert(bucket_id, entry).await;
                    if self.epoch.load(Ordering::SeqCst) != epoch_before {
                        self.cache.invalidate(&bucket_id).await;
                    }
                }
                Ok(role)
            }
            Err(e) => {
                // Drain again: a change for this bucket may have arrived during
                // the refetch, and a member set already known to be outdated
                // must not be served. Draining rather than comparing the epoch,
                // because the epoch is cache-wide - an unrelated bucket's change
                // keeps this one's stale-if-error grace.
                self.drain_invalidations().await;

                // stale-if-error, not stale-while-revalidate: stale data is
                // served only because a refetch failed, never merely because
                // the TTL lapsed. `max_stale` itself has exactly one owner,
                // `MembershipExpiry`: a resident entry is by construction no
                // older than `max_stale` (non-empty) or `ttl` (empty), so
                // reaching this point with an entry still in the cache and
                // refusing once it is gone are the same event, not two
                // separately-judged outcomes.
                if let Some(entry) = self.cache.get(&bucket_id).await {
                    tracing::warn!(
                        "Chain unreachable for bucket {} membership (age {:?}), serving stale data: {}",
                        bucket_id,
                        entry.age(),
                        e
                    );
                    return Ok(entry.role_of(account));
                }
                Err(e)
            }
        }
    }

    /// Drop the cached membership for `bucket_id`, if any, so the next lookup
    /// re-resolves it.
    ///
    /// The epoch is bumped *before* the removal, not after: `get_role`
    /// re-reads it *after* its own insert, so bumping first leaves only two
    /// safe orderings — either that insert lands first and this removal then
    /// deletes what it just wrote, or this removal runs first and that
    /// lookup's own re-read sees the new epoch and undoes its insert. Bumping
    /// afterwards would open a window where a lookup reads the old epoch,
    /// inserts, and re-reads before this removal has run, resurrecting the
    /// invalidated membership.
    async fn invalidate(&self, bucket_id: BucketId) {
        self.epoch.fetch_add(1, Ordering::SeqCst);
        self.cache.invalidate(&bucket_id).await;
    }

    /// Drop every cached membership. `moka`'s `invalidate_all` is O(1) - it
    /// bumps an internal cutoff rather than walking every entry - so unlike
    /// the old `DashMap::clear()` this is cheap on the request path
    /// regardless of how many buckets are resident.
    fn invalidate_all(&self) {
        self.epoch.fetch_add(1, Ordering::SeqCst);
        self.cache.invalidate_all();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::{CountingResolver, FlakyResolver, Gated, QueuedInvalidations};

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
    use std::sync::Arc;

    #[tokio::test]
    async fn invalidating_a_never_cached_bucket_leaves_the_map_empty() {
        // `parse_membership_changes` filters chain-wide membership events by
        // pallet and event name only, with no is-this-our-bucket predicate
        // (provider-node/src/chain_state_coordinator.rs), so `invalidate` is
        // routinely called for buckets this cache has never seen. It must be
        // a no-op on the map, not a slot-creating write - otherwise every
        // chain-wide membership event permanently grows the map by one entry
        // for a bucket that will never be looked up.
        let cache = MembershipCache::new(StaticMembershipResolver(vec![]))
            .with_ttl(Duration::from_secs(300));

        cache.invalidate(42).await;

        // `entry_count()` is eventually consistent - settle it first so this
        // assertion reflects the invalidate above, not a stale count.
        cache.cache.run_pending_tasks().await;
        assert_eq!(
            cache.cache.entry_count(),
            0,
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
            MembershipCache::new(Gated {
                inner: CountingResolver {
                    account: account.clone(),
                    calls: calls.clone(),
                },
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
            cache.cache.get(&1).await.is_none(),
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
            MembershipCache::new(Gated {
                inner: CountingResolver {
                    account: account.clone(),
                    calls: calls.clone(),
                },
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
        cache.invalidate(1).await;

        proceed.store(true, Ordering::SeqCst);
        let role = in_flight.await.unwrap().unwrap();
        assert_eq!(role, Some(Role::Admin));

        assert!(
            cache.cache.get(&1).await.is_none(),
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
            MembershipCache::new(Gated {
                inner: CountingResolver {
                    account: account.clone(),
                    calls: calls.clone(),
                },
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
        cache.invalidate(1).await;

        proceed.store(true, Ordering::SeqCst);
        let role = in_flight.await.unwrap().unwrap();
        assert_eq!(role, Some(Role::Admin));

        assert!(
            cache.cache.get(&1).await.is_none(),
            "an invalidation landing mid-fetch of a never-cached bucket must still discard that fetch's result"
        );

        cache.get_role(1, &account).await.unwrap();
        assert_eq!(
            calls.load(Ordering::SeqCst),
            2,
            "the discarded first-ever fetch must not have been cached, so this lookup must re-resolve"
        );
    }

    #[tokio::test]
    async fn an_unrelated_invalidation_still_allows_serving_stale() {
        // The other side of the bound: bucket 2's member set changing says
        // nothing about bucket 1, so it must not cost bucket 1 its
        // stale-if-error grace. Guards against reaching for the cache-wide
        // epoch here - that would refuse on exactly this case while still
        // missing the one above, since an undrained event bumps no epoch.
        let account = AccountId32::new([30u8; 32]);
        let calls = Arc::new(AtomicUsize::new(0));
        let proceed = Arc::new(AtomicBool::new(true));
        let cache = Arc::new(
            MembershipCache::new(Gated {
                inner: FlakyResolver {
                    account: account.clone(),
                    calls: calls.clone(),
                },
                proceed: proceed.clone(),
            })
            .with_ttl(Duration::ZERO)
            .with_max_stale(Duration::from_secs(300)),
        );

        // Seed bucket 1.
        cache.get_role(1, &account).await.unwrap();
        assert_eq!(calls.load(Ordering::SeqCst), 1);

        // The gate holds every call, including the seed above - close it now so
        // the refetch below blocks instead.
        proceed.store(false, Ordering::SeqCst);

        // Second lookup refetches (TTL zero) and blocks before failing.
        let in_flight = {
            let cache = cache.clone();
            let account = account.clone();
            tokio::spawn(async move { cache.get_role(1, &account).await })
        };

        while calls.load(Ordering::SeqCst) == 1 {
            tokio::task::yield_now().await;
        }

        // An unrelated bucket is invalidated while bucket 1's refetch is in
        // flight, bumping the cache-wide epoch without touching bucket 1's
        // entry at all.
        cache.invalidate(2).await;

        proceed.store(true, Ordering::SeqCst);
        let result = in_flight.await.unwrap();

        assert_eq!(
            result.expect("an unrelated bucket's change must not refuse this one"),
            Some(Role::Admin),
            "bucket 1's cached role is still within max_stale and nothing said it changed"
        );
    }

    #[tokio::test]
    async fn an_undrained_event_for_this_bucket_is_not_served_stale() {
        // The window: an event for *this* bucket arrives after this
        // request's own drain, and no concurrent request drains it either. The cached member set is known-outdated by the
        // time the error arm runs, so it must not be served.
        let account = AccountId32::new([31u8; 32]);
        let calls = Arc::new(AtomicUsize::new(0));
        let proceed = Arc::new(AtomicBool::new(true));
        let feed = QueuedInvalidations::new();
        let cache = Arc::new(
            MembershipCache::new(Gated {
                inner: FlakyResolver {
                    account: account.clone(),
                    calls: calls.clone(),
                },
                proceed: proceed.clone(),
            })
            .with_ttl(Duration::ZERO)
            .with_max_stale(Duration::from_secs(300))
            .with_invalidations(feed.clone()),
        );

        cache.get_role(1, &account).await.unwrap();
        assert_eq!(calls.load(Ordering::SeqCst), 1);

        // The gate holds every call, including the seed above - close it now so
        // the refetch below blocks instead.
        proceed.store(false, Ordering::SeqCst);

        let in_flight = {
            let cache = cache.clone();
            let account = account.clone();
            tokio::spawn(async move { cache.get_role(1, &account).await })
        };

        while calls.load(Ordering::SeqCst) == 1 {
            tokio::task::yield_now().await;
        }

        // Bucket 1's membership changed while its refetch was in flight.
        // Nothing drains this: the in-flight request already drained, and no
        // other request is running.
        feed.queue(Invalidation::Buckets(vec![1]));

        proceed.store(true, Ordering::SeqCst);
        let result = in_flight.await.unwrap();

        assert!(
            matches!(result, Err(MembershipError::Unavailable(_))),
            "a pending event for this very bucket must stop its cached member set \
             being served stale, got {result:?}"
        );
    }

    // ── max_stale (stale-if-error bound) ───────────────────────────────────

    #[tokio::test]
    async fn stale_membership_within_max_stale_is_still_served() {
        // When the chain fails, a cached entry is served stale as long as it
        // is still resident - and residency is exactly `max_stale` (see
        // `MembershipExpiry`), so this covers the "still there" side of that
        // one boundary.
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
        .with_ttl(Duration::ZERO) // force every lookup to attempt a refetch
        .with_max_stale(Duration::from_millis(50));

        let role = cache.get_role(1, &account).await.unwrap();
        assert_eq!(role, Some(Role::Admin));
        assert_eq!(calls.load(Ordering::SeqCst), 1);

        // Age the entry genuinely past max_stale rather than leaning on a
        // zero bound, so this exercises real elapsed time.
        tokio::time::sleep(Duration::from_millis(100)).await;

        // The lookup refetches (TTL zero), the refetch fails, and
        // `MembershipExpiry` has already evicted the entry by this point -
        // there is no cached member set left to fall back on, so the
        // request is refused. Refusal is that eviction, not a separate age
        // check: an age gate separate from the cache's own expiry could
        // disagree with it, so the two are one and the same thing.
        let result = cache.get_role(1, &account).await;
        assert!(
            matches!(result, Err(MembershipError::Unavailable(_))),
            "an entry older than max_stale must not be served on chain failure, got {result:?}"
        );
        assert_eq!(calls.load(Ordering::SeqCst), 2);

        // Residency deliberately not asserted beyond the refusal above: a
        // later successful refetch is unaffected regardless, since `insert`
        // overwrites whatever was there.
    }

    // ── bounding the cache ─────────────────────────────────────────────────
    //
    // The three tests below pin what the bound has to do: cap the entry
    // count, and stop holding an entry once nothing can read it again.

    #[tokio::test]
    async fn cache_size_is_bounded_by_max_entries() {
        // Walking `bucket_id = 1..N` with a self-signed header grows the map
        // by one permanent entry per distinct id - any keypair verifies,
        // membership is what's checked. Once bounded, resident entries must
        // never exceed the configured ceiling.
        let account = AccountId32::new([50u8; 32]);
        let cache = MembershipCache::new(StaticMembershipResolver(vec![(
            account.clone(),
            Role::Reader,
        )
            .into()]))
        .with_ttl(Duration::from_secs(300))
        .with_max_entries(8);

        for bucket_id in 0..64 {
            cache.get_role(bucket_id, &account).await.unwrap();
        }

        // `entry_count()` is eventually consistent - settle it first so this
        // reflects the 64 inserts above, not a lagging count.
        cache.cache.run_pending_tasks().await;
        assert!(
            cache.cache.entry_count() <= 8,
            "resident entries ({}) must not exceed max_entries (8)",
            cache.cache.entry_count()
        );
    }

    #[tokio::test]
    async fn an_entry_past_max_stale_does_not_remain_resident_forever() {
        // Today `max_stale` only gates whether a *failed* refetch may still
        // serve a cached entry (`stale_membership_past_max_stale_is_refused`
        // above) - nothing removes the entry itself once it ages past that
        // bound, so a bucket the chain will never be asked about again stays
        // resident for the life of the process. Once bounded, an entry past
        // `max_stale` must be gone, not merely unserved.
        let account = AccountId32::new([51u8; 32]);
        let cache = MembershipCache::new(StaticMembershipResolver(vec![(
            account.clone(),
            Role::Reader,
        )
            .into()]))
        .with_ttl(Duration::from_secs(300))
        .with_max_stale(Duration::from_millis(10));

        cache.get_role(1, &account).await.unwrap();
        assert!(
            cache.cache.get(&1).await.is_some(),
            "the seed lookup must cache the entry"
        );

        tokio::time::sleep(Duration::from_millis(50)).await;

        assert!(
            cache.cache.get(&1).await.is_none(),
            "an entry older than max_stale must not remain resident"
        );
    }

    #[tokio::test]
    async fn a_refreshed_entry_gets_a_fresh_max_stale_not_the_original_ones_remainder() {
        // A successful refetch of an already-resident (not yet moka-expired)
        // entry takes moka's *update* path, not create - so residency must be
        // re-derived there too, or a bucket looked up more than once counts
        // down from its first insert while `fetched_at` keeps resetting,
        // silently shrinking its real stale-if-error grace toward zero.
        let account = AccountId32::new([52u8; 32]);
        let cache = MembershipCache::new(StaticMembershipResolver(vec![(
            account.clone(),
            Role::Reader,
        )
            .into()]))
        .with_ttl(Duration::from_millis(50))
        .with_max_stale(Duration::from_millis(300));

        cache.get_role(1, &account).await.unwrap();

        // Past ttl (not fresh) but well inside max_stale (not yet
        // moka-expired), so this lookup refetches and *updates* the existing
        // entry rather than creating a new one.
        tokio::time::sleep(Duration::from_millis(100)).await;
        cache.get_role(1, &account).await.unwrap();

        // 350ms after the original insert: past the original create-based
        // boundary (0 + max_stale = 300ms) by a 50ms margin, but within the
        // refreshed one (100 + max_stale = 400ms) by the same margin. Only
        // reachable if the update above actually reset residency.
        tokio::time::sleep(Duration::from_millis(250)).await;
        assert!(
            cache.cache.get(&1).await.is_some(),
            "a refresh must reset residency to a fresh max_stale, not leave the original \
             creation's expiration in place"
        );
    }

    #[tokio::test]
    async fn an_empty_member_set_expires_well_before_a_non_empty_one() {
        // A bucket that doesn't exist on chain resolves to an empty member
        // set exactly like a real, memberless one
        // (`provider-node/src/membership.rs:58-73`), and today both are
        // cached with the same lifetime as any other entry - the multiplier
        // that makes an unbounded map remotely driveable, since every id in
        // the `u64` space is a valid key. An empty result
        // must therefore go once its TTL lapses, rather than holding a slot
        // for the whole `max_stale` window a non-empty one earns.
        struct MixedResolver {
            account: AccountId32,
        }

        #[async_trait::async_trait]
        impl MembershipResolver for MixedResolver {
            async fn fetch_members(
                &self,
                bucket_id: BucketId,
            ) -> Result<Vec<Member>, MembershipError> {
                Ok(if bucket_id == 1 {
                    vec![] // "nonexistent" bucket
                } else {
                    vec![(self.account.clone(), Role::Reader).into()]
                })
            }
        }

        let account = AccountId32::new([52u8; 32]);
        let cache = MembershipCache::new(MixedResolver {
            account: account.clone(),
        })
        .with_ttl(Duration::from_millis(10))
        .with_max_stale(Duration::from_secs(300));

        cache.get_role(1, &account).await.unwrap(); // empty set
        cache.get_role(2, &account).await.unwrap(); // real member

        tokio::time::sleep(Duration::from_millis(50)).await;

        assert!(
            cache.cache.get(&1).await.is_none(),
            "an empty member set must not outlive its TTL"
        );
        assert!(
            cache.cache.get(&2).await.is_some(),
            "a non-empty member set must stay resident to max_stale, well past the TTL"
        );
    }
}
