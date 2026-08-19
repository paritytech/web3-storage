// SPDX-License-Identifier: Apache-2.0

use crate::*;
use frame_support::pallet_prelude::*;

/// Verify a signature against a bare account's own key bytes: on
/// AccountId32 runtimes the SCALE-encoded AccountId IS the public key,
/// and `MultiSignature::verify` handles every scheme against it.
/// `None` when the encoding is not 32 bytes — such a runtime has no
/// plain-account identity to verify against.
pub(crate) fn plain_account_verifies(
    signature: &sp_runtime::MultiSignature,
    message: &[u8],
    encoded_signer: &[u8],
) -> Option<bool> {
    use sp_runtime::traits::Verify;

    let bytes: [u8; 32] = encoded_signer.try_into().ok()?;
    Some(signature.verify(message, &sp_runtime::AccountId32::new(bytes)))
}

impl<T: Config> Pallet<T> {
    /// Verify a MultiSignature against an encoded message using stored public key.
    ///
    /// This:
    /// 1. Retrieves the provider's registered public key from storage
    /// 2. Reconstructs the appropriate public key type from raw bytes
    /// 3. Verifies the signature matches the message and public key
    ///
    /// Returns Error::InvalidSignature if verification fails.
    pub(crate) fn verify_signature(
        signature: &sp_runtime::MultiSignature,
        message: &[u8],
        signer: &T::AccountId,
    ) -> DispatchResult {
        use sp_runtime::traits::Verify;

        // Registered providers sign with their registered key. A plain
        // account (e.g. the bucket admin signing a deletion authorization
        // for the `Deleted` challenge defense) has no registry entry, so
        // its own key bytes are the verification identity.
        let Some(provider) = Providers::<T>::get(signer) else {
            return match plain_account_verifies(signature, message, &signer.encode()) {
                None => Err(Error::<T>::InvalidPublicKey.into()),
                Some(false) => Err(Error::<T>::InvalidSignature.into()),
                Some(true) => Ok(()),
            };
        };
        let public_key_bytes = provider.public_key.as_slice();

        // Convert public key to AccountId32 based on signature type
        let account_id = match signature {
            sp_runtime::MultiSignature::Sr25519(_) | sp_runtime::MultiSignature::Ed25519(_) => {
                // Sr25519 and Ed25519 public keys are 32 bytes, directly used as AccountId32
                if public_key_bytes.len() != 32 {
                    return Err(Error::<T>::InvalidPublicKey.into());
                }
                let mut key_bytes = [0u8; 32];
                key_bytes.copy_from_slice(public_key_bytes);
                sp_runtime::AccountId32::new(key_bytes)
            }
            sp_runtime::MultiSignature::Ecdsa(_) | sp_runtime::MultiSignature::Eth(_) => {
                // Ecdsa/Eth public keys are 33 bytes (compressed), AccountId32 is blake2_256 hash
                if public_key_bytes.len() != 33 {
                    return Err(Error::<T>::InvalidPublicKey.into());
                }
                let hash = sp_io::hashing::blake2_256(public_key_bytes);
                sp_runtime::AccountId32::new(hash)
            }
        };

        // Verify signature against the account ID
        let is_valid = signature.verify(message, &account_id);

        ensure!(is_valid, Error::<T>::InvalidSignature);

        Ok(())
    }

    /// Verify a provider signature over a SCALE-encoded
    /// [`AgreementTermsOf<T>`]. The signed payload is
    /// `blake2_256(context | terms.encode())`, where `context` is the
    /// domain-separation prefix for the redemption path
    /// ([`storage_primitives::PRIMARY_TERM_CONTEXT`] or
    /// [`storage_primitives::REPLICA_TERM_CONTEXT`]) — the caller, not
    /// the terms, decides it, so a quote signed for one flavour can
    /// never be redeemed as the other.
    pub(crate) fn verify_terms_signature(
        provider_info: &ProviderInfo<T>,
        terms: &AgreementTermsOf<T>,
        sig: &sp_runtime::MultiSignature,
        context: &[u8],
    ) -> DispatchResult {
        use sp_runtime::traits::Verify;

        let public_key_bytes = provider_info.public_key.as_slice();
        let account_id = match sig {
            sp_runtime::MultiSignature::Sr25519(_) | sp_runtime::MultiSignature::Ed25519(_) => {
                ensure!(public_key_bytes.len() == 32, Error::<T>::InvalidPublicKey);
                let mut key_bytes = [0u8; 32];
                key_bytes.copy_from_slice(public_key_bytes);
                sp_runtime::AccountId32::new(key_bytes)
            }
            sp_runtime::MultiSignature::Ecdsa(_) | sp_runtime::MultiSignature::Eth(_) => {
                ensure!(public_key_bytes.len() == 33, Error::<T>::InvalidPublicKey);
                let hash = sp_io::hashing::blake2_256(public_key_bytes);
                sp_runtime::AccountId32::new(hash)
            }
        };

        let mut payload = context.to_vec();
        terms.encode_to(&mut payload);
        let hash = sp_io::hashing::blake2_256(&payload);
        ensure!(
            sig.verify(&hash[..], &account_id),
            Error::<T>::InvalidProviderSignature
        );
        Ok(())
    }
}
