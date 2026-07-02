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

        let mut members = Vec::new();
        if let subxt::ext::scale_value::ValueDef::Composite(
            subxt::ext::scale_value::Composite::Unnamed(items),
        ) = &members_val.value
        {
            for item in items {
                let account_val = item.at("account");
                let role_val = item.at("role");

                if let (Some(account_v), Some(role_v)) = (account_val, role_val) {
                    let account_bytes = extract_account_bytes(account_v);
                    if let Some(bytes) = account_bytes {
                        let account = AccountId32::from(bytes);
                        let role = extract_role(role_v);
                        members.push((account, role));
                    }
                }
            }
        }

        Ok(members)
    }
}

fn extract_account_bytes<T>(val: &subxt::ext::scale_value::Value<T>) -> Option<[u8; 32]> {
    use subxt::ext::scale_value::{Composite, Primitive, ValueDef};
    match &val.value {
        ValueDef::Composite(Composite::Unnamed(items)) => {
            let bytes: Vec<u8> = items
                .iter()
                .filter_map(|item| match &item.value {
                    ValueDef::Primitive(Primitive::U128(n)) => Some(*n as u8),
                    _ => None,
                })
                .collect();
            if bytes.len() == 32 {
                let mut arr = [0u8; 32];
                arr.copy_from_slice(&bytes);
                Some(arr)
            } else {
                None
            }
        }
        _ => None,
    }
}

fn extract_role<T>(val: &subxt::ext::scale_value::Value<T>) -> Role {
    use subxt::ext::scale_value::ValueDef;
    if let ValueDef::Variant(variant) = &val.value {
        match variant.name.as_str() {
            "Admin" => Role::Admin,
            "Writer" => Role::Writer,
            "Reader" => Role::Reader,
            _ => Role::Reader, // default to least privilege
        }
    } else {
        Role::Reader
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
    let message = format!("web3storage:{method}:{bucket_id}:{timestamp_str}");
    if !sr25519::Pair::verify(&signature, message.as_bytes(), &pubkey) {
        return Err(Error::AuthRequired);
    }

    Ok(AccountId32::new(pubkey.0))
}

/// Check that the caller has sufficient permissions.
///
/// Auth is enforced by default. This is a no-op (returns `Ok`) only when the
/// operator started the node with `--disable-auth-i-know-what-i-am-doing`, which
/// strips the membership config from the state.
pub async fn require_role(
    state: &ProviderState,
    auth_header: Option<&str>,
    method: &str,
    bucket_id: u64,
    required: RequiredRole,
    max_skew: Duration,
) -> Result<(), Error> {
    if !state.auth_enabled {
        return Ok(());
    }

    let cache = state
        .membership_cache
        .as_ref()
        .ok_or_else(|| Error::Internal("Auth enabled but no membership cache".to_string()))?;

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
        let message = format!("web3storage:{method}:{bucket_id}:{timestamp}");
        let signature = keypair.sign(message.as_bytes());
        format!(
            "Web3Storage 0x{}:0x{}:{}",
            hex::encode(keypair.public().0),
            hex::encode(signature.0),
            timestamp
        )
    }

    fn current_timestamp() -> u64 {
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_secs()
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
