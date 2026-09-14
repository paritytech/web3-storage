// SPDX-License-Identifier: Apache-2.0

use provider_negotiation::{AgreementTermsOf, SignedTerms};
use sp_core::crypto::ByteArray;
use sp_core::{ecdsa, ed25519, sr25519, Pair};
use sp_runtime::MultiSignature;

/// Signature scheme of the provider's signing keypair — the key registered
/// on-chain as `public_key` and verified by the pallet via `MultiSignature`.
/// The extrinsic-submission account stays sr25519 regardless (see
/// `ProviderState::with_seed_scheme`). `Eth` is ecdsa over keccak digests
/// with revive-style account derivation — what Ethereum wallets produce.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
#[cfg_attr(feature = "clap", derive(clap::ValueEnum))]
pub enum KeyScheme {
    #[default]
    Sr25519,
    Ed25519,
    Ecdsa,
    Eth,
}

/// The provider's signing keypair, scheme-tagged so every signature leaves
/// the node as a self-describing [`MultiSignature`].
#[allow(clippy::large_enum_variant)]
#[derive(Clone)]
pub enum ProviderKeypair {
    Sr25519(sr25519::Pair),
    Ed25519(ed25519::Pair),
    Ecdsa(ecdsa::Pair),
    Eth(ecdsa::KeccakPair),
}

impl ProviderKeypair {
    /// Derive from a SURI (e.g. `//Alice` or a mnemonic) for the given scheme.
    pub fn from_seed(seed: &str, scheme: KeyScheme) -> Result<Self, String> {
        fn derive<P: Pair>(seed: &str) -> Result<P, String> {
            P::from_string(seed, None).map_err(|e| format!("Failed to create keypair: {e:?}"))
        }
        Ok(match scheme {
            KeyScheme::Sr25519 => Self::Sr25519(derive(seed)?),
            KeyScheme::Ed25519 => Self::Ed25519(derive(seed)?),
            KeyScheme::Ecdsa => Self::Ecdsa(derive(seed)?),
            KeyScheme::Eth => Self::Eth(derive(seed)?),
        })
    }

    /// Sign a raw message, tagging the signature with its scheme.
    pub fn sign(&self, message: &[u8]) -> MultiSignature {
        match self {
            Self::Sr25519(pair) => MultiSignature::Sr25519(pair.sign(message)),
            Self::Ed25519(pair) => MultiSignature::Ed25519(pair.sign(message)),
            Self::Ecdsa(pair) => MultiSignature::Ecdsa(pair.sign(message)),
            Self::Eth(pair) => MultiSignature::Eth(pair.sign(message)),
        }
    }

    /// Raw public key bytes as registered on-chain: 32 for Sr25519/Ed25519,
    /// 33 (compressed) for Ecdsa/Eth.
    pub fn public_key_bytes(&self) -> Vec<u8> {
        match self {
            Self::Sr25519(pair) => pair.public().to_raw_vec(),
            Self::Ed25519(pair) => pair.public().to_raw_vec(),
            Self::Ecdsa(pair) => pair.public().to_raw_vec(),
            Self::Eth(pair) => pair.public().to_raw_vec(),
        }
    }

