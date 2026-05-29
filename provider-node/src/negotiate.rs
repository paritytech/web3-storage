//! Off-chain terms negotiation — provider-signed [`AgreementTerms`].
//!
//! Bucket owners ask the provider node for signed terms via
//! `POST /negotiate`. The provider node:
//!
//! 1. Allocates a fresh nonce from a persistent monotonic counter
//!    ([`NonceCounter`]). The counter is initialized at startup from the
//!    chain's `ProviderReplayState.hwm + 1`, so duplicates can't survive a
//!    restart that lost the local file (the on-chain replay window will
//!    reject them).
//! 2. Builds [`AgreementTerms`] from the request, the provider's current
//!    `price_per_byte` setting (read from chain), and
//!    `valid_until = current_block + valid_until_offset`.
//! 3. Signs `blake2_256(SCALE(terms))` with the provider's existing
//!    sr25519 checkpoint key (the same one used to sign commitments).

use codec::Encode;
use sp_core::Pair;
use sp_runtime::MultiSignature;
use std::fs;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};

// Wire types are shared with the SDK so client + server agree on serde shape.
pub use storage_client::agreement::{AgreementTermsOf, NegotiateRequest, SignedTerms};

/// Persistent monotonic nonce counter for provider-signed terms.
///
/// Nonces are atomically allocated via [`Self::next`]. Each allocation
/// writes the new value to `path` synchronously so a crash mid-handler
/// can't reissue the same nonce.
///
/// At startup, the caller should reconcile against the chain by calling
/// [`Self::bootstrap_from_hwm`] with the provider's on-chain `hwm`. The
/// counter then resumes at `max(local, hwm + 1)`, which:
///
/// * survives a restart that lost the local file (uses chain hwm);
/// * survives a restart where the chain advanced past our local view
///   (e.g. a parallel quote was redeemed elsewhere) — we skip past it
///   rather than reissue.
///
/// Gap-skipping is fine: unused nonces just expire from the replay
/// window without effect.
#[derive(Debug)]
pub struct NonceCounter {
    counter: AtomicU64,
    path: Option<PathBuf>,
}

impl NonceCounter {
    /// In-memory counter (testing). No persistence.
    pub fn in_memory(start: u64) -> Self {
        Self {
            counter: AtomicU64::new(start),
            path: None,
        }
    }

    /// Counter backed by a file. Reads the existing value if present,
    /// otherwise starts at 0.
    pub fn open(path: PathBuf) -> std::io::Result<Self> {
        let start = match fs::read_to_string(&path) {
            Ok(s) => s.trim().parse::<u64>().unwrap_or(0),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => 0,
            Err(e) => return Err(e),
        };
        Ok(Self {
            counter: AtomicU64::new(start),
            path: Some(path),
        })
    }

    /// Advance the counter to at least `hwm + 1`. Idempotent — only
    /// advances forward.
    pub fn bootstrap_from_hwm(&self, hwm: u64) {
        let target = hwm.saturating_add(1);
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
        self.persist();
    }

    /// Allocate the next nonce. Atomic + persistent: even concurrent
    /// callers each get a distinct value, and the on-disk file always
    /// trails (or matches) the highest issued nonce.
    pub fn next(&self) -> u64 {
        let n = self.counter.fetch_add(1, Ordering::SeqCst);
        self.persist();
        n
    }

    /// Best-effort persist of the *next* value to disk. Failures are
    /// logged but don't fail the call — the chain's replay window is
    /// authoritative anyway.
    fn persist(&self) {
        let Some(ref path) = self.path else { return };
        let next = self.counter.load(Ordering::SeqCst);
        if let Err(e) = fs::write(path, next.to_string()) {
            tracing::warn!("Failed to persist nonce counter to {:?}: {}", path, e);
        }
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
    fn nonce_counter_in_memory_is_monotonic() {
        let c = NonceCounter::in_memory(0);
        assert_eq!(c.next(), 0);
        assert_eq!(c.next(), 1);
        assert_eq!(c.next(), 2);
    }

    #[test]
    fn bootstrap_from_hwm_only_advances() {
        let c = NonceCounter::in_memory(10);
        c.bootstrap_from_hwm(5); // lower than current — no-op
        assert_eq!(c.next(), 10);
        c.bootstrap_from_hwm(20); // higher — advance
        assert_eq!(c.next(), 21);
    }

    #[test]
    fn persists_and_resumes_from_disk() {
        let dir = std::env::temp_dir().join(format!("nonce-counter-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("nonce");
        let c = NonceCounter::open(path.clone()).unwrap();
        assert_eq!(c.next(), 0);
        assert_eq!(c.next(), 1);
        drop(c);
        // Reopening should pick up where we left off.
        let c2 = NonceCounter::open(path).unwrap();
        assert_eq!(c2.next(), 2);
    }
}
