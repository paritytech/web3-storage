// SPDX-License-Identifier: Apache-2.0

//! The replica's signed attestation of the sync roots it holds, and the
//! trait that produces the signature over it.

use codec::Encode;
use provider_types::{ProviderSigner, SigningRefused};
use sp_core::H256;

/// The roots array `confirm_replica_sync` takes: position 0 is the target
/// (current snapshot) root, positions 1-6 map to the bucket's prime-bucketed
/// historical slots.
pub type SyncRoots = [Option<H256>; 7];

/// A replica's signed attestation of the sync roots it claims. Bundling the
/// array with the signature over its SCALE encoding keeps the signed and the
/// submitted payload one value — they cannot drift apart.
#[derive(Clone, Debug)]
pub struct SignedSyncRoots {
    /// The roots shape `confirm_replica_sync` expects: target root in
    /// position 0, positions 1–6 map to the bucket's prime-bucketed
    /// historical slots (unused by the node today).
    pub roots: SyncRoots,
    /// Scheme-tagged signature over `SCALE(roots)` by the registered key.
    pub signature: sp_runtime::MultiSignature,
}

impl SignedSyncRoots {
    /// Attest the target root with the provider's registered signing key.
    pub fn sign(
        signer: &dyn SyncRootsSigner,
        target_mmr_root: H256,
    ) -> Result<Self, SigningRefused> {
        let mut roots: SyncRoots = [None; 7];
        roots[0] = Some(target_mmr_root);
        let signature = signer.sign_sync_roots(&roots)?;
        Ok(Self { roots, signature })
    }
}

/// Signs a replica's sync-roots attestation with the provider's registered
/// key. The node owns the key material and the scheme it was registered
/// under, so it supplies the implementation; this crate only needs the
/// resulting scheme-tagged signature.
///
/// Implementations are expected to refuse when no key is configured, or when
/// the local key no longer matches the on-chain registration - a signature the
/// pallet cannot verify is worse than a skipped confirmation.
pub trait SyncRootsSigner: Send + Sync {
    /// Sign the SCALE encoding of `roots` with the registered key, or
    /// explain why not.
    fn sign_sync_roots(
        &self,
        roots: &SyncRoots,
    ) -> Result<sp_runtime::MultiSignature, SigningRefused>;
}

impl SyncRootsSigner for ProviderSigner {
    fn sign_sync_roots(
        &self,
        roots: &SyncRoots,
    ) -> Result<sp_runtime::MultiSignature, SigningRefused> {
        self.sign(&roots.encode())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use sp_core::Pair;
    use sp_runtime::traits::{IdentifyAccount, Verify};
    use sp_runtime::MultiSigner;

    struct TestSigner(sp_core::sr25519::Pair);

    impl SyncRootsSigner for TestSigner {
        fn sign_sync_roots(
            &self,
            roots: &SyncRoots,
        ) -> Result<sp_runtime::MultiSignature, SigningRefused> {
            Ok(sp_runtime::MultiSignature::Sr25519(
                self.0.sign(&roots.encode()),
            ))
        }
    }

    /// The signature `SignedSyncRoots::sign` produces must verify against the
    /// SCALE encoding of the roots array it bundles - the same payload the
    /// pallet reconstructs and checks in `confirm_replica_sync`. This is the
    /// property the module's own doc comment claims, and the one the
    /// refactor that split the signer out of `coordinator.rs` must preserve.
    #[test]
    fn signed_roots_verify_against_the_scale_encoded_array() {
        let pair = sp_core::sr25519::Pair::from_string("//Test", None).unwrap();
        let account = MultiSigner::Sr25519(pair.public()).into_account();
        let signer = TestSigner(pair);
        let target = H256::repeat_byte(7);

        let attestation = SignedSyncRoots::sign(&signer, target).unwrap();

        assert_eq!(
            attestation.roots,
            [Some(target), None, None, None, None, None, None]
        );
        assert!(attestation
            .signature
            .verify(attestation.roots.encode().as_slice(), &account));
    }

    fn registration(public_key: Vec<u8>) -> provider_types::ProviderInfo {
        provider_types::ProviderInfo {
            multiaddr: "/ip4/1.2.3.4/tcp/3333".to_string(),
            public_key,
            stake: 1_000,
            committed_bytes: 0,
            settings: provider_types::ProviderSettings {
                min_duration: 10,
                max_duration: 100,
                price_per_byte: 1,
                accepting_primary: true,
                replica_sync_price: None,
                accepting_extensions: true,
                max_capacity: 0,
                min_agreement_bytes: 0,
            },
            stats: Default::default(),
            deregister_at: None,
        }
    }

    /// The pallet verifies a sync confirmation against the registered key, so
    /// the node's signer must reject signing until the registration contains
    /// the local key, and then sign the SCALE encoding of the roots.
    #[test]
    fn provider_signer_signs_roots_only_with_the_registered_key() {
        let pair = sp_core::sr25519::Pair::from_string("//Alice", None).unwrap();
        let account = MultiSigner::Sr25519(pair.public()).into_account();
        let keypair = provider_types::ProviderKeypair::from_seed(
            "//Alice",
            provider_types::KeyScheme::Sr25519,
        )
        .unwrap();
        let chain_state = std::sync::Arc::new(provider_types::ChainState::default());
        let signer = ProviderSigner::new(keypair, chain_state.clone());
        let roots: SyncRoots = [
            Some(H256::repeat_byte(7)),
            None,
            None,
            None,
            None,
            None,
            None,
        ];

        assert!(matches!(
            signer.sign_sync_roots(&roots),
            Err(SigningRefused::Unregistered)
        ));

        *chain_state.provider_info.write() = Some(registration(vec![1; 32]));
        assert!(matches!(
            signer.sign_sync_roots(&roots),
            Err(SigningRefused::KeyMismatch)
        ));

        *chain_state.provider_info.write() = Some(registration(signer.public_key_bytes().to_vec()));
        let signature = signer.sign_sync_roots(&roots).unwrap();
        assert!(signature.verify(roots.encode().as_slice(), &account));
    }
}
