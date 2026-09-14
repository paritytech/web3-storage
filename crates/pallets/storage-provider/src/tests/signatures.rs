// SPDX-License-Identifier: Apache-2.0

//! Signature-scheme accept/reject matrix: every `MultiSignature` variant
//! against every registered key shape, for both verification paths
//! (raw-message commitments and hashed terms).

use super::*;
use frame_support::{traits::ConstU32, BoundedVec};
use sp_core::{ecdsa, ed25519, sr25519, Pair as _};
use sp_runtime::MultiSignature;

const MSG: &[u8] = b"commitment-payload-stand-in";

/// Register provider 2 and stamp a fresh keypair of scheme `P` into its
/// on-chain `public_key`.
fn provider_with_key<P: sp_core::Pair>() -> P {
    register_provider(2, 200);
    provider_signer_with::<P>(2)
}

// ─────────────────────────────────────────────────────────────────────────
// Registration key-length validation
// ─────────────────────────────────────────────────────────────────────────

#[test]
fn registration_accepts_32_and_33_byte_keys() {
    new_test_ext().execute_with(|| {
        for (who, len) in [(2u64, 32usize), (3, 33)] {
            let key: BoundedVec<u8, ConstU32<64>> = vec![1u8; len].try_into().unwrap();
            assert_ok!(StorageProvider::register_provider(
                RuntimeOrigin::signed(who),
                b"/ip4/127.0.0.1/tcp/3000".to_vec().try_into().unwrap(),
                key,
                200
            ));
        }
    });
}

#[test]
fn registration_rejects_unverifiable_key_lengths() {
    new_test_ext().execute_with(|| {
        // 64 fits the BoundedVec but no scheme can ever verify against it —
        // accepting it would register a provider that fails every signed call.
        for len in [0usize, 31, 34, 64] {
            let key: BoundedVec<u8, ConstU32<64>> = vec![1u8; len].try_into().unwrap();
            assert_noop!(
                StorageProvider::register_provider(
                    RuntimeOrigin::signed(2),
                    b"/ip4/127.0.0.1/tcp/3000".to_vec().try_into().unwrap(),
                    key,
                    200
                ),
                Error::<Test>::InvalidPublicKey
            );
        }
    });
}

/// A 64-byte key was accepted by registration before #274, so live chains can
/// still hold one. No scheme derives a signer from it, so every variant must
/// fail closed with `InvalidPublicKey` — before verification is even attempted.
#[test]
fn verify_signature_rejects_legacy_64_byte_stored_key() {
    new_test_ext().execute_with(|| {
        register_provider(2, 200);
        crate::Providers::<Test>::mutate(2, |maybe_p| {
            let p = maybe_p.as_mut().expect("provider 2 is registered");
            p.public_key = vec![1u8; 64].try_into().expect("64 fits the bound");
        });

        for sig in [
            MultiSignature::Sr25519(sr25519::Signature::from_raw([0u8; 64])),
            MultiSignature::Ed25519(ed25519::Signature::from_raw([0u8; 64])),
            MultiSignature::Ecdsa(ecdsa::Signature::from_raw([0u8; 65])),
            MultiSignature::Eth(ecdsa::KeccakSignature::from_raw([0u8; 65])),
        ] {
            assert_err!(
                StorageProvider::verify_signature(&sig, MSG, &2),
                Error::<Test>::InvalidPublicKey
            );
        }
    });
}

// ─────────────────────────────────────────────────────────────────────────
// verify_signature (raw message) — positive round-trip per scheme
// ─────────────────────────────────────────────────────────────────────────

#[test]
fn verify_signature_sr25519_round_trip() {
    new_test_ext().execute_with(|| {
        let pair: sr25519::Pair = provider_with_key();
        let sig = MultiSignature::Sr25519(pair.sign(MSG));
        assert_ok!(StorageProvider::verify_signature(&sig, MSG, &2));
    });
}

#[test]
fn verify_signature_ed25519_round_trip() {
    new_test_ext().execute_with(|| {
        let pair: ed25519::Pair = provider_with_key();
        let sig = MultiSignature::Ed25519(pair.sign(MSG));
        assert_ok!(StorageProvider::verify_signature(&sig, MSG, &2));
    });
}

#[test]
fn verify_signature_ecdsa_round_trip() {
    new_test_ext().execute_with(|| {
        let pair: ecdsa::Pair = provider_with_key();
        let sig = MultiSignature::Ecdsa(pair.sign(MSG));
        assert_ok!(StorageProvider::verify_signature(&sig, MSG, &2));
    });
}

/// Regression for #274: the Eth arm used to share the Ecdsa (blake2)
/// account derivation, so a keccak-signed `Eth` signature could never
/// verify against its own registered key.
#[test]
fn verify_signature_eth_round_trip() {
    new_test_ext().execute_with(|| {
        let pair: ecdsa::KeccakPair = provider_with_key();
        let sig = MultiSignature::Eth(pair.sign(MSG));
        assert_ok!(StorageProvider::verify_signature(&sig, MSG, &2));
    });
}

// ─────────────────────────────────────────────────────────────────────────
// verify_signature — negatives
// ─────────────────────────────────────────────────────────────────────────

