use crate::*;
use frame_support::pallet_prelude::*;

impl<T: Config> Pallet<T> {
    /// Verify a MultiSignature against an encoded message using stored public key.
    ///
    /// This:
    /// 1. Retrieves the provider's registered public key from storage
    /// 2. Reconstructs the appropriate public key type from raw bytes
    /// 3. Verifies the signature matches the message and public key
    ///
    /// Returns Error::InvalidSignature if verification fails.
    /// Reject a `CommitmentPayload::nonce` that is too far behind (or ahead
    /// of) the current block. This prevents an attacker who captures one
    /// signed commitment from replaying it forever.
    pub fn ensure_recent_nonce(nonce: u64) -> DispatchResult {
        use sp_runtime::traits::SaturatedConversion;
        let current: u64 = frame_system::Pallet::<T>::block_number().saturated_into();
        let max_age: u64 = T::MaxNonceAge::get().saturated_into();
        // Future-dated nonces are nonsensical — the signer can only know
        // the current block at sign-time. Allow exact equality.
        ensure!(nonce <= current, Error::<T>::CommitmentNonceTooOld);
        ensure!(
            current.saturating_sub(nonce) <= max_age,
            Error::<T>::CommitmentNonceTooOld
        );
        Ok(())
    }

    /// Verify a MultiSignature against an encoded message using stored public key.
    ///
    /// This:
    /// 1. Retrieves the provider's registered public key from storage
    /// 2. Reconstructs the appropriate public key type from raw bytes
    /// 3. Verifies the signature matches the message and public key
    ///
    /// Returns Error::InvalidSignature if verification fails.
    pub fn verify_signature(
        signature: &sp_runtime::MultiSignature,
        message: &[u8],
        signer: &T::AccountId,
    ) -> DispatchResult {
        use sp_runtime::traits::Verify;

        // Get the provider's registered public key
        let provider = Providers::<T>::get(signer).ok_or(Error::<T>::ProviderNotFound)?;
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
    pub fn verify_terms_signature(
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
