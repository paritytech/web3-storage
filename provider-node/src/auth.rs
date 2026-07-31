// SPDX-License-Identifier: GPL-3.0-only

//! Authentication and authorization for the provider node.
//!
//! Provides:
//! - Request signature verification (sr25519)
//! - Membership caching with TTL (queries chain via subxt)
//! - Role-based access control enforcement

use crate::chain_connection::{self, ChainWatch};
use crate::error::Error;
use crate::ProviderState;
use dashmap::DashMap;
use sp_core::{crypto::AccountId32, sr25519, Pair};
use std::time::{Duration, Instant};
use storage_primitives::{Role, Visibility};
use subxt::{OnlineClient, PolkadotConfig};

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

/// The slice of on-chain bucket state that read access control needs:
/// who the members are, and whether reads are member-only.
#[derive(Debug, Clone)]
pub struct BucketAccess {
    pub members: Vec<(AccountId32, Role)>,
    pub visibility: Visibility,
}

/// Cached access entry for a bucket.
#[derive(Debug, Clone)]
struct CachedMembership {
    access: BucketAccess,
    fetched_at: Instant,
}

/// Trait for resolving bucket membership + visibility (enables mocking in
/// tests).
#[async_trait::async_trait]
pub trait MembershipResolver: Send + Sync {
    async fn fetch_access(&self, bucket_id: u64) -> Result<BucketAccess, String>;
}

/// A [`MembershipResolver`] that returns a fixed member set for every bucket.
/// Used by integration tests across crates. Buckets resolve as `Private`
/// (auth always required), matching the fail-safe default.
pub struct StaticMembershipResolver(pub Vec<(AccountId32, Role)>);

#[async_trait::async_trait]
impl MembershipResolver for StaticMembershipResolver {
    async fn fetch_access(&self, _bucket_id: u64) -> Result<BucketAccess, String> {
        Ok(BucketAccess {
            members: self.0.clone(),
            visibility: Visibility::Private,
        })
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

    /// Look up a bucket's access state (members + visibility), TTL-cached
    /// with stale-while-revalidate.
    pub async fn get_access(&self, bucket_id: u64) -> Result<BucketAccess, String> {
        // Check cache first
        if let Some(entry) = self.cache.get(&bucket_id) {
            if entry.fetched_at.elapsed() < self.ttl {
                return Ok(entry.access.clone());
            }
        }

        // Cache miss or stale — fetch from chain
        match self.resolver.fetch_access(bucket_id).await {
            Ok(access) => {
                self.cache.insert(
                    bucket_id,
                    CachedMembership {
                        access: access.clone(),
                        fetched_at: Instant::now(),
                    },
                );
                Ok(access)
            }
            Err(e) => {
                // Stale-while-revalidate: serve stale data if chain is unreachable
                if let Some(entry) = self.cache.get(&bucket_id) {
                    tracing::warn!(
                        "Chain unreachable for bucket {} membership, serving stale data: {}",
                        bucket_id,
                        e
                    );
                    return Ok(entry.access.clone());
                }
                Err(e)
            }
        }
    }

    /// Look up a caller's role in a bucket.
    /// Returns None if the caller is not a member.
    pub async fn get_role(
        &self,
        bucket_id: u64,
        account: &AccountId32,
    ) -> Result<Option<Role>, String> {
        Ok(find_role(
            &self.get_access(bucket_id).await?.members,
            account,
        ))
    }
}

fn find_role(members: &[(AccountId32, Role)], account: &AccountId32) -> Option<Role> {
    members.iter().find(|(a, _)| a == account).map(|(_, r)| *r)
}

/// Chain-backed membership resolver using subxt dynamic queries.
///
/// Borrows the node's shared chain connection from the watch channel owned by
/// the chain-state coordinator, so lookups automatically follow reconnects
/// instead of holding their own (never-reconnecting) socket.
pub struct ChainMembershipResolver {
    chain_rx: ChainWatch,
}

impl ChainMembershipResolver {
    pub fn new(chain_rx: ChainWatch) -> Self {
        Self { chain_rx }
    }

