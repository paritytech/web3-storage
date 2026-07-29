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

/// Chain-backed membership resolver using subxt dynamic queries.
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
        use subxt::dynamic::{At, Value};

        let api = self.source.current_api()?;

        let storage_query =
            subxt::dynamic::storage::<(Value,), Value>("StorageProvider", "Buckets");

        let at = api
            .at_current_block()
            .await
            .map_err(|e| format!("Failed to get storage: {e}"))?;
        let result = at
            .storage()
            .try_fetch(storage_query, (Value::u128(bucket_id as u128),))
            .await
            .map_err(|e| format!("Failed to fetch bucket: {e}"))?;

        let bucket_value = match result {
            Some(v) => v,
            None => return Ok(vec![]),
        };

        let decoded = bucket_value
            .decode()
            .map_err(|e| format!("Failed to decode bucket: {e}"))?;

        let members_val = match decoded.at("members") {
            Some(v) => v,
            None => return Ok(vec![]),
        };

        // `members` is a `BoundedVec<Member>`. Depending on how the runtime's
        // scale-info nests it, scale_value may wrap the sequence (and each
        // `AccountId32`) in extra single-field composites, so walk the value
        // tree and pull out every `{ account, role }` struct rather than
        // assuming a fixed shape.
        let mut members = Vec::new();
        collect_members(members_val, &mut members);

        if members.is_empty() {
            tracing::warn!(bucket_id, value = ?members_val, "auth: decoded zero members");
        } else {
            tracing::debug!(bucket_id, count = members.len(), "auth: resolved members");
        }

        Ok(members)
    }
}

/// Recursively pull `(account, role)` pairs out of a decoded `members` value,
/// tolerating any wrapper composites that `BoundedVec` / `AccountId32` type
/// info introduces. A `Member` is the composite that carries both an
/// `account` and a `role` field.
fn collect_members<T>(val: &subxt::ext::scale_value::Value<T>, out: &mut Vec<(AccountId32, Role)>) {
    use subxt::dynamic::At;
    use subxt::ext::scale_value::{Composite, ValueDef};

    if let (Some(account_v), Some(role_v)) = (val.at("account"), val.at("role")) {
        if let Some(bytes) = extract_account_bytes(account_v) {
            out.push((AccountId32::from(bytes), extract_role(role_v)));
            return;
        }
    }

    match &val.value {
        ValueDef::Composite(Composite::Named(fields)) => {
            for field in fields {
                collect_members(&field.1, out);
            }
        }
        ValueDef::Composite(Composite::Unnamed(items)) => {
            for item in items {
                collect_members(item, out);
            }
        }
        _ => {}
    }
}

/// Extract a 32-byte account id, descending through any wrapper composites
/// (`AccountId32` -> `[u8; 32]` can be one or more composite layers) and
/// collecting the `u8` leaves.
fn extract_account_bytes<T>(val: &subxt::ext::scale_value::Value<T>) -> Option<[u8; 32]> {
    let mut bytes = Vec::with_capacity(32);
    collect_u8_leaves(val, &mut bytes);
    if bytes.len() == 32 {
        let mut arr = [0u8; 32];
        arr.copy_from_slice(&bytes);
        Some(arr)
    } else {
        None
    }
}

fn collect_u8_leaves<T>(val: &subxt::ext::scale_value::Value<T>, out: &mut Vec<u8>) {
    use subxt::ext::scale_value::{Composite, Primitive, ValueDef};
    match &val.value {
        ValueDef::Primitive(Primitive::U128(n)) => out.push(*n as u8),
        ValueDef::Composite(Composite::Unnamed(items)) => {
            for item in items {
                collect_u8_leaves(item, out);
            }
        }
        ValueDef::Composite(Composite::Named(fields)) => {
            for field in fields {
                collect_u8_leaves(&field.1, out);
            }
        }
        _ => {}
    }
}

/// Decode a `Role`, descending through wrapper composites to the enum variant.
fn extract_role<T>(val: &subxt::ext::scale_value::Value<T>) -> Role {
    find_role_variant(val).unwrap_or(Role::Reader)
}

fn find_role_variant<T>(val: &subxt::ext::scale_value::Value<T>) -> Option<Role> {
    use subxt::ext::scale_value::{Composite, ValueDef};
    match &val.value {
        ValueDef::Variant(variant) => match variant.name.as_str() {
            "Admin" => Some(Role::Admin),
            "Writer" => Some(Role::Writer),
            "Reader" => Some(Role::Reader),
            _ => None,
        },
        ValueDef::Composite(Composite::Unnamed(items)) => {
            items.iter().find_map(|v| find_role_variant(v))
        }
        ValueDef::Composite(Composite::Named(fields)) => {
            fields.iter().find_map(|f| find_role_variant(&f.1))
        }
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Regression test for the dynamic decoding of `StorageProvider.Buckets`.
    #[test]
    fn collect_members_handles_chain_value_nesting() {
        use subxt::ext::scale_value::Value;

        let acct = [9u8; 32];
        // AccountId32 -> [u8; 32]: two composite layers around the byte leaves.
        let account = Value::unnamed_composite(vec![Value::unnamed_composite(
            acct.iter().map(|b| Value::u128(*b as u128)),
        )]);
        let member = Value::named_composite(vec![
            ("account".to_string(), account),
            ("role".to_string(), Value::unnamed_variant("Writer", vec![])),
        ]);
        let sequence = Value::unnamed_composite(vec![member]);
        // BoundedVec wrapper around the member sequence.
        let members_val = Value::unnamed_composite(vec![sequence]);

        let mut out = Vec::new();
        collect_members(&members_val, &mut out);

        assert_eq!(
            out.len(),
            1,
            "member must be recovered through both wrappers"
        );
        assert_eq!(out[0].0, AccountId32::from(acct));
        assert!(matches!(out[0].1, Role::Writer));
    }

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
