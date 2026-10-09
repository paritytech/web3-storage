// SPDX-License-Identifier: GPL-3.0-only

//! Off-chain terms negotiation — provider-signed [`AgreementTerms`].
//!
//! Bucket owners ask the provider node for signed terms via
//! `POST /negotiate`, including the nonce they expect to redeem the quote at
//! (their next expected value in the pallet's per-owner agreement nonce). The
//! provider node:
//!
//! 1. Builds [`AgreementTerms`] from the request, the provider's current
//!    `price_per_byte` setting (read from chain), and
//!    `valid_until = current_anchor_block + valid_until_offset`.
//! 2. Signs `blake2_256(TERM_CONTEXT | SCALE(terms))` with the provider's
//!    signing keypair (the same one used to sign commitments; any
//!    [`provider_types::KeyScheme`]). The context is `primary-term-v1:` or
//!    `replica-term-v1:` depending on the quote's flavour.

use crate::error::Error;
use provider_types::ProviderInfo;

// Wire types are shared with the SDK so client + server agree on serde shape.
pub use provider_negotiation::{AgreementTermsOf, NegotiateRequest, SignedTerms};

/// Validate a negotiation request against the provider's current on-chain
/// settings.
///
/// The chain treats the resulting signature as provider consent, so the
/// node must refuse to sign terms it wouldn't accept: without this check a
/// client could propose `price_per_byte = 0`, an out-of-range duration, or
/// fewer bytes than the provider's minimum, more bytes than the provider has capacity for, and the extrinsic would
/// bind the provider to it.
pub fn validate_request(req: &NegotiateRequest, info: &ProviderInfo) -> Result<(), Error> {
    match &req.replica_params {
        None if !info.settings.accepting_primary => return Err(Error::NotAcceptingPrimary),
        Some(_) if info.settings.replica_sync_price.is_none() => {
            return Err(Error::NotAcceptingReplicas)
        }
        // Only `add_replica_provider` redeems a replica quote, and it needs
        // an existing bucket; the pallet rejects the quote otherwise.
        Some(_) if req.bucket.is_none() => return Err(Error::ReplicaRequiresBucket),
        _ => {}
    }

    if req.price_per_byte < info.settings.price_per_byte {
        return Err(Error::PriceBelowListed {
            proposed: req.price_per_byte,
            listed: info.settings.price_per_byte,
        });
    }

    if req.duration < info.settings.min_duration || req.duration > info.settings.max_duration {
        return Err(Error::DurationOutOfBounds {
            duration: req.duration,
            min: info.settings.min_duration,
            max: info.settings.max_duration,
        });
    }

    if req.max_bytes == 0 {
        return Err(Error::InvalidMaxBytesRequest);
    }

    if req.max_bytes < info.settings.min_bytes {
        return Err(Error::MaxBytesBelowMinimum {
            requested: req.max_bytes,
            min_bytes: info.settings.min_bytes,
        });
    }

    // `max_capacity == 0` means unlimited.
    if info.settings.max_capacity > 0
        && info.committed_bytes.saturating_add(req.max_bytes) > info.settings.max_capacity
    {
        return Err(Error::CapacityExceeded {
            requested: req.max_bytes,
            committed: info.committed_bytes,
            max_capacity: info.settings.max_capacity,
        });
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use provider_types::{ProviderSettings, ProviderStats};
    use sp_runtime::AccountId32;

    fn info(min_bytes: u64) -> ProviderInfo {
        ProviderInfo {
            multiaddr: "/ip4/127.0.0.1/tcp/3333".to_string(),
            public_key: vec![0; 32],
            stake: 1_000_000,
            committed_bytes: 0,
            settings: ProviderSettings {
                min_duration: 10,
                max_duration: 100,
                price_per_byte: 5,
                accepting_primary: true,
                replica_sync_price: None,
                accepting_extensions: true,
                max_capacity: 0,
                min_bytes,
            },
            stats: ProviderStats::default(),
            deregister_at: None,
        }
    }

    fn request(max_bytes: u64) -> NegotiateRequest {
        NegotiateRequest {
            owner: AccountId32::new([7u8; 32]),
            max_bytes,
            duration: 50,
            price_per_byte: 5,
            nonce: 0,
            bucket: None,
            replica_params: None,
        }
    }

    #[test]
    fn rejects_max_bytes_below_min_bytes() {
        let err = validate_request(&request(99), &info(100)).unwrap_err();
        assert!(matches!(
            err,
            Error::MaxBytesBelowMinimum {
                requested: 99,
                min_bytes: 100
            }
        ));
    }

    #[test]
    fn rejects_zero_max_bytes() {
        let err = validate_request(&request(0), &info(0)).unwrap_err();
        assert!(matches!(err, Error::InvalidMaxBytesRequest));
    }

    #[test]
    fn accepts_max_bytes_equal_to_min_bytes() {
        assert!(validate_request(&request(100), &info(100)).is_ok());
    }

    #[test]
    fn accepts_any_nonzero_max_bytes_when_min_bytes_is_zero() {
        assert!(validate_request(&request(1), &info(0)).is_ok());
    }
}
