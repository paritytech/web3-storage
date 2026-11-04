// SPDX-License-Identifier: Apache-2.0

//! The replica's signed attestation of the sync roots it holds, and the
//! trait that produces the signature over it.

use provider_types::SigningRefused;
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
    /// The roots shape `confirm_replica_sync` expects: the synced root at the
    /// position it matches (0 for the snapshot, 1-6 for the historical
    /// slots), `None` everywhere else.
    pub roots: SyncRoots,
    /// Scheme-tagged signature over `SCALE(roots)` by the registered key.
    pub signature: sp_runtime::MultiSignature,
}

impl SignedSyncRoots {
    /// Attest `target_mmr_root` at `position` with the provider's registered
    /// signing key.
    ///
    /// The pallet returns the first position whose root matches, so the root
    /// goes at one position only. Panics if `position` is above 6.
    pub fn sign(
        signer: &dyn SyncRootsSigner,
        position: u8,
        target_mmr_root: H256,
    ) -> Result<Self, SigningRefused> {
        let mut roots: SyncRoots = [None; 7];
        roots[usize::from(position)] = Some(target_mmr_root);
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

#[cfg(test)]
mod tests {
    use super::*;
    use codec::Encode;
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

        let attestation = SignedSyncRoots::sign(&signer, 0, target).unwrap();

        assert_eq!(
            attestation.roots,
            [Some(target), None, None, None, None, None, None]
        );
        assert!(attestation
            .signature
            .verify(attestation.roots.encode().as_slice(), &account));
    }

    /// The pallet compares `roots[0]` to the snapshot and `roots[p]` to
    /// `historical_roots[p - 1]`, so the root must sit at exactly the position
    /// it was selected at and nowhere else.
    #[test]
    fn the_root_is_signed_at_its_position_only() {
        let pair = sp_core::sr25519::Pair::from_string("//Test", None).unwrap();
        let account = MultiSigner::Sr25519(pair.public()).into_account();
        let signer = TestSigner(pair);
        let target = H256::repeat_byte(7);

        for position in 0u8..=6 {
            let attestation = SignedSyncRoots::sign(&signer, position, target).unwrap();

            let mut expected: SyncRoots = [None; 7];
            expected[usize::from(position)] = Some(target);
            assert_eq!(attestation.roots, expected, "position {position}");
            assert!(attestation
                .signature
                .verify(attestation.roots.encode().as_slice(), &account));
        }
    }
}
