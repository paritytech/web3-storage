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
//! The on-chain pallet hashes `blake2_256(SCALE(terms))` and verifies the
//! signature against the provider's registered public key, so the same
//! encoding has to be used on both sides — `sign_terms` enforces that.

use codec::Encode;
use serde::{Deserialize, Serialize, Deserializer};
use sp_core::hashing::blake2_256;
use sp_runtime::{AccountId32, MultiSignature};
use storage_primitives::AgreementTerms;

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
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NegotiateRequest {
    /// Account that will own the resulting bucket.
    pub owner: AccountId32,
    /// Storage quota requested, in bytes.
    /// FIX: Safely handles the JS BigInt sent as a string
    #[serde(deserialize_with = "deserialize_number_from_string_or_number")]
    pub max_bytes: u64,
    /// Agreement duration in blocks from activation.
    pub duration: u32,
    /// Price per byte per block the owner is willing to lock in.
    /// FIX: Safely handles the JS BigInt sent as a string
    #[serde(deserialize_with = "deserialize_number_from_string_or_number")]
    pub price_per_byte: u128,
    /// `Some(_)` to negotiate a replica agreement (per-sync funding +
    /// minimum sync interval); `None` for a primary agreement.
    pub replica_params: Option<ReplicaTermsOf>,
}

/// Provider-signed agreement terms
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
pub fn sign_terms(
    keypair: &subxt_signer::sr25519::Keypair,
    terms: &AgreementTermsOf,
) -> MultiSignature {
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


// Universal helper function to accept either a JSON string or raw JSON number
fn deserialize_number_from_string_or_number<'de, T, D>(deserializer: D) -> Result<T, D::Error>
where
    D: Deserializer<'de>,
    T: std::str::FromStr + Deserialize<'de>,
    <T as std::str::FromStr>::Err: std::fmt::Display,
{
    #[derive(Deserialize)]
    #[serde(untagged)]
    enum StringOrNumber<T> {
        String(String),
        Number(T),
    }

    match StringOrNumber::deserialize(deserializer)? {
        StringOrNumber::String(s) => s.parse::<T>().map_err(serde::de::Error::custom),
        StringOrNumber::Number(n) => Ok(n),
    }
}