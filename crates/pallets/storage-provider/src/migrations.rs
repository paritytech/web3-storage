// SPDX-License-Identifier: Apache-2.0

//! Storage migrations for `pallet-storage-provider`.

extern crate alloc;

/// v0 -> v1: `Challenge` gained `leaf_count` and `ReplicaSyncRecord` became
/// `{ root, range, block }` (leaf-index binding, #301).
///
/// Neither value is translatable: an in-flight challenge's `leaf_count` was
/// never recorded, and an old sync record cannot show that its commitment was
/// the bucket's current snapshot (the pre-#301 pallet fabricated `(0, 0)`
/// ranges for historical-root syncs). So the migration drains all pending
/// challenges — refunding each challenger's deposit and resetting the pending
/// counters — and drops replica `last_sync` records, so a replica must
/// re-sync before it is challengeable again.
pub mod v1 {
    use crate::pallet::{
        BalanceOf, Challenges, Config, Pallet, PendingChallenges, PendingChallengesByBucket,
        StorageAgreement, StorageAgreements,
    };
    use frame_support::{
        pallet_prelude::*,
        traits::{ReservableCurrency, UncheckedOnRuntimeUpgrade},
        weights::Weight,
    };
    use frame_system::pallet_prelude::BlockNumberFor;
    use storage_primitives::ProviderRole;

    /// Pre-#301 layouts, used only to decode the old values.
    mod old {
        use super::*;
        use sp_core::H256;
        use storage_primitives::{ChunkLocation, Commitment};

        #[derive(Decode)]
        #[allow(dead_code)]
        pub struct Challenge<AccountId, Balance> {
            pub bucket_id: u64,
            pub provider: AccountId,
            pub challenger: AccountId,
            pub mmr_root: H256,
            pub start_seq: u64,
            pub target: ChunkLocation,
            pub deposit: Balance,
        }

        #[derive(Decode)]
        #[allow(dead_code)]
        pub struct ReplicaSyncRecord<BlockNumber> {
            pub commitment: Commitment,
            pub block: BlockNumber,
        }

        #[derive(Decode)]
        #[allow(dead_code)]
        pub enum ProviderRole<Balance, BlockNumber> {
            Primary,
            Replica {
                sync_balance: Balance,
                sync_price: Balance,
                min_sync_interval: BlockNumber,
                last_sync: Option<ReplicaSyncRecord<BlockNumber>>,
            },
        }

        #[derive(Decode)]
        pub struct StorageAgreement<AccountId, Balance, BlockNumber> {
            pub owner: AccountId,
            pub max_bytes: u64,
            pub payment_locked: Balance,
            pub price_per_byte: Balance,
            pub expires_at: BlockNumber,
            pub extensions_blocked: bool,
            pub role: ProviderRole<Balance, BlockNumber>,
            pub started_at: BlockNumber,
        }
    }

    type OldChallenge<T> = old::Challenge<<T as frame_system::Config>::AccountId, BalanceOf<T>>;
    type OldStorageAgreement<T> = old::StorageAgreement<
        <T as frame_system::Config>::AccountId,
        BalanceOf<T>,
        BlockNumberFor<T>,
    >;

    pub struct InnerMigrateV0ToV1<T>(core::marker::PhantomData<T>);

    impl<T: Config> UncheckedOnRuntimeUpgrade for InnerMigrateV0ToV1<T> {
        fn on_runtime_upgrade() -> Weight {
            // Drain every pending challenge, refunding the challenger's
            // deposit. Returning `None` from `translate` removes the entry.
            let mut drained = 0u64;
            Challenges::<T>::translate::<OldChallenge<T>, _>(|_deadline, _index, old| {
                T::Currency::unreserve(&old.challenger, old.deposit);
                drained = drained.saturating_add(1);
                None
            });

            // With every challenge drained the pending counters are all zero;
            // dropping the entries is equivalent and cheaper than decrementing.
            let cleared = PendingChallenges::<T>::clear(u32::MAX, None).unique as u64;
            let cleared_by_bucket =
                PendingChallengesByBucket::<T>::clear(u32::MAX, None).unique as u64;

            // Re-store agreements in the new layout, dropping old sync records.
            let mut agreements = 0u64;
            StorageAgreements::<T>::translate::<OldStorageAgreement<T>, _>(
                |_bucket_id, _provider, old| {
                    agreements = agreements.saturating_add(1);
                    Some(StorageAgreement::<T> {
                        owner: old.owner,
                        max_bytes: old.max_bytes,
                        payment_locked: old.payment_locked,
                        price_per_byte: old.price_per_byte,
                        expires_at: old.expires_at,
                        extensions_blocked: old.extensions_blocked,
                        role: match old.role {
                            old::ProviderRole::Primary => ProviderRole::Primary,
                            old::ProviderRole::Replica {
                                sync_balance,
                                sync_price,
                                min_sync_interval,
                                last_sync: _,
                            } => ProviderRole::Replica {
                                sync_balance,
                                sync_price,
                                min_sync_interval,
                                last_sync: None,
                            },
                        },
                        started_at: old.started_at,
                    })
                },
            );

            let touched = drained
                .saturating_add(cleared)
                .saturating_add(cleared_by_bucket)
                .saturating_add(agreements);
            // One read + one write per touched entry, plus one balance write
            // per refunded deposit.
            T::DbWeight::get().reads_writes(touched, touched.saturating_add(drained))
        }

        #[cfg(feature = "try-runtime")]
        fn pre_upgrade() -> Result<alloc::vec::Vec<u8>, sp_runtime::TryRuntimeError> {
            let agreements = StorageAgreements::<T>::iter_keys().count() as u64;
            Ok(agreements.encode())
        }

        #[cfg(feature = "try-runtime")]
        fn post_upgrade(state: alloc::vec::Vec<u8>) -> Result<(), sp_runtime::TryRuntimeError> {
            let before = u64::decode(&mut &state[..]).map_err(|_| "invalid pre_upgrade state")?;
            // `iter()` fully decodes values under the NEW layout; a short count
            // means an entry failed to decode (or was dropped).
            let after = StorageAgreements::<T>::iter().count() as u64;
            ensure!(
                before == after,
                "StorageAgreements entry count changed during migration"
            );
            ensure!(
                Challenges::<T>::iter().next().is_none(),
                "Challenges must be empty after migration"
            );
            ensure!(
                PendingChallenges::<T>::iter().next().is_none(),
                "PendingChallenges must be empty after migration"
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
