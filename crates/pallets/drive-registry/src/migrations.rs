// SPDX-License-Identifier: Apache-2.0

//! Storage migrations for `pallet-drive-registry`.

extern crate alloc;

/// v0 -> v1: drop the trailing `payment` field from `DriveInfo`.
///
/// `DriveInfo` carried a `payment: Balance` field until #105 moved agreement
/// payment off the drive record. `Drives` entries written by the
/// `v0.1.1-paseo` runtime still encode that trailing balance, so the post-#105
/// `DriveInfo` (which has no `payment`) cannot decode them — `try-runtime`
/// reports the keys as undecodable. This re-stores every entry in the new
/// layout, dropping the trailing balance.
pub mod v1 {
    use crate::pallet::{BalanceOf, Config, Drives, Pallet};
    use file_system_primitives::DriveInfo;
    use frame_support::{pallet_prelude::*, traits::UncheckedOnRuntimeUpgrade, weights::Weight};
    use frame_system::pallet_prelude::BlockNumberFor;

    /// The pre-migration `DriveInfo`, used only to decode the old `Drives`
    /// values. Mirrors the `v0.1.1-paseo` layout, including the trailing
    /// `payment` field that the current type dropped.
    mod old {
        use super::*;
        use sp_runtime::BoundedVec;

        #[derive(Decode)]
        pub struct DriveInfo<AccountId, BlockNumber, MaxNameLength: Get<u32>, Balance> {
            pub owner: AccountId,
            pub bucket_id: u64,
            pub created_at: BlockNumber,
            pub name: Option<BoundedVec<u8, MaxNameLength>>,
            pub max_capacity: u64,
            pub storage_period: BlockNumber,
            pub expires_at: BlockNumber,
            // Decoded only to consume the trailing bytes; dropped by the
            // migration, hence never read.
            #[allow(dead_code)]
            pub payment: Balance,
        }
    }

    type OldDriveInfo<T> = old::DriveInfo<
        <T as frame_system::Config>::AccountId,
        BlockNumberFor<T>,
        <T as Config>::MaxDriveNameLength,
        BalanceOf<T>,
    >;

    pub struct InnerMigrateV0ToV1<T>(core::marker::PhantomData<T>);

    impl<T: Config> UncheckedOnRuntimeUpgrade for InnerMigrateV0ToV1<T> {
        fn on_runtime_upgrade() -> Weight {
            let mut translated = 0u64;
            Drives::<T>::translate::<OldDriveInfo<T>, _>(|_drive_id, old| {
                translated = translated.saturating_add(1);
                Some(DriveInfo {
                    owner: old.owner,
                    bucket_id: old.bucket_id,
                    created_at: old.created_at,
                    name: old.name,
                    max_capacity: old.max_capacity,
                    storage_period: old.storage_period,
                    expires_at: old.expires_at,
                })
            });
            // One read + one write per migrated entry.
            T::DbWeight::get().reads_writes(translated, translated)
        }

        #[cfg(feature = "try-runtime")]
        fn pre_upgrade() -> Result<alloc::vec::Vec<u8>, sp_runtime::TryRuntimeError> {
            // Count entries decoded with the OLD layout so post_upgrade can
            // confirm none were dropped.
            let count = Drives::<T>::iter_keys().count() as u64;
            Ok(count.encode())
        }

        #[cfg(feature = "try-runtime")]
        fn post_upgrade(state: alloc::vec::Vec<u8>) -> Result<(), sp_runtime::TryRuntimeError> {
            let before = u64::decode(&mut &state[..]).map_err(|_| "invalid pre_upgrade state")?;
            // `iter()` fully decodes values under the NEW layout; if any entry
            // still failed to decode this count would be short.
            let after = Drives::<T>::iter().count() as u64;
            ensure!(
                before == after,
                "Drives entry count changed during migration"
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