    /// Sign negotiated terms, bundling terms + scheme-tagged signature.
    ///
    /// The digest is the one `provider_negotiation` defines; signing goes
    /// through [`Self::sign`] rather than the generic helper there, which
    /// cannot cover `Eth` (upstream has no `From<KeccakSignature>` for
    /// `MultiSignature`).
    pub fn sign_terms(&self, terms: AgreementTermsOf) -> SignedTerms {
        let signature = self.sign(&sp_crypto_hashing::blake2_256(&terms.signing_payload()));
        SignedTerms { terms, signature }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The account the pallet will verify against, derived from the raw
    /// registered key the way `Pallet::expected_signer_account` does — the
    /// signature's variant picks the derivation.
    fn expected_signer(key: &[u8], sig: &MultiSignature) -> sp_runtime::AccountId32 {
        use sp_runtime::{traits::IdentifyAccount, MultiSigner};
        match sig {
            MultiSignature::Sr25519(_) => {
                MultiSigner::Sr25519(sr25519::Public::try_from(key).unwrap())
            }
            MultiSignature::Ed25519(_) => {
                MultiSigner::Ed25519(ed25519::Public::try_from(key).unwrap())
            }
            MultiSignature::Ecdsa(_) => MultiSigner::Ecdsa(ecdsa::Public::try_from(key).unwrap()),
            MultiSignature::Eth(_) => MultiSigner::Eth(ecdsa::KeccakPublic::try_from(key).unwrap()),
        }
        .into_account()
    }

    /// sign_terms produces, for every scheme, a signature the pallet's
    /// terms verification accepts: over `blake2_256(signing_payload())`,
    /// against the account derived from the raw registered key.
    #[test]
    fn sign_terms_round_trips_for_every_scheme() {
        use sp_runtime::traits::Verify;
        use sp_runtime::AccountId32;

        let terms = AgreementTermsOf {
            owner: AccountId32::new([7u8; 32]),
            max_bytes: 1024,
            duration: 50,
            price_per_byte: 5,
            valid_until: 100,
            nonce: 1,
            bucket_id: None,
            replica_params: None,
        };

        for scheme in [
            KeyScheme::Sr25519,
            KeyScheme::Ed25519,
            KeyScheme::Ecdsa,
            KeyScheme::Eth,
        ] {
            let keypair = ProviderKeypair::from_seed("//Alice", scheme).unwrap();
            let key = keypair.public_key_bytes();
            let signed = keypair.sign_terms(terms.clone());
            assert_eq!(
                signed.terms, terms,
                "{scheme:?} must bundle the terms unchanged"
            );

            let hash = sp_crypto_hashing::blake2_256(&signed.terms.signing_payload());
            assert!(
                signed
                    .signature
                    .verify(&hash[..], &expected_signer(&key, &signed.signature)),
                "{scheme:?} terms signature failed verification"
            );
        }
    }

    /// Every scheme round-trips: sign() emits a SCALE MultiSignature whose
    /// variant matches the configured scheme and whose registered key shape
    /// is 32 (Sr25519/Ed25519) or 33 (Ecdsa/Eth) bytes.
    #[test]
    fn sign_round_trips_for_every_scheme() {
        use codec::{Decode, Encode};
        use sp_runtime::traits::Verify;

        let msg = b"scheme-round-trip";
        for (scheme, key_len) in [
            (KeyScheme::Sr25519, 32),
            (KeyScheme::Ed25519, 32),
            (KeyScheme::Ecdsa, 33),
            (KeyScheme::Eth, 33),
        ] {
            let keypair = ProviderKeypair::from_seed("//Alice", scheme).unwrap();
            let key = keypair.public_key_bytes();
            assert_eq!(key.len(), key_len, "{scheme:?} key length");

            let encoded = keypair.sign(msg).encode();
            let sig = MultiSignature::decode(&mut &encoded[..]).unwrap();
            let matches_scheme = matches!(
                (&sig, scheme),
                (MultiSignature::Sr25519(_), KeyScheme::Sr25519)
                    | (MultiSignature::Ed25519(_), KeyScheme::Ed25519)
                    | (MultiSignature::Ecdsa(_), KeyScheme::Ecdsa)
                    | (MultiSignature::Eth(_), KeyScheme::Eth)
            );
            assert!(matches_scheme, "{scheme:?} produced {sig:?}");

            // Verify the same way the pallet does: derive the expected
            // account from the raw key bytes for this scheme.
            assert!(
                sig.verify(&msg[..], &expected_signer(&key, &sig)),
                "{scheme:?} signature failed verification"
            );
        }
    }
}
