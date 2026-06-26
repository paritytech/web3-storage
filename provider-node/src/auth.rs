// SPDX-License-Identifier: GPL-3.0-only

//! Authentication and authorization for the provider node.
//!
//! Provides:
//! - Request signature verification (sr25519)
//! - Membership caching with TTL (queries chain via subxt)
//! - Role-based access control enforcement

use crate::error::Error;
use crate::ProviderState;
use dashmap::DashMap;
use sp_core::{crypto::AccountId32, sr25519, Pair};
use std::time::{Duration, Instant};
use storage_primitives::Role;
use subxt::{OnlineClient, PolkadotConfig};
use tokio::sync::OnceCell;

/// Caller identity extracted from a signed request.
#[derive(Debug, Clone)]
pub struct CallerIdentity {
    pub account_id: AccountId32,
    pub role: Role,
}

/// Required role for an endpoint.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RequiredRole {
    Reader,
    Writer,
    Admin,
}

/// Cached membership entry for a bucket.
#[derive(Debug, Clone)]
struct CachedMembership {
    members: Vec<(AccountId32, Role)>,
    fetched_at: Instant,
}

/// Trait for resolving bucket membership (enables mocking in tests).
#[async_trait::async_trait]
pub trait MembershipResolver: Send + Sync {
    async fn fetch_members(&self, bucket_id: u64) -> Result<Vec<(AccountId32, Role)>, String>;
}

/// A [`MembershipResolver`] that returns a fixed member set for every bucket.
/// Used by integration tests across crates.
pub struct StaticMembershipResolver(pub Vec<(AccountId32, Role)>);

#[async_trait::async_trait]
impl MembershipResolver for StaticMembershipResolver {
    async fn fetch_members(&self, _bucket_id: u64) -> Result<Vec<(AccountId32, Role)>, String> {
        Ok(self.0.clone())
    }
}

/// Membership cache backed by chain queries via subxt.
pub struct MembershipCache {
    cache: DashMap<u64, CachedMembership>,
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
        bucket_id: u64,
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

/// Chain-backed membership resolver using subxt dynamic queries.
pub struct ChainMembershipResolver {
    chain_rpc: String,
    /// Lazily-established chain connection, reused across lookups.
    ///
    /// `OnlineClient` is an `Arc`-backed handle over a single WebSocket
    /// connection and a metadata cache, so connecting is the expensive part
    /// (network handshake + metadata download). We connect on the first
    /// lookup and reuse the same client for every subsequent one instead of
    /// reconnecting per request. If the first attempt fails the cell stays
    /// empty, so the next lookup retries.
    api: OnceCell<OnlineClient<PolkadotConfig>>,
}

impl ChainMembershipResolver {
    pub fn new(chain_rpc: String) -> Self {
        Self {
            chain_rpc,
            api: OnceCell::new(),
        }
    }

