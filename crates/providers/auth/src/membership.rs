// SPDX-License-Identifier: Apache-2.0

//! Bucket membership resolution with TTL caching.
//!
//! Resolution itself is left to [`MembershipResolver`]; this crate deliberately
//! knows nothing about the chain, so the node can supply a subxt-backed
//! implementation without dragging that dependency in here.

use dashmap::DashMap;
use sp_core::crypto::AccountId32;
use std::time::{Duration, Instant};
use storage_primitives::{BucketId, Role};

/// Cached membership entry for a bucket.
#[derive(Debug, Clone)]
struct CachedMembership {
    members: Vec<(AccountId32, Role)>,
    fetched_at: Instant,
}

/// Trait for resolving bucket membership (enables mocking in tests).
#[async_trait::async_trait]
pub trait MembershipResolver: Send + Sync {
    async fn fetch_members(&self, bucket_id: BucketId) -> Result<Vec<(AccountId32, Role)>, String>;
}

/// A [`MembershipResolver`] that returns a fixed member set for every bucket.
/// Used by integration tests across crates.
pub struct StaticMembershipResolver(pub Vec<(AccountId32, Role)>);

#[async_trait::async_trait]
impl MembershipResolver for StaticMembershipResolver {
    async fn fetch_members(
        &self,
        _bucket_id: BucketId,
    ) -> Result<Vec<(AccountId32, Role)>, String> {
        Ok(self.0.clone())
    }
}

/// TTL cache in front of a [`MembershipResolver`].
pub struct MembershipCache {
    cache: DashMap<BucketId, CachedMembership>,
    ttl: Duration,
    resolver: Box<dyn MembershipResolver>,
}

impl MembershipCache {
    pub fn new(resolver: Box<dyn MembershipResolver>, ttl: Duration) -> Self {
        Self {
            cache: DashMap::new(),
            ttl,
            resolver,
        }
    }

    /// Look up a caller's role in a bucket.
    /// Returns None if the caller is not a member.
    pub async fn get_role(
        &self,
        bucket_id: BucketId,
        account: &AccountId32,
    ) -> Result<Option<Role>, String> {
        // Check cache first
        if let Some(entry) = self.cache.get(&bucket_id) {
            if entry.fetched_at.elapsed() < self.ttl {
                return Ok(find_role(&entry.members, account));
            }
        }

        // Cache miss or stale — fetch from chain
        match self.resolver.fetch_members(bucket_id).await {
            Ok(members) => {
                let role = find_role(&members, account);
                self.cache.insert(
                    bucket_id,
                    CachedMembership {
                        members,
                        fetched_at: Instant::now(),
                    },
                );
                Ok(role)
            }
            Err(e) => {
                // Stale-while-revalidate: serve stale data if chain is unreachable
                if let Some(entry) = self.cache.get(&bucket_id) {
                    tracing::warn!(
                        "Chain unreachable for bucket {} membership, serving stale data: {}",
                        bucket_id,
                        e
                    );
                    return Ok(find_role(&entry.members, account));
                }
                Err(e)
            }
        }
    }
}

fn find_role(members: &[(AccountId32, Role)], account: &AccountId32) -> Option<Role> {
    members.iter().find(|(a, _)| a == account).map(|(_, r)| *r)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_find_role() {
        let alice = AccountId32::new([1u8; 32]);
        let bob = AccountId32::new([2u8; 32]);
        let charlie = AccountId32::new([3u8; 32]);

        let members = vec![
            (alice.clone(), Role::Admin),
            (bob.clone(), Role::Writer),
            (charlie.clone(), Role::Reader),
        ];

        assert_eq!(find_role(&members, &alice), Some(Role::Admin));
        assert_eq!(find_role(&members, &bob), Some(Role::Writer));
        assert_eq!(find_role(&members, &charlie), Some(Role::Reader));

        let unknown = AccountId32::new([4u8; 32]);
        assert_eq!(find_role(&members, &unknown), None);
    }
}
