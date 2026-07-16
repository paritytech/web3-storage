// SPDX-License-Identifier: Apache-2.0

//! Unified signer for provider-auth headers and on-chain extrinsics.

use crate::{ClientError, ClientResult};
use std::str::FromStr;
use std::sync::Arc;
use subxt_signer::{
    sr25519::{dev, Keypair},
    SecretUri,
};

/// A signer used for both provider-request authentication headers and on-chain
/// extrinsic signing.
///
/// Build it from a raw [`Keypair`], a secret URI / mnemonic ([`Signer::from_seed`]),
/// or a well-known dev account name ([`Signer::dev`]). Internally it is a
/// reference-counted sr25519 keypair (so cloning is cheap), and it implements
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

    /// A well-known dev account by name: `"alice"`..`"ferdie"` (case-insensitive).
    pub fn dev(name: &str) -> ClientResult<Self> {
        let keypair = match name.to_ascii_lowercase().as_str() {
            "alice" => dev::alice(),
            "bob" => dev::bob(),
            "charlie" => dev::charlie(),
            "dave" => dev::dave(),
            "eve" => dev::eve(),
            "ferdie" => dev::ferdie(),
            other => return Err(ClientError::Config(format!("unknown dev account: {other}"))),
        };
        Ok(Self(Arc::new(keypair)))
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
    fn seed_and_dev_agree_for_alice() {
        let seed = Signer::from_seed("//Alice").unwrap();
        let dev = Signer::dev("Alice").unwrap();
        assert_eq!(seed.keypair().public_key().0, dev.keypair().public_key().0);
    }

    #[test]
    fn unknown_dev_account_errors() {
        assert!(Signer::dev("nobody").is_err());
    }
}
