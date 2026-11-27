// SPDX-License-Identifier: Apache-2.0

//! Request signature verification (sr25519) and role-based access control.

use crate::error::AuthError;
use crate::http_auth::auth_message;
use crate::membership::{
    role_satisfies, LocalBucketAgreement, MembershipCache, MembershipInvalidations,
    MembershipResolver,
};
use sp_core::{crypto::AccountId32, sr25519, Pair};
use std::time::Duration;
use storage_primitives::{BucketId, Role, Visibility};

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
    let pubkey = sr25519::Public::from_raw(
        array_bytes::hex2array(pubkey_hex).map_err(|_| AuthError::AuthRequired)?,
    );

    // Decode signature
    let signature = sr25519::Signature::from_raw(
        array_bytes::hex2array(sig_hex).map_err(|_| AuthError::AuthRequired)?,
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
    /// The node's own provider account — see [`Self::with_provider_account`].
    provider_account: Option<AccountId32>,
}

/// Default clock-skew tolerance. Matches `--auth-max-skew`'s default.
const DEFAULT_MAX_SKEW: Duration = Duration::from_secs(300);

impl Authenticator {
    /// Every bound has a default; set them with the `with_*` builders. Named
    /// rather than positional because they are all `Duration`s, so a positional
    /// signature lets two of them be swapped silently.
    pub fn new(resolver: impl MembershipResolver + 'static) -> Self {
        Self {
            membership: MembershipCache::new(resolver),
            max_skew: DEFAULT_MAX_SKEW,
            provider_account: None,
        }
    }

    /// The node's own provider account. A request signed by it passes every
    /// Reader check: the operator can read the node's disk anyway, and
    /// provider accounts are never bucket members, so the provider's own
    /// tooling (e.g. the dashboard fetching challenge proofs) has no other
    /// identity to authenticate with. Writer/Admin checks are unaffected.
    pub fn with_provider_account(mut self, account: AccountId32) -> Self {
        self.provider_account = Some(account);
        self
    }

    /// Feed membership invalidations into the cache, so a change takes effect
    /// on the next request rather than at the end of the TTL. Without this the
    /// TTL is the only bound.
    pub fn with_invalidations(mut self, feed: impl MembershipInvalidations + 'static) -> Self {
        self.membership = self.membership.with_invalidations(feed);
        self
    }

    /// How long a resolved member set is reused before the chain is rechecked.
    pub fn with_ttl(mut self, ttl: Duration) -> Self {
        self.membership = self.membership.with_ttl(ttl);
        self
    }

    /// How far a request timestamp may drift from the server clock.
    pub fn with_max_skew(mut self, max_skew: Duration) -> Self {
        self.max_skew = max_skew;
        self
    }

    /// How old a cached member set may be and still authorize a request while
    /// the chain is unreachable; past it the lookup fails rather than
    /// authorizing from an unboundedly stale snapshot. Defaults to 5 minutes.
    pub fn with_max_stale(mut self, max_stale: Duration) -> Self {
        self.membership = self.membership.with_max_stale(max_stale);
        self
    }

    /// Maximum number of buckets' membership resident in the cache at once.
    /// Defaults to 10,000.
    pub fn with_max_entries(mut self, max_entries: u64) -> Self {
        self.membership = self.membership.with_max_entries(max_entries);
        self
    }

    /// Checks that a request may access `bucket_id` with role `required`.
    ///
    /// - Writer or Admin on a bucket this node serves as a replica: fails with
    ///   [`AuthError::NotPrimary`], whoever signs it. A replica takes data only
    ///   by sync.
    /// - Reader on a `Public` bucket or a replica bucket: passes without
    ///   authentication (replicas serve everyone, per the design's
    ///   anti-censorship rule).
    /// - Otherwise: needs a signed `Authorization` header from an account with
    ///   `required` on the bucket. The node's own provider account passes any
    ///   Reader check (see [`Self::with_provider_account`]). A bucket with
    ///   unknown visibility counts as `Private`.
    pub async fn require_role(
        &self,
        auth_header: Option<&str>,
        method: &str,
        bucket_id: BucketId,
        required: Role,
    ) -> Result<(), AuthError> {
        // One lookup serves both gates; its failure only matters once the
        // request has proven it needs a role.
        let access = self.membership.lookup(bucket_id).await;
        if required != Role::Reader
            && access
                .as_ref()
                .is_ok_and(|entry| entry.access.serves_as_replica())
        {
            return Err(AuthError::NotPrimary);
        }
        if required == Role::Reader
            && access.as_ref().is_ok_and(|entry| {
                entry.access.visibility == Visibility::Public || entry.access.serves_as_replica()
            })
        {
            return Ok(());
        }

        let header = auth_header.ok_or(AuthError::AuthRequired)?;
        let account = verify_signature(header, method, bucket_id, self.max_skew)?;

        // Checked before the membership lookup result is consulted, so
        // operator reads keep working through a chain outage.
        if required == Role::Reader && self.provider_account.as_ref() == Some(&account) {
            return Ok(());
        }

        let role = access?
            .role_of(&account)
            .ok_or(AuthError::InsufficientRole)?;

        if !role_satisfies(required, role) {
            return Err(AuthError::InsufficientRole);
        }

        Ok(())
    }

