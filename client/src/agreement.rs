//! Provider-signed agreement terms — wire format + client-side signing helper.
//!
//! Bucket creation went from a request/accept handshake to a single
//! `establish_storage_agreement` redemption of provider-signed
//! [`AgreementTerms`]. This module holds:
//!
//! * The runtime-specific [`AgreementTermsOf`] alias the client uses to
//!   talk to the parachain (AccountId32 / u128 / u32).
//! * [`SignedTerms`] — the negotiated bundle returned over HTTP by a
//!   provider node's `/negotiate` endpoint.
//! * [`NegotiateRequest`] — the JSON body that bucket owners POST to
//!   `/negotiate`.
//! * [`sign_terms`] — a helper for provider-side code (tests, fixtures)
//!   that need to sign terms without going through the full provider
//!   keystore.
//!
//! The on-chain pallet hashes `blake2_256(SCALE(terms))` and verifies the
//! signature against the provider's registered public key, so the same
//! encoding has to be used on both sides — `sign_terms` enforces that.

use codec::Encode;
use serde::{Deserialize, Serialize};
use sp_core::hashing::blake2_256;
use sp_runtime::{AccountId32, MultiSignature};
use storage_primitives::AgreementTerms;

/// Concrete [`AgreementTerms`] type for the storage parachain.
///
/// Balance is `u128`, BlockNumber is `u32`; matches `parachains_common`'s
/// runtime types used by `storage_paseo_runtime`.
pub type AgreementTermsOf = AgreementTerms<AccountId32, u128, u32>;

/// Request body that a bucket owner POSTs to a provider node's
/// `/negotiate` endpoint.
///
/// The provider node turns this into [`AgreementTerms`] (filling in
/// `price_per_byte` from its own settings, choosing a `nonce`, and
/// resolving `valid_until = current_block + valid_until_offset`), signs
/// the SCALE encoding, and returns [`SignedTerms`].
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NegotiateRequest {
    /// The account that will own the resulting bucket / drive. Must
    /// match the `Signed` origin that later submits
    /// `establish_storage_agreement`.
    pub owner: AccountId32,
    /// Storage quota requested, in bytes.
    pub max_bytes: u64,
    /// Agreement duration in blocks from activation.
    pub duration: u32,
    /// How many blocks the resulting quote should be valid for. The
    /// provider chooses `valid_until = current_block + valid_until_offset`
    /// so the owner has a bounded window to redeem it on-chain.
    pub valid_until_offset: u32,
}

/// Provider-signed agreement terms ready to be redeemed via
/// `establish_storage_agreement`.
///
/// `signature` is a `MultiSignature` over `blake2_256(SCALE(terms))`,
/// produced by the provider's registered key. We carry the signature as
/// hex over the wire — `MultiSignature` doesn't derive serde directly,
/// and hex keeps the JSON readable.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SignedTerms {
    pub terms: AgreementTermsOf,
    #[serde(with = "hex_multi_signature")]
    pub signature: MultiSignature,
}

/// Sign already-built terms with a provider keypair.
///
/// Mirror of the on-chain `verify_terms_signature`: SCALE-encode, hash
/// with blake2-256, then sign. The runtime accepts `MultiSignature`, so
/// callers wrap the raw sr25519 signature with `MultiSignature::Sr25519`.
pub fn sign_terms(keypair: &subxt_signer::sr25519::Keypair, terms: &AgreementTermsOf) -> MultiSignature {
    let hash = blake2_256(&terms.encode());
    let raw = keypair.sign(&hash);
    MultiSignature::Sr25519(sp_core::sr25519::Signature::from_raw(raw.0))
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
