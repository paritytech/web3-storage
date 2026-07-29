// SPDX-License-Identifier: Apache-2.0

//! Bucket membership resolution with TTL caching (queries chain via subxt).

use sp_core::crypto::AccountId32;
use std::time::{Duration, Instant};
use storage_primitives::{BucketId, Role};
use subxt::{OnlineClient, PolkadotConfig};

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

/// Membership cache backed by chain queries via subxt.
pub struct MembershipCache {
    cache: dashmap::DashMap<BucketId, CachedMembership>,
    ttl: Duration,
    resolver: Box<dyn MembershipResolver>,
}

impl MembershipCache {
    pub fn new(resolver: Box<dyn MembershipResolver>, ttl: Duration) -> Self {
        Self {
            cache: dashmap::DashMap::new(),
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

/// Source of the current live chain client.
///
/// Implementations are expected to follow reconnects (e.g. by borrowing the
/// client from a watch channel owned by whoever manages the connection).
/// Erroring while no connection has been established yet is expected and
/// retryable: the caller surfaces it as a lookup error and the request is
/// retried later.
pub trait ChainApiSource: Send + Sync {
    fn current_api(&self) -> Result<OnlineClient<PolkadotConfig>, String>;
}

/// Chain-backed membership resolver using the typed `storage-subxt` bindings.
///
/// Borrows the current chain connection from a [`ChainApiSource`] on every
/// lookup, so lookups automatically follow reconnects instead of holding
/// their own (never-reconnecting) socket.
pub struct ChainMembershipResolver {
    source: Box<dyn ChainApiSource>,
}

impl ChainMembershipResolver {
    pub fn new(source: Box<dyn ChainApiSource>) -> Self {
        Self { source }
    }
}

#[async_trait::async_trait]
impl MembershipResolver for ChainMembershipResolver {
    async fn fetch_members(&self, bucket_id: BucketId) -> Result<Vec<(AccountId32, Role)>, String> {
        let api = self.source.current_api()?;

        // Typed read via the static bindings. `unvalidated`: the bindings are
        // generated from the paseo runtime, and the local runtime shares the
        // pallet - exact-hash validation would couple the binary to a single
        // runtime build for no safety gain (a shape mismatch fails decoding).
        let storage_address = storage_subxt::api::storage()
            .storage_provider()
            .buckets()
            .unvalidated();

        let at = api
            .at_current_block()
            .await
            .map_err(|e| format!("Failed to get storage: {e}"))?;
        let result = at
            .storage()
            .try_fetch(storage_address, (bucket_id,))
            .await
            .map_err(|e| format!("Failed to fetch bucket: {e}"))?;

        let bucket_value = match result {
            Some(v) => v,
            None => return Ok(vec![]),
        };

        let bucket = bucket_value
            .decode()
            .map_err(|e| format!("Failed to decode bucket: {e}"))?;

        let members: Vec<(AccountId32, Role)> = bucket
            .members
            .0
            .into_iter()
            .map(|m| (AccountId32::new(m.account.0), from_runtime_role(m.role)))
            .collect();

        if members.is_empty() {
            tracing::warn!(bucket_id, "auth: decoded zero members");
        } else {
            tracing::debug!(bucket_id, count = members.len(), "auth: resolved members");
        }

        Ok(members)
    }
}

fn from_runtime_role(role: storage_subxt::api::runtime_types::storage_primitives::Role) -> Role {
    use storage_subxt::api::runtime_types::storage_primitives::Role as RuntimeRole;
    match role {
        RuntimeRole::Admin => Role::Admin,
        RuntimeRole::Writer => Role::Writer,
        RuntimeRole::Reader => Role::Reader,
    }
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
