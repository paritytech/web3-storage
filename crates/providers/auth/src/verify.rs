// SPDX-License-Identifier: Apache-2.0

//! Request signature verification (sr25519) and role-based access control.

use crate::error::AuthError;
use crate::http_auth::auth_message;
use crate::membership::{
    MembershipCache, MembershipInvalidations, MembershipResolver, RequiredRole,
};
use sp_core::{crypto::AccountId32, sr25519, Pair};
use std::time::Duration;
use storage_primitives::BucketId;

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
///   [`AuthError::TimestampExpired`].
/// - `pubkey_hex` / `signature_hex` are hex (optionally `0x`-prefixed) encodings
///   of the 32-byte sr25519 public key and 64-byte signature.
///
/// On success returns the [`AccountId32`] derived from the recovered public key;
/// [`Authenticator::require_role`] maps that account to a bucket role.
fn verify_signature(
    auth_header: &str,
    method: &str,
    bucket_id: BucketId,
    max_skew: Duration,
) -> Result<AccountId32, AuthError> {
    let payload = auth_header
        .strip_prefix("Web3Storage ")
        .ok_or(AuthError::AuthRequired)?;

    let parts: Vec<&str> = payload.splitn(3, ':').collect();
    if parts.len() != 3 {
        return Err(AuthError::AuthRequired);
    }

    let pubkey_hex = parts[0];
    let sig_hex = parts[1];
    let timestamp_str = parts[2];

    // Validate timestamp
    let timestamp: u64 = timestamp_str
        .parse()
        .map_err(|_| AuthError::TimestampExpired)?;
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs();
    if now.abs_diff(timestamp) > max_skew.as_secs() {
        return Err(AuthError::TimestampExpired);
    }

    // Decode public key
    let pubkey_bytes = hex::decode(pubkey_hex.strip_prefix("0x").unwrap_or(pubkey_hex))
        .map_err(|_| AuthError::AuthRequired)?;
    if pubkey_bytes.len() != 32 {
        return Err(AuthError::AuthRequired);
    }
    let pubkey = sr25519::Public::from_raw(
        pubkey_bytes
            .as_slice()
            .try_into()
            .map_err(|_| AuthError::AuthRequired)?,
    );

    // Decode signature
    let sig_bytes = hex::decode(sig_hex.strip_prefix("0x").unwrap_or(sig_hex))
        .map_err(|_| AuthError::AuthRequired)?;
    if sig_bytes.len() != 64 {
        return Err(AuthError::AuthRequired);
    }
    let signature = sr25519::Signature::from_raw(
        sig_bytes
            .as_slice()
            .try_into()
            .map_err(|_| AuthError::AuthRequired)?,
    );

    // Verify signature. Two signing surfaces reach this endpoint:
    // - the Rust SDK / provider tests sign the raw message bytes;
    // - browser wallet extensions and PAPI's `PolkadotSigner.signBytes` never
    //   expose a raw key and wrap the payload in `<Bytes>…</Bytes>` before
    //   signing (see `@polkadot-api/signers-common`'s `getSignBytes`).
    // Accept either so wallet-backed clients (the UIs) can authenticate.
    let message = auth_message(method, bucket_id, timestamp_str);
    let verified = sr25519::Pair::verify(&signature, message.as_bytes(), &pubkey)
        || sr25519::Pair::verify(&signature, wrap_bytes(message.as_bytes()), &pubkey);
    if !verified {
        return Err(AuthError::AuthRequired);
    }

    Ok(AccountId32::new(pubkey.0))
}

/// Verifies signed requests and enforces bucket roles. Holds the settings that
/// do not change per request, so callers pass only the request itself.
pub struct Authenticator {
    membership: MembershipCache,
    max_skew: Duration,
}

impl Authenticator {
    /// `ttl` bounds how long a resolved member set is reused; `max_skew` how far
    /// a request timestamp may drift from the server clock.
    pub fn new(
        resolver: impl MembershipResolver + 'static,
        ttl: Duration,
        max_skew: Duration,
    ) -> Self {
        Self {
            membership: MembershipCache::new(resolver, ttl),
            max_skew,
        }
    }

    /// Feed membership invalidations into the cache, so a change takes effect
    /// on the next request rather than at the end of the TTL. Without this the
    /// TTL is the only bound.
    pub fn with_invalidations(mut self, feed: impl MembershipInvalidations + 'static) -> Self {
        self.membership = self.membership.with_invalidations(feed);
        self
    }