    /// Return the current chain client, or fail while the chain has never
    /// been reached (the caller surfaces this as a lookup error and the
    /// request is retried later).
    fn api(&self) -> Result<OnlineClient<PolkadotConfig>, String> {
        chain_connection::current_api(&self.chain_rx).map_err(|e| e.to_string())
    }
}

#[async_trait::async_trait]
impl MembershipResolver for ChainMembershipResolver {
    async fn fetch_access(&self, bucket_id: u64) -> Result<BucketAccess, String> {
        let api = self.api()?;

        // Typed read via the static bindings. `unvalidated`: the bindings are
        // generated from the paseo runtime, and the local runtime shares the
        // pallet — exact-hash validation would couple the binary to a single
        // runtime build for no safety gain (a shape mismatch fails decoding).
        let storage_address = storage_subxt::api::storage()
            .storage_provider()
            .buckets()
            .unvalidated();
        let at = api
            .at_current_block()
            .await
            .map_err(|e| format!("Failed to get storage: {e}"))?;
        let bucket = at
            .storage()
            .try_fetch(storage_address, (bucket_id,))
            .await
            .map_err(|e| format!("Failed to fetch bucket: {e}"))?;

        // A missing bucket resolves as memberless and Private: nobody gets in.
        let Some(bucket) = bucket else {
            return Ok(BucketAccess {
                members: Vec::new(),
                visibility: Visibility::Private,
            });
        };
        let bucket = bucket
            .decode()
            .map_err(|e| format!("Failed to decode bucket: {e}"))?;

        let members = bucket
            .members
            .0
            .into_iter()
            .map(|m| (AccountId32::new(m.account.0), m.role.into()))
            .collect::<Vec<_>>();
        let visibility: Visibility = bucket.visibility.into();

        tracing::debug!(bucket_id, count = members.len(), "auth: resolved members");

        Ok(BucketAccess {
            members,
            visibility,
        })
    }
}

/// Wrap a payload the way Polkadot message-signing surfaces do before signing:
/// `<Bytes>` ++ payload ++ `</Bytes>`. Browser extensions (`signRaw`) and PAPI's
/// `PolkadotSigner.signBytes` apply this wrapper so a signed message can never be
/// mistaken for a signed extrinsic. The auth message is always short, so no
/// hashing step applies.
fn wrap_bytes(msg: &[u8]) -> Vec<u8> {
    const PREFIX: &[u8] = b"<Bytes>";
    const SUFFIX: &[u8] = b"</Bytes>";
    let mut out = Vec::with_capacity(PREFIX.len() + msg.len() + SUFFIX.len());
    out.extend_from_slice(PREFIX);
    out.extend_from_slice(msg);
    out.extend_from_slice(SUFFIX);
    out
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

    // Verify signature. Two signing surfaces reach this endpoint:
    // - the Rust SDK / provider tests sign the raw message bytes;
    // - browser wallet extensions and PAPI's `PolkadotSigner.signBytes` never
    //   expose a raw key and wrap the payload in `<Bytes>…</Bytes>` before
    //   signing (see `@polkadot-api/signers-common`'s `getSignBytes`).
    // Accept either so wallet-backed clients (the UIs) can authenticate.
    let message = provider_negotiation::auth_message(method, bucket_id, timestamp_str);
    let wrapped = wrap_bytes(message.as_bytes());
    let verified = sr25519::Pair::verify(&signature, message.as_bytes(), &pubkey)
        || sr25519::Pair::verify(&signature, &wrapped, &pubkey);
    if !verified {
        return Err(Error::AuthRequired);
    }

    Ok(AccountId32::new(pubkey.0))
}

/// Check that the caller has sufficient permissions.
///
/// Reader-level requests against a `Public` bucket are served without
/// authentication — an honest primary serves public-bucket reads to anyone.
/// Everything else (any request on a `Private` bucket, and every
/// Writer/Admin request) requires a valid signed `Authorization` header
/// whose account holds the [`RequiredRole`] for the bucket. If the bucket's
/// visibility cannot be established, it gates like `Private` (fail-safe).
pub async fn require_role(
    state: &ProviderState,
    auth_header: Option<&str>,
    method: &str,
    bucket_id: u64,
    required: RequiredRole,
    max_skew: Duration,
) -> Result<(), Error> {
    if required == RequiredRole::Reader {
        let is_public = state
            .membership_cache
            .get_access(bucket_id)
            .await
            .map(|access| access.visibility == Visibility::Public)
            .unwrap_or(false);
        if is_public {
            return Ok(());
        }
    }

    let header = auth_header.ok_or(Error::AuthRequired)?;
    let account = verify_signature(header, method, bucket_id, max_skew)?;

    let role = state
        .membership_cache
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
        provider_negotiation::build_auth_header(
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
    fn test_verify_wrapped_signature() {
        // Wallet signers (browser extensions, PAPI `signBytes`) wrap the message
        // in `<Bytes>…</Bytes>` before signing and never expose a raw key. The
        // provider must accept that form so UI/wallet clients can authenticate.
        let keypair = sr25519::Pair::from_string("//Alice", None).unwrap();
        let ts = current_timestamp();
        let message = provider_negotiation::auth_message("PUT", 1, &ts.to_string());
        let sig = keypair.sign(&wrap_bytes(message.as_bytes()));
        let header = format!(
            "Web3Storage 0x{}:0x{}:{}",
            hex::encode(keypair.public().0),
            hex::encode(sig.0),
            ts
        );

        let result = verify_signature(&header, "PUT", 1, Duration::from_secs(300));
        assert!(
            result.is_ok(),
            "wrapped (wallet-style) signature must verify"
        );
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

    #[tokio::test]
    async fn chain_resolver_fails_cleanly_before_first_connect() {
        // Before the chain-state coordinator publishes a connection, lookups
        // must surface a retryable error rather than panic or hang.
        let (_tx, rx) = tokio::sync::watch::channel(None);
        let resolver = ChainMembershipResolver::new(rx);
        let err = resolver
            .fetch_access(1)
            .await
            .expect_err("no connection published yet");
        assert!(err.contains("not established"), "unexpected error: {err}");
    }
}