    /// Checks that this node's agreement on `bucket_id` allows a request
    /// with role `required` at `anchor_block`, and returns the agreement.
    ///
    /// - No agreement on the bucket: fails with [`AuthError::NoAgreement`].
    /// - Writer or Admin once the agreement has expired: fails with
    ///   [`AuthError::AgreementExpired`]. Reader passes until the agreement
    ///   is removed from chain.
    pub async fn require_agreement(
        &self,
        bucket_id: BucketId,
        required: Role,
        anchor_block: u32,
    ) -> Result<LocalBucketAgreement, AuthError> {
        let agreement = self
            .membership
            .lookup(bucket_id)
            .await?
            .access
            .local_agreement
            .clone()
            .ok_or(AuthError::NoAgreement(bucket_id))?;
        if required != Role::Reader && !agreement.is_writable_at(anchor_block) {
            return Err(AuthError::AgreementExpired(bucket_id));
        }
        Ok(agreement)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::error::MembershipError;
    use crate::http_auth::build_auth_header;
    use crate::membership::{
        AgreementRole, BucketAccess, Member, MembershipResolver, StaticMembershipResolver,
    };
    use crate::test_support::{AgreementResolver, FlakyResolver};
    use sp_core::Pair;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::Arc;
    use std::time::Duration;

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
            "Web3Storage {}:{}:{}",
            array_bytes::bytes2hex("0x", keypair.public().0),
            array_bytes::bytes2hex("0x", sig.0),
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
        Authenticator::new(StaticMembershipResolver::private(members))
    }

    #[tokio::test]
    async fn require_agreement_reports_the_resolved_agreement() {
        let with_agreement =
            Authenticator::new(StaticMembershipResolver::private(vec![]).with_max_bytes(1_000));
        assert_eq!(
            with_agreement
                .require_agreement(1, Role::Reader, 0)
                .await
                .unwrap(),
            LocalBucketAgreement {
                role: AgreementRole::Primary,
                max_bytes: 1_000,
                expires_at: u32::MAX
            }
        );

        let without =
            Authenticator::new(StaticMembershipResolver::private(vec![]).without_agreement());
        assert!(matches!(
            without.require_agreement(1, Role::Reader, 0).await,
            Err(AuthError::NoAgreement(1))
        ));
    }

    /// Expiry stops writes only: a read at or past `expires_at` still gets
    /// the agreement.
    #[tokio::test]
    async fn require_agreement_rejects_writes_once_expired() {
        let auth =
            Authenticator::new(StaticMembershipResolver::private(vec![]).with_expires_at(100));

        assert!(auth.require_agreement(1, Role::Writer, 99).await.is_ok());
        assert!(matches!(
            auth.require_agreement(1, Role::Writer, 100).await,
            Err(AuthError::AgreementExpired(1))
        ));
        assert!(matches!(
            auth.require_agreement(1, Role::Admin, 101).await,
            Err(AuthError::AgreementExpired(1))
        ));
        assert!(auth.require_agreement(1, Role::Reader, 101).await.is_ok());
    }

