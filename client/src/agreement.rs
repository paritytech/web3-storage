// SPDX-License-Identifier: Apache-2.0

//! Provider-signed agreement terms — wire format + client-side signing helper.
//!
//! * Runtime-specific [`AgreementTermsOf`] alias the client uses to
//!   talk to the parachain (AccountId32 / u128 / u32).
//! * [`SignedTerms`] — the negotiated bundle returned over HTTP by a
//!   provider node's `/negotiate` endpoint.
//! * [`NegotiateRequest`] — the JSON body that bucket owners POST to
//!   `/negotiate`.
//! * [`sign_terms`] — a helper for provider-side code (tests, fixtures)
//!   that need to sign terms without going through the full provider
//!   keystore.
//!
//! The on-chain pallet hashes `blake2_256(TERM_CONTEXT | SCALE(terms))` —
//! `primary-term-v1:` or `replica-term-v1:` depending on the redemption
//! path — and verifies the signature against the provider's registered
//! public key, so the same payload has to be built on both sides —
//! `sign_terms` enforces that via [`AgreementTerms::signing_payload`].

use serde::{Deserialize, Serialize};
use serde_with::{serde_as, DisplayFromStr, PickFirst};
use storage_primitives::{AgreementTerms, BucketId, ReplicaTerms};
use storage_subxt::api::runtime_types::sp_runtime::MultiSignature;
use storage_subxt::subxt::utils::AccountId32;
use storage_subxt::subxt_signer;

/// Concrete [`AgreementTerms`] type for the storage parachain.
///
/// Balance is `u128`, BlockNumber is `u32`; matches
/// types used by runtime.
pub type AgreementTermsOf = AgreementTerms<AccountId32, u128, u32>;

/// Concrete `ReplicaTerms` matching the parachain's
/// `(Balance, BlockNumber) = (u128, u32)`.
pub type ReplicaTermsOf = ReplicaTerms<u128, u32>;

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

/// Sign already-built terms with a provider keypair.
pub fn sign_terms(
    keypair: &subxt_signer::sr25519::Keypair,
    terms: &AgreementTermsOf,
) -> MultiSignature {
    let hash = sp_core::hashing::blake2_256(&terms.signing_payload());
    let raw = keypair.sign(&hash);
    MultiSignature::Sr25519(raw.0)
}

/// Hex-bytes serde adapter for [`MultiSignature`] — SCALE-encode then hex.
mod hex_multi_signature {
    use super::MultiSignature;
    use codec::{Decode, Encode};
    use serde::{Deserialize, Deserializer, Serializer};

    pub fn serialize<S: Serializer>(sig: &MultiSignature, s: S) -> Result<S::Ok, S::Error> {
        s.serialize_str(&hex::encode(sig.encode()))
    }

    pub fn deserialize<'de, D: Deserializer<'de>>(d: D) -> Result<MultiSignature, D::Error> {
        let s = String::deserialize(d)?;
        let bytes = hex::decode(&s).map_err(serde::de::Error::custom)?;
        MultiSignature::decode(&mut &bytes[..]).map_err(serde::de::Error::custom)
    }
}
