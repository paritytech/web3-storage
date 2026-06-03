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
//! 3. Signs `blake2_256(SCALE(terms))` with the provider's existing
//!    sr25519 checkpoint key (the same one used to sign commitments).

use codec::Encode;
use sp_core::Pair;
use sp_runtime::MultiSignature;
use std::sync::atomic::{AtomicU64, Ordering};

// Wire types are shared with the SDK so client + server agree on serde shape.
pub use storage_client::agreement::{AgreementTermsOf, NegotiateRequest, SignedTerms};

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
/// Mirrors the on-chain verifier: SCALE-encode → blake2-256 → sr25519
/// sign → wrap as `MultiSignature::Sr25519`.
pub fn sign_terms(keypair: &sp_core::sr25519::Pair, terms: &AgreementTermsOf) -> MultiSignature {
    let hash = sp_core::hashing::blake2_256(&terms.encode());
    let sig = keypair.sign(&hash);
    MultiSignature::Sr25519(sig)
}

#[cfg(test)]
mod tests {
    use super::*;

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