    #[tokio::test]
    async fn require_agreement_is_served_from_the_cache() {
        let calls = Arc::new(AtomicUsize::new(0));
        let auth = Authenticator::new(AgreementResolver {
            max_bytes: 5,
            calls: calls.clone(),
        });

        auth.require_agreement(1, Role::Reader, 0).await.unwrap();
        let agreement = auth.require_agreement(1, Role::Reader, 0).await.unwrap();

        assert_eq!(agreement.max_bytes, 5);
        assert_eq!(calls.load(Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn public_bucket_serves_reads_without_auth_but_still_gates_writes() {
        let alice = sr25519::Pair::from_string("//Alice", None).unwrap();
        let auth = Authenticator::new(StaticMembershipResolver::new(BucketAccess {
            members: vec![(AccountId32::new(alice.public().0), Role::Reader).into()],
            visibility: Visibility::Public,
            local_agreement: None,
        }));

        let anonymous_read = auth.require_role(None, "GET", 1, Role::Reader).await;
        assert!(anonymous_read.is_ok(), "got {anonymous_read:?}");

        let anonymous_write = auth.require_role(None, "PUT", 1, Role::Writer).await;
        assert!(matches!(anonymous_write, Err(AuthError::AuthRequired)));

        let header = make_auth_header(&alice, "PUT", 1, current_timestamp());
        let reader_write = auth
            .require_role(Some(&header), "PUT", 1, Role::Writer)
            .await;
        assert!(matches!(reader_write, Err(AuthError::InsufficientRole)));
    }

    #[tokio::test]
    async fn replica_served_bucket_reads_anonymously_but_writes_stay_gated() {
        // Design rule: replicas serve everyone — visibility gates primaries
        // only. A private bucket this node replicates must answer anonymous
        // reads, while writes are rejected because a replica takes data by sync.
        let auth =
            Authenticator::new(StaticMembershipResolver::private(vec![]).with_serves_as_replica());

        let anonymous_read = auth.require_role(None, "GET", 1, Role::Reader).await;
        assert!(anonymous_read.is_ok(), "got {anonymous_read:?}");

        let anonymous_write = auth.require_role(None, "PUT", 1, Role::Writer).await;
        assert!(matches!(anonymous_write, Err(AuthError::NotPrimary)));
    }

    #[tokio::test]
    async fn replica_bucket_rejects_writer_and_admin_requests_even_from_members() {
        let alice = sr25519::Pair::from_string("//Alice", None).unwrap();
        let alice_account = AccountId32::new(alice.public().0);
        let members = |role| vec![(alice_account.clone(), role).into()];

        for (role, required) in [
            (Role::Writer, Role::Writer),
            (Role::Admin, Role::Writer),
            (Role::Admin, Role::Admin),
        ] {
            let auth = Authenticator::new(
                StaticMembershipResolver::private(members(role)).with_serves_as_replica(),
            );
            let header = make_auth_header(&alice, "PUT", 1, current_timestamp());
            let signed = auth.require_role(Some(&header), "PUT", 1, required).await;
            assert!(
                matches!(signed, Err(AuthError::NotPrimary)),
                "{role:?} as {required:?}: got {signed:?}"
            );
        }
    }

    #[tokio::test]
    async fn primary_bucket_still_accepts_member_writes() {
        let alice = sr25519::Pair::from_string("//Alice", None).unwrap();
        let auth = Authenticator::new(
            StaticMembershipResolver::private(vec![(
                AccountId32::new(alice.public().0),
                Role::Writer,
            )
                .into()])
            .with_max_bytes(1_000),
        );
        let header = make_auth_header(&alice, "PUT", 1, current_timestamp());

        let write = auth
            .require_role(Some(&header), "PUT", 1, Role::Writer)
            .await;
        assert!(write.is_ok(), "got {write:?}");
    }

    #[tokio::test]
    async fn provider_account_reads_any_bucket_but_cannot_write() {
        // The node's own provider account is never a bucket member, yet its
        // tooling must fetch challenge proofs — Reader passes, Writer doesn't.
        let provider = sr25519::Pair::from_string("//Provider", None).unwrap();
        let provider_account = AccountId32::new(provider.public().0);
        let auth = Authenticator::new(StaticMembershipResolver::private(vec![]))
            .with_provider_account(provider_account);

        let header = make_auth_header(&provider, "GET", 1, current_timestamp());
        let read = auth
            .require_role(Some(&header), "GET", 1, Role::Reader)
            .await;
        assert!(read.is_ok(), "got {read:?}");

        let header = make_auth_header(&provider, "PUT", 1, current_timestamp());
        let write = auth
            .require_role(Some(&header), "PUT", 1, Role::Writer)
            .await;
        assert!(matches!(write, Err(AuthError::InsufficientRole)));
    }

    #[tokio::test]
    async fn provider_account_reads_survive_a_chain_outage() {
        // FlakyResolver fails from the second lookup on; with zero ttl and
        // max_stale the membership entry is unusable — the operator's own
        // reads must still pass, since they never consult membership.
        let provider = sr25519::Pair::from_string("//Provider", None).unwrap();
        let provider_account = AccountId32::new(provider.public().0);
        let calls = Arc::new(AtomicUsize::new(0));
        let auth = Authenticator::new(FlakyResolver {
            account: provider_account.clone(),
            calls,
        })
        .with_ttl(Duration::ZERO)
        .with_max_stale(Duration::ZERO)
        .with_provider_account(provider_account);

        for _ in 0..2 {
            let header = make_auth_header(&provider, "GET", 1, current_timestamp());
            let read = auth
                .require_role(Some(&header), "GET", 1, Role::Reader)
                .await;
            assert!(read.is_ok(), "got {read:?}");
        }
    }

    #[tokio::test]
    async fn private_bucket_read_requires_auth() {
        let alice = sr25519::Pair::from_string("//Alice", None).unwrap();
        let auth = authenticator(vec![
            (AccountId32::new(alice.public().0), Role::Reader).into()
        ]);
        let anonymous_read = auth.require_role(None, "GET", 1, Role::Reader).await;
        assert!(matches!(anonymous_read, Err(AuthError::AuthRequired)));
    }

    async fn require_role_for(granted: Role, required: Role) -> Result<(), AuthError> {
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
            (Role::Reader, Role::Reader, true),
            (Role::Reader, Role::Writer, false),
            (Role::Reader, Role::Admin, false),
            (Role::Writer, Role::Reader, true),
            (Role::Writer, Role::Writer, true),
            (Role::Writer, Role::Admin, false),
            (Role::Admin, Role::Reader, true),
            (Role::Admin, Role::Writer, true),
            (Role::Admin, Role::Admin, true),
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
                Box::new(StaticMembershipResolver::private(vec![member]))
            } else {
                Box::new(StaticMembershipResolver::private(vec![]))
            }
        }

        let keypair = sr25519::Pair::from_string("//Alice", None).unwrap();
        let account = AccountId32::new(keypair.public().0);
        let resolver = pick(true, (account, Role::Admin).into());

        let auth = Authenticator::new(resolver);
        let header = make_auth_header(&keypair, "PUT", 1, current_timestamp());

        assert!(auth
            .require_role(Some(&header), "PUT", 1, Role::Admin)
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
            .require_role(Some(&header), "GET", 1, Role::Reader)
            .await;
        assert!(matches!(result, Err(AuthError::InsufficientRole)));
    }

    #[tokio::test]
    async fn require_role_rejects_unsigned_and_forged_requests() {
        let alice = sr25519::Pair::from_string("//Alice", None).unwrap();
        let auth = authenticator(vec![
            (AccountId32::new(alice.public().0), Role::Admin).into()
        ]);

        let missing = auth.require_role(None, "GET", 1, Role::Reader).await;
        assert!(matches!(missing, Err(AuthError::AuthRequired)));

        // Header signed for bucket 1 must not authorize bucket 2, even though
        // the signer is an Admin of the bucket it did sign for.
        let header = make_auth_header(&alice, "GET", 1, current_timestamp());
        let replayed = auth
            .require_role(Some(&header), "GET", 2, Role::Reader)
            .await;
        assert!(matches!(replayed, Err(AuthError::AuthRequired)));
    }

    #[tokio::test]
    async fn require_role_refuses_membership_past_max_stale_on_chain_failure() {
        // With max_stale zero, once the chain fails on the second lookup the
        // cached entry - however briefly held - is already past the bound.
        // `require_role` must surface the exact variant the provider node
        // maps to `503 membership_unavailable`, rather than authorizing from
        // an unboundedly stale snapshot.
        let keypair = sr25519::Pair::from_string("//Alice", None).unwrap();
        let account = AccountId32::new(keypair.public().0);
        let calls = Arc::new(AtomicUsize::new(0));
        let auth = Authenticator::new(FlakyResolver {
            account: account.clone(),
            calls: calls.clone(),
        })
        .with_ttl(Duration::ZERO)
        .with_max_stale(Duration::ZERO);
        let header = make_auth_header(&keypair, "GET", 1, current_timestamp());

        // Seeds the cache via the resolver's one successful call.
        auth.require_role(Some(&header), "GET", 1, Role::Reader)
            .await
            .expect("first lookup must succeed and cache the member set");
        assert_eq!(calls.load(Ordering::SeqCst), 1);

        let result = auth
            .require_role(Some(&header), "GET", 1, Role::Reader)
            .await;
        assert!(
            matches!(
                result,
                Err(AuthError::MembershipLookup(MembershipError::Unavailable(_)))
            ),
            "an entry past max_stale must surface Unavailable (-> 503), got {result:?}"
        );
    }

    #[tokio::test]
    async fn with_max_stale_widens_the_bound_the_ttl_would_have_set() {
        // Same setup as the test above, which refuses with max_stale zero.
        // Raising it via the builder must let the brief outage through -
        // otherwise `--auth-max-stale` is decorative and the flag silently
        // does nothing.
        let keypair = sr25519::Pair::from_string("//Alice", None).unwrap();
        let account = AccountId32::new(keypair.public().0);
        let calls = Arc::new(AtomicUsize::new(0));
        let auth = Authenticator::new(FlakyResolver {
            account: account.clone(),
            calls: calls.clone(),
        })
        .with_ttl(Duration::ZERO)
        .with_max_stale(Duration::from_secs(300));
        let header = make_auth_header(&keypair, "GET", 1, current_timestamp());

        auth.require_role(Some(&header), "GET", 1, Role::Reader)
            .await
            .expect("first lookup must succeed and cache the member set");

        let result = auth
            .require_role(Some(&header), "GET", 1, Role::Reader)
            .await;
        assert!(
            result.is_ok(),
            "within the configured max_stale the cached role must still authorize, got {result:?}"
        );
        assert_eq!(calls.load(Ordering::SeqCst), 2);
    }
}
