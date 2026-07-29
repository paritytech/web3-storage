// SPDX-License-Identifier: Apache-2.0

//! Provider-signed agreement terms — wire format + signing helper.
//!
//! Shared by the client SDK and the provider node so both sides of the
//! `/negotiate` endpoint agree on the wire format.
//!
//! * Runtime-specific [`AgreementTermsOf`] alias used to
//!   talk to the parachain (AccountId32 / u128 / u32).
//! * [`SignedTerms`] — the negotiated bundle returned over HTTP by a
//!   provider node's `/negotiate` endpoint.
//! * [`NegotiateRequest`] — the JSON body that bucket owners POST to
//!   `/negotiate`.
//! * [`sign_terms`] — a helper for provider-side code (tests, fixtures)
//!   that need to sign terms without going through the full provider
//!   keystore.
//! * [`http_auth`] — the signed `Authorization` header format for
//!   bucket-scoped provider HTTP requests (client builds, provider verifies).
//!
//! The on-chain pallet hashes `blake2_256(TERM_CONTEXT | SCALE(terms))` —
//! `primary-term-v1:` or `replica-term-v1:` depending on the redemption
//! path — and verifies the signature against the provider's registered
//! public key, so the same payload has to be built on both sides —
//! `sign_terms` enforces that via [`AgreementTerms::signing_payload`].

pub mod http_auth;

pub use http_auth::{auth_message, build_auth_header};

use serde::{Deserialize, Serialize};
use serde_with::{serde_as, DisplayFromStr, PickFirst};
use sp_core::hashing::blake2_256;
use sp_runtime::{AccountId32, MultiSignature};
use storage_primitives::{AgreementTerms, BucketId};

/// Concrete [`AgreementTerms`] type for the storage parachain.
///
/// Balance is `u128`, BlockNumber is `u32`; matches
/// types used by runtime.
pub type AgreementTermsOf = AgreementTerms<AccountId32, u128, u32>;

/// Concrete `ReplicaTerms` matching the parachain's
/// `(Balance, BlockNumber) = (u128, u32)`.
pub type ReplicaTermsOf = storage_primitives::ReplicaTerms<u128, u32>;

/// The owner proposes the agreement shape they want; the provider node
/// allocates a fresh nonce and a validity window from its own state,
/// builds the full [`AgreementTermsOf`], signs it, and returns
/// [`SignedTerms`].
#[serde_as]
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NegotiateRequest {
    /// Account that will own the resulting bucket.
    pub owner: AccountId32,
    /// Storage quota requested, in bytes.
    /// FIX: Safely handles the JS BigInt sent as a string
    #[serde_as(as = "PickFirst<(DisplayFromStr, _)>")]
    pub max_bytes: u64,
    /// Agreement duration in blocks from activation.
    pub duration: u32,
    /// Price per byte per block the owner is willing to lock in.
    /// FIX: Safely handles the JS BigInt sent as a string
    #[serde_as(as = "PickFirst<(DisplayFromStr, _)>")]
    pub price_per_byte: u128,
    /// Bucket the quote is bound to.
    /// - `None` for primary terms;
    /// - `Some(id)` for replica terms — must match the bucket targeted by
    ///   the extrinsic.
    pub bucket_id: Option<BucketId>,
    /// `Some(_)` to negotiate a replica agreement (per-sync funding +
    /// minimum sync interval); `None` for a primary agreement.
    pub replica_params: Option<ReplicaTermsOf>,
}

/// Provider-signed agreement terms
///
/// `signature` is a `MultiSignature` over
/// `blake2_256(TERM_CONTEXT | SCALE(terms))`, produced by the provider's
/// registered key. We carry the signature as hex over the wire —
/// `MultiSignature` doesn't derive serde directly, and hex keeps the JSON
/// readable.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SignedTerms {
    pub terms: AgreementTermsOf,
    #[serde(with = "hex_multi_signature")]
    pub signature: MultiSignature,
}

/// Sign already-built terms with a provider keypair, returning them bundled
/// with the signature so the two can never be paired up wrongly.
///
/// The key's scheme must match the provider's registered on-chain key.
pub fn sign_terms<P>(keypair: &P, terms: AgreementTermsOf) -> SignedTerms
where
    P: sp_core::Pair,
    P::Signature: Into<MultiSignature>,
{
    let hash = blake2_256(&terms.signing_payload());
    let signature = keypair.sign(&hash).into();
    SignedTerms { terms, signature }
}

/// Hex-bytes serde adapter for [`MultiSignature`] — SCALE-encode then hex.
mod hex_multi_signature {
    use codec::{Decode, Encode};
    use serde::{Deserialize, Deserializer, Serializer};
    use sp_runtime::MultiSignature;

    pub fn serialize<S: Serializer>(sig: &MultiSignature, s: S) -> Result<S::Ok, S::Error> {
        s.serialize_str(&hex::encode(sig.encode()))
    }

    pub fn deserialize<'de, D: Deserializer<'de>>(d: D) -> Result<MultiSignature, D::Error> {
        let s = String::deserialize(d)?;
        let bytes = hex::decode(&s).map_err(serde::de::Error::custom)?;
        MultiSignature::decode(&mut &bytes[..]).map_err(serde::de::Error::custom)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // Rust clients serialize `max_bytes`/`price_per_byte` as raw JSON
    // numbers. Regression test: the previous untagged-enum deserializer
    // rejected any JSON number for the u128 field (serde's untagged
    // buffering has no 128-bit support), surfacing as a 422 from
    // `/negotiate` for every Rust caller.
    #[test]
    fn negotiate_request_roundtrips_rust_numbers() {
        let req = NegotiateRequest {
            owner: AccountId32::new([0u8; 32]),
            max_bytes: 1_000_000_000,
            duration: 500,
            price_per_byte: 1,
            bucket_id: None,
            replica_params: None,
        };
        let json = serde_json::to_string(&req).unwrap();
        let decoded: NegotiateRequest = serde_json::from_str(&json).unwrap();
        assert_eq!(decoded.max_bytes, req.max_bytes);
        assert_eq!(decoded.price_per_byte, req.price_per_byte);
    }

    // JS clients send BigInt fields as decimal strings (commit 17528eb).
    #[test]
    fn negotiate_request_accepts_js_bigint_strings() {
        let json = format!(
            r#"{{"owner":"{}","max_bytes":"1073741824","duration":50,"price_per_byte":"340282366920938463463374607431768211455","bucket_id":null,"replica_params":null}}"#,
            AccountId32::new([0u8; 32])
        );
        let decoded: NegotiateRequest = serde_json::from_str(&json).unwrap();
        assert_eq!(decoded.max_bytes, 1_073_741_824);
        assert_eq!(decoded.price_per_byte, u128::MAX);
    }
}