    /// Auth is always enforced: the caller must present a valid signed
    /// `Authorization` header whose account holds `required` for the bucket.
    pub async fn require_role(
        &self,
        auth_header: Option<&str>,
        method: &str,
        bucket_id: BucketId,
        required: RequiredRole,
    ) -> Result<(), AuthError> {
        let header = auth_header.ok_or(AuthError::AuthRequired)?;
        let account = verify_signature(header, method, bucket_id, self.max_skew)?;

        let role = self
            .membership
            .get_role(bucket_id, &account)
            .await?
            .ok_or(AuthError::InsufficientRole)?;

        if !required.is_satisfied_by(role) {
            return Err(AuthError::InsufficientRole);
        }

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::http_auth::build_auth_header;
    use crate::membership::{Member, StaticMembershipResolver};
    use sp_core::Pair;
    use std::time::Duration;
    use storage_primitives::Role;

    fn make_auth_header(
        keypair: &sr25519::Pair,
        method: &str,
        bucket_id: BucketId,
        timestamp: u64,
    ) -> String {
        build_auth_header(&keypair.public().0, method, bucket_id, timestamp, |msg| {
            keypair.sign(msg).0
        })
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
        let message = auth_message("PUT", 1, &ts.to_string());
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
        assert!(matches!(result, Err(AuthError::TimestampExpired)));
    }

    #[test]
    fn test_verify_missing_prefix() {
        let result = verify_signature("Bearer token123", "PUT", 1, Duration::from_secs(300));
        assert!(matches!(result, Err(AuthError::AuthRequired)));
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

    /// Run `require_role` against a bucket whose only member is `//Alice`
    /// holding `granted`, with a valid signed header from that same account.
    fn authenticator(members: Vec<Member>) -> Authenticator {
        Authenticator::new(
            StaticMembershipResolver(members),
            Duration::from_secs(60),
            Duration::from_secs(300),
        )
    }

    async fn require_role_for(granted: Role, required: RequiredRole) -> Result<(), AuthError> {
        let keypair = sr25519::Pair::from_string("//Alice", None).unwrap();
        let auth = authenticator(vec![(AccountId32::new(keypair.public().0), granted).into()]);
        let header = make_auth_header(&keypair, "PUT", 1, current_timestamp());

        auth.require_role(Some(&header), "PUT", 1, required).await
    }

    #[tokio::test]
    async fn require_role_enforces_the_role_matrix() {
        // Drives every (granted, required) pair through `require_role` so the
        // privilege ladder cannot be widened without a test failing.
        for (granted, required, allowed) in [
            (Role::Reader, RequiredRole::Reader, true),
            (Role::Reader, RequiredRole::Writer, false),
            (Role::Reader, RequiredRole::Admin, false),
            (Role::Writer, RequiredRole::Reader, true),
            (Role::Writer, RequiredRole::Writer, true),
            (Role::Writer, RequiredRole::Admin, false),
            (Role::Admin, RequiredRole::Reader, true),
            (Role::Admin, RequiredRole::Writer, true),
            (Role::Admin, RequiredRole::Admin, true),
        ] {
            let result = require_role_for(granted, required).await;
            if allowed {
                assert!(
                    result.is_ok(),
                    "{granted:?} must satisfy {required:?}, got {result:?}"
                );
            } else {
                assert!(
                    matches!(result, Err(AuthError::InsufficientRole)),
                    "{granted:?} must not satisfy {required:?}, got {result:?}"
                );
            }
        }
    }

    #[tokio::test]
    async fn accepts_a_resolver_chosen_at_runtime() {
        // Picking a resolver at runtime forces the caller to box it; the
        // `impl MembershipResolver` bound must still accept that.
        fn pick(mock: bool, member: Member) -> Box<dyn MembershipResolver> {
            if mock {
                Box::new(StaticMembershipResolver(vec![member]))
            } else {
                Box::new(StaticMembershipResolver(vec![]))
            }
        }

        let keypair = sr25519::Pair::from_string("//Alice", None).unwrap();
        let account = AccountId32::new(keypair.public().0);
        let resolver = pick(true, (account, Role::Admin).into());

        let auth = Authenticator::new(resolver, Duration::from_secs(60), Duration::from_secs(300));
        let header = make_auth_header(&keypair, "PUT", 1, current_timestamp());

        assert!(auth
            .require_role(Some(&header), "PUT", 1, RequiredRole::Admin)
            .await
            .is_ok());
    }

    #[tokio::test]
    async fn require_role_rejects_non_member() {
        // A valid signature is not authorization: Bob signs correctly but holds
        // no role in the bucket, so even a read must be refused.
        let alice = sr25519::Pair::from_string("//Alice", None).unwrap();
        let bob = sr25519::Pair::from_string("//Bob", None).unwrap();
        let auth = authenticator(vec![
            (AccountId32::new(alice.public().0), Role::Admin).into()
        ]);
        let header = make_auth_header(&bob, "GET", 1, current_timestamp());

        let result = auth
            .require_role(Some(&header), "GET", 1, RequiredRole::Reader)
            .await;
        assert!(matches!(result, Err(AuthError::InsufficientRole)));
    }

    #[tokio::test]
    async fn require_role_rejects_unsigned_and_forged_requests() {
        let alice = sr25519::Pair::from_string("//Alice", None).unwrap();
        let auth = authenticator(vec![
            (AccountId32::new(alice.public().0), Role::Admin).into()
        ]);

        let missing = auth
            .require_role(None, "GET", 1, RequiredRole::Reader)
            .await;
        assert!(matches!(missing, Err(AuthError::AuthRequired)));

        // Header signed for bucket 1 must not authorize bucket 2, even though
        // the signer is an Admin of the bucket it did sign for.
        let header = make_auth_header(&alice, "GET", 1, current_timestamp());
        let replayed = auth
            .require_role(Some(&header), "GET", 2, RequiredRole::Reader)
            .await;
        assert!(matches!(replayed, Err(AuthError::AuthRequired)));
    }
}
