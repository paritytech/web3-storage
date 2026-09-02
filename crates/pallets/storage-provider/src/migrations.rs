// SPDX-License-Identifier: Apache-2.0

//! Storage migrations for `pallet-storage-provider`.

extern crate alloc;

/// v0 -> v1: the leaf-index binding (#301) reshaped `Challenge` and
/// `ReplicaSyncRecord`, and neither old value is translatable — an in-flight
/// challenge's `leaf_count` was never recorded, and an old sync record cannot
/// show its range was the current snapshot. Drain pending challenges
/// (refunding deposits, resetting the pending counters) and drop replica
/// `last_sync` records so replicas re-sync before being challengeable.
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

#[cfg(test)]
mod tests {
    use super::v1;
    use crate::mock::{new_test_ext, Balances, Test};
    use crate::pallet::{
        Challenges, PendingChallenges, PendingChallengesByBucket, StorageAgreements,
    };
    use codec::Encode;
    use frame_support::storage::unhashed;
    use frame_support::traits::{ReservableCurrency, UncheckedOnRuntimeUpgrade};
    use sp_core::H256;
    use storage_primitives::{ChunkLocation, Commitment, ProviderRole};

    // Encode-side mirrors of the pre-#301 layouts, with the mock's concrete
    // types (AccountId / Balance / BlockNumber = u64), used to plant raw
    // old-layout values that the current types cannot decode.
    #[derive(Encode)]
    struct OldChallenge {
        bucket_id: u64,
        provider: u64,
        challenger: u64,
        mmr_root: H256,
        start_seq: u64,
        target: ChunkLocation,
        deposit: u64,
    }

    #[derive(Encode)]
    struct OldReplicaSyncRecord {
        commitment: Commitment,
        block: u64,
    }

    #[derive(Encode)]
    enum OldProviderRole {
        #[codec(index = 1)]
        Replica {
            sync_balance: u64,
            sync_price: u64,
            min_sync_interval: u64,
            last_sync: Option<OldReplicaSyncRecord>,
        },
    }

    #[derive(Encode)]
    struct OldStorageAgreement {
        owner: u64,
        max_bytes: u64,
        payment_locked: u64,
        price_per_byte: u64,
        expires_at: u64,
        extensions_blocked: bool,
        role: OldProviderRole,
        started_at: u64,
    }

    #[test]
    fn v1_drains_challenges_refunds_deposits_and_drops_sync_records() {
        new_test_ext().execute_with(|| {
            let challenger = 3u64;
            let deposit = 50u64;
            Balances::reserve(&challenger, deposit).unwrap();

            let challenge = OldChallenge {
                bucket_id: 7,
                provider: 2,
                challenger,
                mmr_root: H256::repeat_byte(0xAA),
                start_seq: 0,
                target: ChunkLocation {
                    leaf_index: 1,
                    chunk_index: 0,
                },
                deposit,
            };
            unhashed::put_raw(
                &Challenges::<Test>::hashed_key_for(100u64, 0u16),
                &challenge.encode(),
            );
            PendingChallenges::<Test>::insert(2u64, 1u32);
            PendingChallengesByBucket::<Test>::insert(7u64, 2u64, 1u32);

            let agreement = OldStorageAgreement {
                owner: 1,
                max_bytes: 1024,
                payment_locked: 500,
                price_per_byte: 2,
                expires_at: 999,
                extensions_blocked: false,
                role: OldProviderRole::Replica {
                    sync_balance: 40,
                    sync_price: 5,
                    min_sync_interval: 10,
                    last_sync: Some(OldReplicaSyncRecord {
                        commitment: Commitment {
                            mmr_root: H256::repeat_byte(0xBB),
                            start_seq: 0,
                            leaf_count: 3,
                        },
                        block: 12,
                    }),
                },
                started_at: 5,
            };
            unhashed::put_raw(
                &StorageAgreements::<Test>::hashed_key_for(7u64, 2u64),
                &agreement.encode(),
            );

            v1::InnerMigrateV0ToV1::<Test>::on_runtime_upgrade();

            assert!(Challenges::<Test>::iter().next().is_none());
            assert_eq!(Balances::reserved_balance(challenger), 0);
            assert!(PendingChallenges::<Test>::iter().next().is_none());
            assert!(PendingChallengesByBucket::<Test>::iter().next().is_none());

            let migrated = StorageAgreements::<Test>::get(7, 2)
                .expect("agreement must decode under the new layout");
            assert_eq!(migrated.owner, 1);
            assert_eq!(migrated.max_bytes, 1024);
            assert_eq!(migrated.payment_locked, 500);
            assert_eq!(migrated.expires_at, 999);
            match migrated.role {
                ProviderRole::Replica {
                    sync_balance,
                    sync_price,
                    min_sync_interval,
                    last_sync,
                } => {
                    assert_eq!((sync_balance, sync_price, min_sync_interval), (40, 5, 10));
                    assert!(last_sync.is_none());
                }
                ProviderRole::Primary => panic!("role must remain Replica"),
            }
        });
    }
}
