// SPDX-License-Identifier: Apache-2.0

//! Storage migrations for `pallet-storage-provider`.

/// v0 -> v1: `Challenge` gained `leaf_count`, needed to verify a challenge
/// response proves the exact challenged leaf position, not merely some leaf
/// under the challenged root (see `verify_mmr_proof_at`).
///
/// An in-flight challenge's `leaf_count` was never recorded under v0, so an
/// old value isn't translatable to the new layout: the migration drops every
/// pending challenge, refunding each challenger's deposit, and resets the
/// pending-challenge counters.
pub mod v1 {
    use crate::pallet::{
        BalanceOf, Challenges, Config, Pallet, PendingChallenges, PendingChallengesByBucket,
    };
    use frame_support::{pallet_prelude::*, traits::UncheckedOnRuntimeUpgrade, weights::Weight};

    /// Pre-`leaf_count` layout, used only to decode old values.
    mod old {
        use super::*;
        use sp_core::H256;
        use storage_primitives::{BucketId, ChunkLocation};

        #[derive(Decode)]
        #[allow(dead_code)]
        pub struct Challenge<AccountId, Balance> {
            pub bucket_id: BucketId,
            pub provider: AccountId,
            pub challenger: AccountId,
            pub mmr_root: H256,
            pub start_seq: u64,
            pub target: ChunkLocation,
            pub deposit: Balance,
            pub authorized: bool,
        }
    }

    type OldChallenge<T> = old::Challenge<<T as frame_system::Config>::AccountId, BalanceOf<T>>;

    /// The actual v0 -> v1 upgrade logic, without the storage-version gate.
    /// Use [`MigrateV0ToV1`] instead; this is exposed for `try-runtime` checks.
    pub struct InnerMigrateV0ToV1<T>(core::marker::PhantomData<T>);

    impl<T: Config> UncheckedOnRuntimeUpgrade for InnerMigrateV0ToV1<T> {
        fn on_runtime_upgrade() -> Weight {
            // Drain every pending challenge, refunding the challenger's
            // deposit. Returning `None` from `translate` removes the entry.
            let mut drained = 0u64;
            Challenges::<T>::translate::<OldChallenge<T>, _>(|_deadline, _index, old| {
                Pallet::<T>::release_challenge_deposit(&old.challenger, old.deposit);
                drained = drained.saturating_add(1);
                None
            });

            // With every challenge drained the pending counters are all zero;
            // dropping the entries is equivalent and cheaper than decrementing.
            let cleared = PendingChallenges::<T>::clear(u32::MAX, None).unique as u64;
            let cleared_by_bucket =
                PendingChallengesByBucket::<T>::clear(u32::MAX, None).unique as u64;

            let touched = drained
                .saturating_add(cleared)
                .saturating_add(cleared_by_bucket);
            // One read + one write per touched entry, plus one balance release
            // per refunded deposit.
            T::DbWeight::get().reads_writes(touched, touched.saturating_add(drained))
        }

        #[cfg(feature = "try-runtime")]
        fn pre_upgrade() -> Result<alloc::vec::Vec<u8>, sp_runtime::TryRuntimeError> {
            Ok(alloc::vec::Vec::new())
        }

        #[cfg(feature = "try-runtime")]
        fn post_upgrade(_state: alloc::vec::Vec<u8>) -> Result<(), sp_runtime::TryRuntimeError> {
            ensure!(
                Challenges::<T>::iter().next().is_none(),
                "Challenges must be empty after migration"
            );
            ensure!(
                PendingChallenges::<T>::iter().next().is_none(),
                "PendingChallenges must be empty after migration"
            );
            ensure!(
                PendingChallengesByBucket::<T>::iter().next().is_none(),
                "PendingChallengesByBucket must be empty after migration"
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
    use crate::pallet::{Challenges, HoldReason, PendingChallenges, PendingChallengesByBucket};
    use codec::Encode;
    use frame_support::storage::unhashed;
    use frame_support::traits::{
        fungible::{InspectHold, Mutate, MutateHold},
        UncheckedOnRuntimeUpgrade,
    };
    use sp_core::H256;
    use storage_primitives::ChunkLocation;

    // Encode-side mirror of the pre-`leaf_count` layout, with the mock's
    // concrete types (AccountId / Balance = u64), used to plant a raw
    // old-layout value that the current type cannot decode.
    #[derive(Encode)]
    struct OldChallenge {
        bucket_id: u64,
        provider: u64,
        challenger: u64,
        mmr_root: H256,
        start_seq: u64,
        target: ChunkLocation,
        deposit: u64,
        authorized: bool,
    }

    #[test]
    fn v1_drains_challenges_refunds_deposits_and_zeroes_pending_counters() {
        new_test_ext().execute_with(|| {
            let challenger = 3u64;
            let deposit = 50u64;
            // +1: `hold` keeps the existential deposit free, so holding the
            // whole free balance would fail with `FundsUnavailable`.
            let _ = Balances::set_balance(&challenger, deposit + 1);
            Balances::hold(&HoldReason::ChallengeDeposit.into(), &challenger, deposit).unwrap();

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
                authorized: true,
            };
            unhashed::put_raw(
                &Challenges::<Test>::hashed_key_for(100u64, 0u16),
                &challenge.encode(),
            );
            PendingChallenges::<Test>::insert(2u64, 1u32);
            PendingChallengesByBucket::<Test>::insert(7u64, 2u64, 1u32);

            v1::InnerMigrateV0ToV1::<Test>::on_runtime_upgrade();

            assert!(Challenges::<Test>::iter().next().is_none());
            assert_eq!(
                Balances::balance_on_hold(&HoldReason::ChallengeDeposit.into(), &challenger),
                0
            );
            assert!(PendingChallenges::<Test>::iter().next().is_none());
            assert!(PendingChallengesByBucket::<Test>::iter().next().is_none());
        });
    }
}
