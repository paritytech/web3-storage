// SPDX-License-Identifier: GPL-3.0-only

//! Off-chain terms negotiation — provider-signed [`AgreementTerms`].
//!
//! Bucket owners ask the provider node for signed terms via
//! `POST /negotiate`. The provider node:
//!
//! 1. Allocates a fresh nonce from an in-memory monotonic counter
//!    ([`NonceCounter`]). The chain-state coordinator aligns the counter with the
//!    chain's `ProviderReplayState.hsn + 1` (on connect and on every relevant
//!    provider event), so a restart can't reissue a nonce the chain already
//!    accepted (the on-chain replay window is authoritative and rejects any
//!    out-of-range reuse).
//! 2. Builds [`AgreementTerms`] from the request, the provider's current
//!    `price_per_byte` setting (read from chain), and
//!    `valid_until = current_anchor_block + valid_until_offset`.
//! 3. Signs `blake2_256(TERM_CONTEXT | SCALE(terms))` with the provider's
//!    existing sr25519 checkpoint key (the same one used to sign
//!    commitments). The context is `primary-term-v1:` or
//!    `replica-term-v1:` depending on the quote's flavour.

use crate::error::Error;
use crate::types::ProviderInfo;
use provider_storage::{NonceStore, NullNonceStore};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::Arc;

// Wire types are shared with the SDK so client + server agree on serde shape.
pub use provider_negotiation::{AgreementTermsOf, NegotiateRequest, SignedTerms};

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

    if req.max_bytes == 0 {
        return Err(Error::CapacityExceeded {
            requested: req.max_bytes,
            committed: info.committed_bytes,
            max_capacity: info.max_capacity,
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

/// Monotonic nonce counter for provider-signed terms.
///
/// Nonces are atomically allocated via [`Self::next`]. The chain-state
/// coordinator aligns the counter with the chain's `ProviderReplayState.hsn + 1`
/// (on connect and on every relevant provider event), so the counter resumes at
/// `max(persisted_local, hsn + 1)`:
///
/// * **Local persistence** (disk mode): each allocation is persisted before
///   returning, so a **clean process restart** does not reissue nonces that were
///   signed but not yet redeemed. Power-loss/kernel-panic may lose the last write
///   (RocksDB WAL is not fsynced per allocation); in that case the counter falls
///   back to `chain_hsn + 1`, which is still safe — the chain's replay window
///   rejects any duplicate redemption.
/// * **Chain alignment**: `bootstrap_from_hsn` advances the counter past any
///   nonce the chain has already accepted, covering redemptions that happened
///   while the node was down or while we weren't watching.
///
/// Gap-skipping is fine: unused nonces just expire from the replay window
/// without effect. The on-chain replay window is authoritative and rejects
/// any out-of-range reuse, so a missed nonce can never lead to a double
/// redemption.
///
/// Until the first successful [`Self::bootstrap_from_hsn`] the counter has not
/// been reconciled with the chain, so `/negotiate` must not sign with it; query
/// [`Self::is_bootstrapped`] to gate that.
pub struct NonceCounter {
    counter: AtomicU64,
    /// Set once the counter has been aligned with the chain's replay window.
    bootstrapped: AtomicBool,
    store: Arc<dyn NonceStore>,
}

impl std::fmt::Debug for NonceCounter {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("NonceCounter")
            .field("counter", &self.counter)
            .field("bootstrapped", &self.bootstrapped)
            .finish()
    }
}

impl NonceCounter {
    /// Create a counter starting at `start` with a no-op store.
    ///
    /// All existing call sites use this constructor; no persistence occurs
    /// (in-memory mode, or tests). The counter is *not* considered bootstrapped
    /// until [`Self::bootstrap_from_hsn`] aligns it with the chain.
    pub fn new(start: u64) -> Self {
        Self {
            counter: AtomicU64::new(start),
            bootstrapped: AtomicBool::new(false),
            store: Arc::new(NullNonceStore),
        }
    }

    /// Create a counter starting at `start` backed by `store` for persistence.
    ///
    /// Use this in disk mode: seed `start` from `store.load().unwrap_or(1)`
    /// (the persisted high-water mark), then call `bootstrap_from_hsn` to
    /// advance past the chain's replay head.
    pub fn with_store(start: u64, store: Arc<dyn NonceStore>) -> Self {
        Self {
            counter: AtomicU64::new(start),
            bootstrapped: AtomicBool::new(false),
            store,
        }
    }

    /// Whether the counter has been reconciled with the chain's replay window
    /// at least once. `/negotiate` gates on this so it never signs a nonce
    /// that was not derived from on-chain state.
    pub fn is_bootstrapped(&self) -> bool {
        self.bootstrapped.load(Ordering::SeqCst)
    }

    /// Advance the counter to at least `hsn + 1` and mark it bootstrapped.
    /// Idempotent — only advances forward.
    pub fn bootstrap_from_hsn(&self, hsn: u64) {
        self.bootstrapped.store(true, Ordering::SeqCst);
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

    /// Allocate the next nonce. Atomic: concurrent callers each get a distinct
    /// value.
    ///
    /// The value *after* the increment (`nonce + 1`) is persisted as the new
    /// high-water mark before the nonce is returned. This means the persisted
    /// value always equals the next nonce that *will* be issued, so a counter
    /// seeded with `store.load().unwrap_or(1)` on restart correctly resumes
    /// above every nonce that was signed.
    pub fn next(&self) -> u64 {
        let nonce = self.counter.fetch_add(1, Ordering::SeqCst);
        self.store.persist(nonce.saturating_add(1));
        nonce
    }
}
