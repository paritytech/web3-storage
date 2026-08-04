// SPDX-License-Identifier: Apache-2.0

use crate::*;
use frame_support::pallet_prelude::*;
use sp_runtime::traits::SaturatedConversion;

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
    /// of) the anchor block. This prevents an attacker who captures one
    /// signed commitment from replaying it forever.
    pub(crate) fn ensure_recent_nonce(nonce: u64) -> DispatchResult {
        let anchor_block: u64 = Self::current_anchor_block().saturated_into();
        let max_age: u64 = T::MaxNonceAge::get().saturated_into();
        // Future-dated nonces are nonsensical — the signer can only know
        // the anchor block at sign-time. Allow exact equality.
        ensure!(nonce <= anchor_block, Error::<T>::CommitmentNonceTooOld);
        ensure!(
            anchor_block.saturating_sub(nonce) <= max_age,
            Error::<T>::CommitmentNonceTooOld
        );
        Ok(())
    }

    /// Derive the `AccountId32` that `MultiSignature::verify` compares
    /// against, from the provider's registered raw public key. The signature
    /// variant picks the derivation — `Eth` uses the revive-style keccak
    /// address mapping, not blake2, so it must not share the Ecdsa arm.
    fn expected_signer_account(
        public_key_bytes: &[u8],
        signature: &sp_runtime::MultiSignature,
    ) -> Result<sp_runtime::AccountId32, Error<T>> {
        use sp_runtime::{traits::IdentifyAccount, MultiSignature, MultiSigner};

        let signer = match signature {
            MultiSignature::Sr25519(_) => MultiSigner::Sr25519(
                sp_core::sr25519::Public::try_from(public_key_bytes)
                    .map_err(|_| Error::<T>::InvalidPublicKey)?,
            ),
            MultiSignature::Ed25519(_) => MultiSigner::Ed25519(
                sp_core::ed25519::Public::try_from(public_key_bytes)
                    .map_err(|_| Error::<T>::InvalidPublicKey)?,
            ),
            MultiSignature::Ecdsa(_) => MultiSigner::Ecdsa(
                sp_core::ecdsa::Public::try_from(public_key_bytes)
                    .map_err(|_| Error::<T>::InvalidPublicKey)?,
            ),
            MultiSignature::Eth(_) => MultiSigner::Eth(
                sp_core::ecdsa::KeccakPublic::try_from(public_key_bytes)
                    .map_err(|_| Error::<T>::InvalidPublicKey)?,
            ),
        };
        Ok(signer.into_account())
    }

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

        // Get the provider's registered public key
        let provider = Providers::<T>::get(signer).ok_or(Error::<T>::ProviderNotFound)?;
        let account_id = Self::expected_signer_account(provider.public_key.as_slice(), signature)?;

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

        let account_id = Self::expected_signer_account(provider_info.public_key.as_slice(), sig)?;

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