/// A signature variant whose scheme needs a different key length than the
/// registered key fails fast with `InvalidPublicKey`.
#[test]
fn verify_signature_rejects_key_length_mismatch() {
    new_test_ext().execute_with(|| {
        // 32-byte key registered, 33-byte-key schemes submitted.
        let _pair_32: sr25519::Pair = provider_with_key();
        let ecdsa_pair = ecdsa::Pair::from_seed(&[9u8; 32]);
        let keccak_pair = ecdsa::KeccakPair::from_seed(&[9u8; 32]);
        for sig in [
            MultiSignature::Ecdsa(ecdsa_pair.sign(MSG)),
            MultiSignature::Eth(keccak_pair.sign(MSG)),
        ] {
            assert_err!(
                StorageProvider::verify_signature(&sig, MSG, &2),
                Error::<Test>::InvalidPublicKey
            );
        }

        // 33-byte key registered, 32-byte-key schemes submitted.
        provider_signer_with::<ecdsa::Pair>(2);
        let sr_pair = sr25519::Pair::from_seed(&[9u8; 32]);
        let ed_pair = ed25519::Pair::from_seed(&[9u8; 32]);
        for sig in [
            MultiSignature::Sr25519(sr_pair.sign(MSG)),
            MultiSignature::Ed25519(ed_pair.sign(MSG)),
        ] {
            assert_err!(
                StorageProvider::verify_signature(&sig, MSG, &2),
                Error::<Test>::InvalidPublicKey
            );
        }
    });
}

/// Sr25519 and Ed25519 share the 32-byte key shape: a signature made with
/// the right key but tagged as the sibling scheme must still fail — the
/// derivation matches, the curve verification does not.
#[test]
fn verify_signature_rejects_wrong_variant_same_key_bytes() {
    new_test_ext().execute_with(|| {
        let pair: sr25519::Pair = provider_with_key();
        let sr_sig = pair.sign(MSG);
        let mistagged = MultiSignature::Ed25519(ed25519::Signature::from_raw(sr_sig.0));
        assert_err!(
            StorageProvider::verify_signature(&mistagged, MSG, &2),
            Error::<Test>::InvalidSignature
        );
    });
}

/// Ecdsa and Eth share the 33-byte compressed key shape but hash the
/// message differently (blake2 vs keccak) and derive different accounts —
/// cross-tagging must fail in both directions.
#[test]
fn verify_signature_rejects_ecdsa_eth_cross_tagging() {
    new_test_ext().execute_with(|| {
        let pair: ecdsa::Pair = provider_with_key();
        let keccak_pair = ecdsa::KeccakPair::from_seed(&[2u8; 32]);

        // Blake2-signed bytes tagged as Eth.
        let ecdsa_sig = pair.sign(MSG);
        let as_eth = MultiSignature::Eth(ecdsa::KeccakSignature::from_raw(ecdsa_sig.0));
        assert_err!(
            StorageProvider::verify_signature(&as_eth, MSG, &2),
            Error::<Test>::InvalidSignature
        );

        // Keccak-signed bytes tagged as Ecdsa (key re-stamped to match the
        // keccak pair so only the variant is wrong).
        provider_signer_with::<ecdsa::KeccakPair>(2);
        let keccak_sig = keccak_pair.sign(MSG);
        let as_ecdsa = MultiSignature::Ecdsa(ecdsa::Signature::from_raw(keccak_sig.0));
        assert_err!(
            StorageProvider::verify_signature(&as_ecdsa, MSG, &2),
            Error::<Test>::InvalidSignature
        );
    });
}

// ─────────────────────────────────────────────────────────────────────────
// Terms path (blake2(context ‖ SCALE(terms))) — end-to-end through
// establish_storage_agreement per scheme
// ─────────────────────────────────────────────────────────────────────────

fn establish_with<P: sp_core::Pair>(wrap: impl Fn(P::Signature) -> MultiSignature) {
    new_test_ext().execute_with(|| {
        let pair: P = provider_with_key();
        let terms = primary_terms(1, 100, 100, 0);
        let hash = sp_io::hashing::blake2_256(&terms.signing_payload());
        let sig = wrap(pair.sign(&hash));
        assert_ok!(StorageProvider::establish_storage_agreement(
            RuntimeOrigin::signed(1),
            2,
            terms,
            sig,
            storage_primitives::Visibility::Public
        ));
        assert!(StorageAgreements::<Test>::get(0, 2).is_some());
    });
}

#[test]
fn terms_redemption_works_with_ed25519() {
    establish_with::<ed25519::Pair>(MultiSignature::Ed25519);
}

#[test]
fn terms_redemption_works_with_ecdsa() {
    establish_with::<ecdsa::Pair>(MultiSignature::Ecdsa);
}

#[test]
fn terms_redemption_works_with_eth() {
    establish_with::<ecdsa::KeccakPair>(MultiSignature::Eth);
}

#[test]
fn terms_redemption_rejects_scheme_key_mismatch() {
    new_test_ext().execute_with(|| {
        // Registered key is sr25519 (32B); quote signed with ecdsa (33B key).
        let _sr: sr25519::Pair = provider_with_key();
        let ecdsa_pair = ecdsa::Pair::from_seed(&[7u8; 32]);
        let terms = primary_terms(1, 100, 100, 0);
        let hash = sp_io::hashing::blake2_256(&terms.signing_payload());
        let sig = MultiSignature::Ecdsa(ecdsa_pair.sign(&hash));
        assert_noop!(
            StorageProvider::establish_storage_agreement(
                RuntimeOrigin::signed(1),
                2,
                terms,
                sig,
                storage_primitives::Visibility::Public
            ),
            Error::<Test>::InvalidPublicKey
        );
    });
}