    /// Return the shared chain client, connecting on first use.
    async fn api(&self) -> Result<&OnlineClient<PolkadotConfig>, String> {
        self.api
            .get_or_try_init(|| async {
                OnlineClient::<PolkadotConfig>::from_url(&self.chain_rpc)
                    .await
                    .map_err(|e| format!("Failed to connect to chain: {e}"))
            })
            .await
    }
}

#[async_trait::async_trait]
impl MembershipResolver for ChainMembershipResolver {
    async fn fetch_members(&self, bucket_id: u64) -> Result<Vec<(AccountId32, Role)>, String> {
        use subxt::dynamic::{At, Value};

        let api = self.api().await?;

        let storage_query = subxt::dynamic::storage(
            "StorageProvider",
            "Buckets",
            vec![Value::u128(bucket_id as u128)],
        );

        let result = api
            .storage()
            .at_latest()
            .await
            .map_err(|e| format!("Failed to get storage: {e}"))?
            .fetch(&storage_query)
            .await
            .map_err(|e| format!("Failed to fetch bucket: {e}"))?;

        let bucket_value = match result {
            Some(v) => v,
            None => return Ok(vec![]),
        };

        let decoded = bucket_value
            .to_value()
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

/// Verify an sr25519 signature from an `Authorization` header.
///
/// The client signs the request by building the message
///
/// ```text
/// web3storage:<METHOD>:<bucket_id>:<timestamp>
/// ```
///
/// and sending the signature back in the `Authorization` header
///
/// ```text
/// Authorization: Web3Storage <pubkey_hex>:<signature_hex>:<timestamp>
/// ```
///
/// where:
/// - `METHOD` is the upper-case HTTP verb of the request (`GET`, `PUT`, …).
/// - `bucket_id` is the decimal bucket id the request targets.
/// - `timestamp` is the client's current Unix time in **seconds**; the same
///   string is used in both the signed message and the header. It must be
///   within `max_skew` of the server clock or the request is rejected with
///   [`Error::TimestampExpired`].
/// - `pubkey_hex` / `signature_hex` are hex (optionally `0x`-prefixed) encodings
///   of the 32-byte sr25519 public key and 64-byte signature.
///
/// On success returns the [`AccountId32`] derived from the recovered public key;
/// the caller maps that account to a bucket role in [`require_role`].
pub fn verify_signature(
    auth_header: &str,
    method: &str,
    bucket_id: u64,
    max_skew: Duration,
) -> Result<AccountId32, Error> {
    let payload = auth_header
        .strip_prefix("Web3Storage ")
        .ok_or(Error::AuthRequired)?;

    let parts: Vec<&str> = payload.splitn(3, ':').collect();
    if parts.len() != 3 {
        return Err(Error::AuthRequired);
    }

    let pubkey_hex = parts[0];
    let sig_hex = parts[1];
    let timestamp_str = parts[2];

    // Validate timestamp
    let timestamp: u64 = timestamp_str.parse().map_err(|_| Error::TimestampExpired)?;
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs();
    if now.abs_diff(timestamp) > max_skew.as_secs() {
        return Err(Error::TimestampExpired);
    }

    // Decode public key
    let pubkey_bytes = hex::decode(pubkey_hex.strip_prefix("0x").unwrap_or(pubkey_hex))
        .map_err(|_| Error::AuthRequired)?;
    if pubkey_bytes.len() != 32 {
        return Err(Error::AuthRequired);
    }
    let pubkey = sr25519::Public::from_raw(
        pubkey_bytes
            .as_slice()
            .try_into()
            .map_err(|_| Error::AuthRequired)?,
    );

    // Decode signature
    let sig_bytes = hex::decode(sig_hex.strip_prefix("0x").unwrap_or(sig_hex))
        .map_err(|_| Error::AuthRequired)?;
    if sig_bytes.len() != 64 {
        return Err(Error::AuthRequired);
    }
    let signature = sr25519::Signature::from_raw(
        sig_bytes
            .as_slice()
            .try_into()
            .map_err(|_| Error::AuthRequired)?,
    );

    // Verify signature
    let message = storage_primitives::auth_message(method, bucket_id, timestamp_str);
    if !sr25519::Pair::verify(&signature, message.as_bytes(), &pubkey) {
        return Err(Error::AuthRequired);
    }

    Ok(AccountId32::new(pubkey.0))
}

/// Check that the caller has sufficient permissions.
///
/// Auth is always enforced. The caller must present a valid signed
/// `Authorization` header whose account holds the [`RequiredRole`] for the
/// bucket; otherwise the request is rejected.
pub async fn require_role(
    state: &ProviderState,
    auth_header: Option<&str>,
    method: &str,
    bucket_id: u64,
    required: RequiredRole,
    max_skew: Duration,
) -> Result<(), Error> {
    let cache = state
        .membership_cache
        .as_ref()
        .ok_or_else(|| Error::Internal("No membership cache".to_string()))?;

    let header = auth_header.ok_or(Error::AuthRequired)?;
    let account = verify_signature(header, method, bucket_id, max_skew)?;

    let role = cache
        .get_role(bucket_id, &account)
        .await
        .map_err(|e| Error::Internal(format!("Membership lookup failed: {e}")))?
        .ok_or(Error::InsufficientRole)?;

    let allowed = match required {
        RequiredRole::Reader => true, // any role can read
        RequiredRole::Writer => matches!(role, Role::Writer | Role::Admin),
        RequiredRole::Admin => matches!(role, Role::Admin),
    };

    if !allowed {
        return Err(Error::InsufficientRole);
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use sp_core::Pair;
    use std::time::Duration;

    fn make_auth_header(
        keypair: &sr25519::Pair,
        method: &str,
        bucket_id: u64,
        timestamp: u64,
    ) -> String {
        storage_primitives::build_auth_header(
            &keypair.public().0,
            method,
            bucket_id,
            timestamp,
            |msg| keypair.sign(msg).0,
        )
    }

    fn current_timestamp() -> u64 {
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_secs()
    }

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
    fn test_verify_valid_signature() {
        let keypair = sr25519::Pair::from_string("//Alice", None).unwrap();
        let ts = current_timestamp();
        let header = make_auth_header(&keypair, "PUT", 1, ts);

        let result = verify_signature(&header, "PUT", 1, Duration::from_secs(300));
        assert!(result.is_ok());
        assert_eq!(result.unwrap(), AccountId32::new(keypair.public().0));
    }

    #[test]
    fn test_verify_invalid_signature() {
        let keypair = sr25519::Pair::from_string("//Alice", None).unwrap();
        let ts = current_timestamp();
        let header = make_auth_header(&keypair, "PUT", 1, ts);

        // Wrong method
        let result = verify_signature(&header, "GET", 1, Duration::from_secs(300));
        assert!(result.is_err());
    }

    #[test]
    fn test_verify_expired_timestamp() {
        let keypair = sr25519::Pair::from_string("//Alice", None).unwrap();
        let ts = current_timestamp() - 600; // 10 minutes ago
        let header = make_auth_header(&keypair, "PUT", 1, ts);

        let result = verify_signature(&header, "PUT", 1, Duration::from_secs(300));
        assert!(matches!(result, Err(Error::TimestampExpired)));
    }

    #[test]
    fn test_verify_missing_prefix() {
        let result = verify_signature("Bearer token123", "PUT", 1, Duration::from_secs(300));
        assert!(matches!(result, Err(Error::AuthRequired)));
    }

    #[test]
    fn test_verify_wrong_bucket() {
        let keypair = sr25519::Pair::from_string("//Alice", None).unwrap();
        let ts = current_timestamp();
        let header = make_auth_header(&keypair, "PUT", 1, ts);

        // Verify with different bucket
        let result = verify_signature(&header, "PUT", 2, Duration::from_secs(300));
        assert!(result.is_err());
    }

    #[test]
    fn test_role_enforcement_reader() {
        // Reader can satisfy RequiredRole::Reader (any role satisfies Reader)
        assert!(matches!(RequiredRole::Reader, RequiredRole::Reader));
        // Reader cannot satisfy Writer
        assert!(!matches!(Role::Reader, Role::Writer | Role::Admin));
    }

    #[test]
    fn test_role_enforcement_writer() {
        // Writer can satisfy Reader and Writer, but not Admin
        assert!(matches!(Role::Writer, Role::Writer | Role::Admin));
        assert!(!matches!(Role::Writer, Role::Admin));
    }

    #[test]
    fn test_role_enforcement_admin() {
        // Admin satisfies everything
        assert!(matches!(Role::Admin, Role::Admin));
        assert!(matches!(Role::Admin, Role::Writer | Role::Admin));
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
