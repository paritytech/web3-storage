//! Off-chain terms negotiation — provider-signed [`AgreementTerms`].
//!
//! Bucket owners ask the provider node for signed terms via
//! `POST /negotiate`. The provider node:
//!
//! 1. Allocates a fresh nonce from an in-memory monotonic counter
//!    ([`NonceCounter`]). The counter is initialized at startup from the
//!    chain's `ProviderReplayState.hsn + 1`, so a restart can't reissue a
//!    nonce the chain already accepted (the on-chain replay window is
//!    authoritative and rejects any out-of-range reuse).
//! 2. Builds [`AgreementTerms`] from the request, the provider's current
//!    `price_per_byte` setting (read from chain), and
//!    `valid_until = current_block + valid_until_offset`.
//! 3. Signs `blake2_256(TERM_CONTEXT | SCALE(terms))` with the provider's
//!    existing sr25519 checkpoint key (the same one used to sign
//!    commitments). The context is `primary-term-v1:` or
//!    `replica-term-v1:` depending on the quote's flavour.

use crate::error::Error;
use sp_core::Pair;
use sp_runtime::MultiSignature;
use std::sync::atomic::{AtomicU64, Ordering};
use storage_client::discovery::ProviderInfo;

// Wire types are shared with the SDK so client + server agree on serde shape.
pub use storage_client::agreement::{AgreementTermsOf, NegotiateRequest, SignedTerms};

/// Validate a negotiation request against the provider's current on-chain
/// settings.
///
/// The chain treats the resulting signature as provider consent, so the
/// node must refuse to sign terms it wouldn't accept: without this check a
/// client could propose `price_per_byte = 0`, an out-of-range duration, or
/// more bytes than the provider has capacity for, and the extrinsic would
/// bind the provider to it.
pub fn validate_request(req: &NegotiateRequest, info: &ProviderInfo) -> Result<(), Error> {
    match &req.replica_params {
        None if !info.accepting_primary => return Err(Error::NotAcceptingPrimary),
        Some(_) if info.replica_sync_price.is_none() => return Err(Error::NotAcceptingReplicas),
        _ => {}
    }

    if req.price_per_byte < info.price_per_byte {
        return Err(Error::PriceBelowListed {
            proposed: req.price_per_byte,
            listed: info.price_per_byte,
        });
    }

    if req.duration < info.min_duration || req.duration > info.max_duration {
        return Err(Error::DurationOutOfBounds {
            duration: req.duration,
            min: info.min_duration,
            max: info.max_duration,
        });
    }

    // `max_capacity == 0` means unlimited.
    if info.max_capacity > 0
        && info.committed_bytes.saturating_add(req.max_bytes) > info.max_capacity
    {
        return Err(Error::CapacityExceeded {
            requested: req.max_bytes,
            committed: info.committed_bytes,
            max_capacity: info.max_capacity,
        });
    }

    Ok(())
}

/// In-memory monotonic nonce counter for provider-signed terms.
///
/// Nonces are atomically allocated via [`Self::next`]. There is no local
/// persistence: at startup the caller reconciles against the chain by
/// calling [`Self::bootstrap_from_hsn`] with the provider's on-chain
/// `hsn`, so the counter resumes at `hsn + 1`. This:
///
/// * survives a restart (the chain hsn is the source of truth);
/// * survives a restart where the chain advanced past our last view
///   (e.g. a parallel quote was redeemed elsewhere) — we skip past it
///   rather than reissue.
///
/// Gap-skipping is fine: unused nonces just expire from the replay
/// window without effect. The on-chain replay window is authoritative
/// and rejects any out-of-range reuse, so a missed nonce can never lead
/// to a double redemption.
#[derive(Debug)]
pub struct NonceCounter {
    counter: AtomicU64,
}

impl NonceCounter {
    /// Create a counter starting at `start`. In normal operation the
    /// caller follows up with [`Self::bootstrap_from_hsn`] to align with
    /// the chain.
    pub fn new(start: u64) -> Self {
        Self {
            counter: AtomicU64::new(start),
        }
    }

    /// Advance the counter to at least `hsn + 1`. Idempotent — only
    /// advances forward.
    pub fn bootstrap_from_hsn(&self, hsn: u64) {
        let target = hsn.saturating_add(1);
        // Standard CAS loop — bump only if our target is higher than
        // whatever is already there.
        let mut current = self.counter.load(Ordering::SeqCst);
        while current < target {
            match self.counter.compare_exchange_weak(
                current,
                target,
                Ordering::SeqCst,
                Ordering::SeqCst,
            ) {
                Ok(_) => break,
                Err(observed) => current = observed,
            }
        }
    }

    /// Allocate the next nonce. Atomic: concurrent callers each get a
    /// distinct value.
    pub fn next(&self) -> u64 {
        self.counter.fetch_add(1, Ordering::SeqCst)
    }
}

