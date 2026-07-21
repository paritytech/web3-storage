// SPDX-License-Identifier: Apache-2.0

//! Unified signer for provider-auth headers and on-chain extrinsics.

use crate::{ClientError, ClientResult};
use std::str::FromStr;
use std::sync::Arc;
use subxt_signer::{sr25519::Keypair, SecretUri};

/// A signer used for both provider-request authentication headers and on-chain
/// extrinsic signing.
///
/// Build it from a raw [`Keypair`] or a secret URI / mnemonic
/// ([`Signer::from_seed`], e.g. `"//Alice"` for a dev account). Internally it is
/// a reference-counted sr25519 keypair (so cloning is cheap), and it implements
/// [`subxt::tx::Signer`] so it can be handed straight to subxt for extrinsic
/// submission.
#[derive(Clone)]
pub struct Signer(Arc<Keypair>);

impl Signer {
    /// Wrap an existing sr25519 keypair.
    pub fn from_keypair(keypair: Keypair) -> Self {
        Self(Arc::new(keypair))
    }

    /// Derive from a secret URI or mnemonic, e.g. `"//Alice"`, `"<mnemonic>"`,
    /// or `"<mnemonic>//hard/soft"`.
    pub fn from_seed(seed: &str) -> ClientResult<Self> {
        let uri = SecretUri::from_str(seed)
            .map_err(|e| ClientError::Config(format!("invalid signer seed: {e}")))?;
        Keypair::from_uri(&uri)
            .map(|keypair| Self(Arc::new(keypair)))
            .map_err(|e| ClientError::Config(format!("invalid signer seed: {e}")))
    }

    /// The underlying keypair, e.g. to build provider-auth headers.
    pub fn keypair(&self) -> &Keypair {
        &self.0
    }
}

impl From<Keypair> for Signer {
    fn from(keypair: Keypair) -> Self {
        Self(Arc::new(keypair))
    }
}

impl core::fmt::Debug for Signer {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        // Public key only — never print secret material.
        write!(f, "Signer({:?})", self.0.public_key().0)
    }
}

/// Delegates to the inner keypair so a `Signer` can be passed straight to subxt.
impl<T> subxt::tx::Signer<T> for Signer
where
    T: subxt::Config,
    Keypair: subxt::tx::Signer<T>,
{
    fn account_id(&self) -> T::AccountId {
        <Keypair as subxt::tx::Signer<T>>::account_id(self.keypair())
    }

    fn sign(&self, signer_payload: &[u8]) -> T::Signature {
        <Keypair as subxt::tx::Signer<T>>::sign(self.keypair(), signer_payload)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn from_seed_matches_well_known_dev_alice() {
        // "//Alice" must derive the canonical well-known Alice account.
        use subxt_signer::sr25519::dev;
        let seed = Signer::from_seed("//Alice").unwrap();
        assert_eq!(
            seed.keypair().public_key().0,
            dev::alice().public_key().0
        );
    }

    #[test]
    fn invalid_seed_errors() {
        assert!(Signer::from_seed("not a valid bip39 mnemonic").is_err());
    }
}
