// SPDX-License-Identifier: Apache-2.0

use crate::{ChainState, ProviderInfo, ProviderKeypair, SigningRefused};
use provider_negotiation::{AgreementTermsOf, SignedTerms};
use sp_runtime::MultiSignature;
use std::sync::Arc;

/// The provider's signing keypair and the chain state it is checked against.
///
/// Every signature requires an on-chain registration whose `public_key`
/// equals the local key, because the pallet rejects any other signature. The
/// check reads the live [`ChainState`], so it passes after the chain-state
/// coordinator's next refresh without a restart.
pub struct ProviderSigner {
    keypair: ProviderKeypair,
    /// `keypair`'s raw public key, derived once at construction.
    public_key: Vec<u8>,
    chain_state: Arc<ChainState>,
}

impl ProviderSigner {
    /// Signer for `keypair`, checked against the registration in `chain_state`.
    pub fn new(keypair: ProviderKeypair, chain_state: Arc<ChainState>) -> Self {
        Self {
            public_key: keypair.public_key_bytes(),
            keypair,
            chain_state,
        }
    }

    /// Raw public key bytes of the local keypair.
    pub fn public_key_bytes(&self) -> &[u8] {
        &self.public_key
    }

    /// Sign a raw message, tagging the signature with its scheme.
    ///
    /// Returns [`SigningRefused::Unregistered`] while no registration is
    /// published and [`SigningRefused::KeyMismatch`] when the registered key
    /// differs from the local one.
    pub fn sign(&self, message: &[u8]) -> Result<MultiSignature, SigningRefused> {
        {
            let registration = self.chain_state.provider_info.read();
            let registration = registration.as_ref().ok_or(SigningRefused::Unregistered)?;
            self.ensure_key_matches(registration)?;
        }
        Ok(self.keypair.sign(message))
    }

    /// Sign negotiated terms for a caller that already has a `registration`
    /// snapshot: the key is checked against the snapshot the terms were
    /// validated with, not one the coordinator refreshed afterwards.
    pub fn sign_terms(
        &self,
        registration: &ProviderInfo,
        terms: AgreementTermsOf,
    ) -> Result<SignedTerms, SigningRefused> {
        self.ensure_key_matches(registration)?;
        Ok(self.keypair.sign_terms(terms))
    }

    /// Check that the local key equals the `public_key` in `registration`.
    pub fn ensure_key_matches(&self, registration: &ProviderInfo) -> Result<(), SigningRefused> {
        if registration.public_key != self.public_key {
            tracing::warn!(
                registered = %array_bytes::bytes2hex("", &registration.public_key),
                local = %array_bytes::bytes2hex("", &self.public_key),
                "local signing key does not match registered on-chain public_key"
            );
            return Err(SigningRefused::KeyMismatch);
        }
        Ok(())
    }
}