/// Sign agreement terms with the provider's checkpoint sr25519 key.
///
/// Mirrors the on-chain verifier: term context | SCALE-encode →
/// blake2-256 → sr25519 sign → wrap as `MultiSignature::Sr25519`.
pub fn sign_terms(keypair: &sp_core::sr25519::Pair, terms: &AgreementTermsOf) -> MultiSignature {
    let hash = sp_core::hashing::blake2_256(&terms.signing_payload());
    let sig = keypair.sign(&hash);
    MultiSignature::Sr25519(sig)
}

#[cfg(test)]
mod tests {
    use super::*;
    use sp_runtime::AccountId32;
    use storage_client::agreement::ReplicaTermsOf;

    fn provider_info() -> ProviderInfo {
        ProviderInfo {
            multiaddr: "/ip4/127.0.0.1/tcp/3333".to_string(),
            stake: 1_000_000_000_000,
            committed_bytes: 0,
            max_capacity: 0,
            min_duration: 10,
            max_duration: 100_000,
            price_per_byte: 5,
            accepting_primary: true,
            replica_sync_price: None,
            accepting_extensions: true,
            agreements_total: 0,
            challenges_failed: 0,
        }
    }

    fn request() -> NegotiateRequest {
        NegotiateRequest {
            owner: AccountId32::new([0u8; 32]),
            max_bytes: 1024,
            duration: 50,
            price_per_byte: 5,
            bucket_id: None,
            replica_params: None,
        }
    }

    #[test]
    fn accepts_request_matching_settings() {
        assert!(validate_request(&request(), &provider_info()).is_ok());
    }

    #[test]
    fn rejects_price_below_listed() {
        let mut req = request();
        req.price_per_byte = 0;
        let err = validate_request(&req, &provider_info()).unwrap_err();
        assert!(matches!(
            err,
            Error::PriceBelowListed {
                proposed: 0,
                listed: 5
            }
        ));
    }

    #[test]
    fn accepts_price_above_listed() {
        let mut req = request();
        req.price_per_byte = 10;
        assert!(validate_request(&req, &provider_info()).is_ok());
    }

    #[test]
    fn rejects_duration_out_of_bounds() {
        let mut req = request();
        req.duration = 5;
        assert!(matches!(
            validate_request(&req, &provider_info()),
            Err(Error::DurationOutOfBounds { .. })
        ));
        req.duration = 100_001;
        assert!(matches!(
            validate_request(&req, &provider_info()),
            Err(Error::DurationOutOfBounds { .. })
        ));
    }

    #[test]
    fn rejects_bytes_beyond_remaining_capacity() {
        let mut info = provider_info();
        info.max_capacity = 2048;
        info.committed_bytes = 1536;
        let err = validate_request(&request(), &info).unwrap_err();
        assert!(matches!(
            err,
            Error::CapacityExceeded {
                requested: 1024,
                committed: 1536,
                max_capacity: 2048
            }
        ));
    }

    #[test]
    fn zero_capacity_means_unlimited() {
        let mut req = request();
        req.max_bytes = u64::MAX;
        assert!(validate_request(&req, &provider_info()).is_ok());
    }

    #[test]
    fn rejects_primary_when_not_accepting() {
        let mut info = provider_info();
        info.accepting_primary = false;
        assert!(matches!(
            validate_request(&request(), &info),
            Err(Error::NotAcceptingPrimary)
        ));
    }

    #[test]
    fn rejects_replica_without_sync_price() {
        let mut req = request();
        req.bucket_id = Some(1);
        req.replica_params = Some(ReplicaTermsOf {
            sync_balance: 1_000,
            min_sync_interval: 10,
        });
        assert!(matches!(
            validate_request(&req, &provider_info()),
            Err(Error::NotAcceptingReplicas)
        ));
    }

    #[test]
    fn accepts_replica_even_when_primary_closed() {
        let mut info = provider_info();
        info.accepting_primary = false;
        info.replica_sync_price = Some(7);
        let mut req = request();
        req.bucket_id = Some(1);
        req.replica_params = Some(ReplicaTermsOf {
            sync_balance: 1_000,
            min_sync_interval: 10,
        });
        assert!(validate_request(&req, &info).is_ok());
    }

    #[test]
    fn nonce_counter_is_monotonic() {
        let c = NonceCounter::new(0);
        assert_eq!(c.next(), 0);
        assert_eq!(c.next(), 1);
        assert_eq!(c.next(), 2);
    }

    #[test]
    fn bootstrap_from_hsn_only_advances() {
        let c = NonceCounter::new(10);
        c.bootstrap_from_hsn(5); // lower than current — no-op
        assert_eq!(c.next(), 10);
        c.bootstrap_from_hsn(20); // higher — advance
        assert_eq!(c.next(), 21);
    }
}
