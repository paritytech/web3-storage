// SPDX-License-Identifier: GPL-3.0-only

//! Off-chain terms negotiation — provider-signed [`AgreementTerms`].
//!
//! Bucket owners ask the provider node for signed terms via
//! `POST /negotiate`. The provider node:
//!
//! 1. Allocates a fresh nonce from an in-memory monotonic counter
//!    ([`provider_coordinator::NonceCounter`]). The chain-state coordinator aligns the counter with the
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
use provider_coordinator::ProviderInfo;

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
