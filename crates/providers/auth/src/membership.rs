// SPDX-License-Identifier: Apache-2.0

//! Bucket membership and the role ladder. Resolution is left to
//! [`MembershipResolver`], so this crate needs no chain dependency.

use crate::error::MembershipError;
use dashmap::DashMap;
use sp_core::crypto::AccountId32;
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

    fn is_fresh(&self, ttl: Duration) -> bool {
        self.fetched_at.elapsed() < ttl
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

/// A bucket's cache slot: its membership (if any) plus a generation counter
/// bumped on every [`MembershipCache::invalidate`] call.
///
/// The generation lets a resolver call that was already in flight when an
/// `invalidate()` landed detect that its result is now stale and must not be
/// persisted. Without it, a fetch started just before a revocation could still
/// write the pre-revocation members into the cache *after* the invalidation ran,
/// silently resurrecting the revoked access for a full TTL.
#[derive(Debug, Clone)]
struct CacheSlot {
    membership: Option<CachedMembership>,
    generation: u64,
}

/// TTL cache in front of a [`MembershipResolver`].
pub(crate) struct MembershipCache {
    cache: DashMap<BucketId, CacheSlot>,
    ttl: Duration,
    resolver: Box<dyn MembershipResolver>,
}

impl MembershipCache {
    pub(crate) fn new(resolver: impl MembershipResolver + 'static, ttl: Duration) -> Self {
        Self {
            cache: DashMap::new(),
            ttl,
            resolver: Box::new(resolver),
        }
    }

    /// Look up a caller's role in a bucket.
    /// Returns None if the caller is not a member.
    pub(crate) async fn get_role(
        &self,
        bucket_id: BucketId,
        account: &AccountId32,
    ) -> Result<Option<Role>, MembershipError> {
        // Check cache first, remembering the generation so the fetch below can
        // detect an invalidate() that lands while it is in flight.
        let generation_before = match self.cache.get(&bucket_id) {
            Some(slot) => {
                if let Some(entry) = &slot.membership {
                    if entry.is_fresh(self.ttl) {
                        return Ok(entry.role_of(account));
                    }
                }
                slot.generation
            }
            None => 0,
        };

        // Cache miss or stale — fetch from chain
        match self.resolver.fetch_members(bucket_id).await {
            Ok(members) => {
                let entry = CachedMembership::new(members);
                let role = entry.role_of(account);
                // Persist only if the generation hasn't moved since we started;
                // otherwise an invalidate() ran while we awaited the chain and
                // this result is already stale. `and_modify`/`or_insert_with`
                // take the same per-key lock as `invalidate`, so there is no
                // window between the check and the write.
                self.cache
                    .entry(bucket_id)
                    .and_modify(|slot| {
                        if slot.generation == generation_before {
                            slot.membership = Some(entry.clone());
                        }
                    })
                    .or_insert_with(|| CacheSlot {
                        membership: Some(entry),
                        generation: generation_before,
                    });
                Ok(role)
            }
            Err(e) => {
                // Stale-while-revalidate: serve stale data if chain is unreachable
                if let Some(entry) = self
                    .cache
                    .get(&bucket_id)
                    .and_then(|slot| slot.membership.clone())
                {
                    tracing::warn!(
                        "Chain unreachable for bucket {} membership, serving stale data: {}",
                        bucket_id,
                        e
                    );
                    return Ok(entry.role_of(account));
                }
                Err(e)
            }
        }
    }

    /// Drop the cached membership for `bucket_id`, if any, so the next lookup
    /// re-resolves it, and bump its generation so a resolver call already in
    /// flight cannot resurrect the invalidated membership when it completes.
    pub(crate) fn invalidate(&self, bucket_id: BucketId) {
        self.cache
            .entry(bucket_id)
            .and_modify(|slot| {
                slot.membership = None;
                slot.generation = slot.generation.saturating_add(1);
            })
            .or_insert_with(|| CacheSlot {
                membership: None,
                // Above the 0 an in-flight fetch recorded for an absent slot, so
                // that fetch's result is discarded rather than cached.
                generation: 1,
            });
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
}
