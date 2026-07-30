// SPDX-License-Identifier: Apache-2.0

//! Storage migrations for `pallet-storage-provider`.

extern crate alloc;

/// v0 -> v1: backfill the `commitment_nonce` field added to `BucketSnapshot` by
/// the challenge flow overhaul (#125).
///
/// `BucketSnapshot` (nested in `Bucket::snapshot`) gained a trailing
/// `commitment_nonce: u64` field, required by `extend_checkpoint` to verify a
/// late-arriving signature against the payload the original signers signed.
/// `Buckets` entries checkpointed before that change still encode the old
/// (nonce-less) layout, so the current `Bucket`/`BucketSnapshot` decode treats
/// them as undecodable — `try-runtime` reports the keys as undecodable. This
/// re-stores every entry in the new layout, defaulting `commitment_nonce` to
/// `0` (harmless: the nonce only matters to late-signature verification for a
/// checkpoint still accepting signers, which no pre-existing snapshot has).
pub mod v1 {
    use crate::{BlockNumberFor, Bucket, Buckets, Config, Member, Pallet};
    use frame_support::{pallet_prelude::*, traits::UncheckedOnRuntimeUpgrade, weights::Weight};
    use sp_core::H256;
    use storage_primitives::{BucketSnapshot, Commitment};

    /// The pre-migration `Bucket`/`BucketSnapshot`, used only to decode the old
    /// `Buckets` values. Mirrors the pre-#125 layout, i.e. the current layout
    /// minus the trailing `commitment_nonce` field.
    mod old {
        use super::*;
        use alloc::vec::Vec;

        #[derive(Decode)]
        pub struct BucketSnapshot<BlockNumber> {
            pub commitment: Commitment,
            pub checkpoint_block: BlockNumber,
            pub primary_signers: Vec<u8>,
        }

        #[derive(Decode)]
        pub struct Bucket<T: Config> {
            pub members: BoundedVec<Member<T>, T::MaxMembers>,
            pub frozen_start_seq: Option<u64>,
            pub min_providers: u32,
            pub primary_providers: BoundedVec<T::AccountId, T::MaxPrimaryProviders>,
            pub snapshot: Option<BucketSnapshot<BlockNumberFor<T>>>,
            pub historical_roots: [(u32, H256); 6],
            pub total_snapshots: u32,
        }
    }

    pub struct InnerMigrateV0ToV1<T>(core::marker::PhantomData<T>);

    impl<T: Config> UncheckedOnRuntimeUpgrade for InnerMigrateV0ToV1<T> {
        fn on_runtime_upgrade() -> Weight {
            let total = Buckets::<T>::iter_keys().count() as u64;
            let mut translated = 0u64;
            Buckets::<T>::translate::<old::Bucket<T>, _>(|_bucket_id, old| {
                translated = translated.saturating_add(1);
                Some(Bucket {
                    members: old.members,
                    frozen_start_seq: old.frozen_start_seq,
                    min_providers: old.min_providers,
                    primary_providers: old.primary_providers,
                    snapshot: old.snapshot.map(|s| BucketSnapshot {
                        commitment: s.commitment,
                        checkpoint_block: s.checkpoint_block,
                        primary_signers: s.primary_signers,
                        commitment_nonce: 0,
                    }),
                    historical_roots: old.historical_roots,
                    total_snapshots: old.total_snapshots,
                    // Fail-safe default; no v0 chain with data of value exists.
                    visibility: storage_primitives::Visibility::Private,
                })
            });
            // `translate` reads every key in the map (whether or not it
            // decodes under the old layout) but only rewrites the ones that
            // do, so `total` reads and `translated` writes is the true upper
            // bound rather than under-counting reads on decode failures.
            T::DbWeight::get().reads_writes(total, translated)
        }

        #[cfg(feature = "try-runtime")]
        fn pre_upgrade() -> Result<alloc::vec::Vec<u8>, sp_runtime::TryRuntimeError> {
            // Count existing keys before migration so post_upgrade can confirm
            // none were dropped.
            let count = Buckets::<T>::iter_keys().count() as u64;
            Ok(count.encode())
        }

        #[cfg(feature = "try-runtime")]
        fn post_upgrade(state: alloc::vec::Vec<u8>) -> Result<(), sp_runtime::TryRuntimeError> {
            let before = u64::decode(&mut &state[..]).map_err(|_| "invalid pre_upgrade state")?;
            // `iter_keys()` only enumerates keys; `iter()` fully decodes every
            // value under the NEW layout. If any entry still failed to decode,
            // `translate` would have left it as a raw, undecodable blob that
            // `iter()` silently skips, so the two counts would diverge.
            let after_keys = Buckets::<T>::iter_keys().count() as u64;
            let after_values = Buckets::<T>::iter().count() as u64;
            ensure!(
                before == after_keys,
                "Buckets entry count changed during migration"
            );
            ensure!(
                after_keys == after_values,
                "some Buckets entry failed to decode under the new layout"
            );
            Ok(())
        }
    }

    /// Runs [`InnerMigrateV0ToV1`] only when the on-chain storage version is 0,
    /// then bumps it to 1.
    pub type MigrateV0ToV1<T> = frame_support::migrations::VersionedMigration<
        0,
        1,
        InnerMigrateV0ToV1<T>,
        Pallet<T>,
        <T as frame_system::Config>::DbWeight,
    >;
}
