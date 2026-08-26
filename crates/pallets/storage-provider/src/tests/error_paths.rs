// SPDX-License-Identifier: Apache-2.0

use super::*;
use storage_primitives::{Commitment, Role};

#[test]
fn create_bucket_fails_too_many_buckets() {
    // MaxBucketsPerMember = 100 in mock, so create 100 then fail on 101st
    new_test_ext().execute_with(|| {
        for _ in 0..100 {
            create_bucket(1, 0);
        }

        // `create_bucket_internal` mutates NextBucketId before the member
        // index push fails, so only assert the error.
        assert_err!(
            StorageProvider::create_bucket_internal(&1, 0, None),
            Error::<Test>::TooManyBucketsForMember
        );
    });
}

#[test]
fn set_member_fails_max_members() {
    new_test_ext().execute_with(|| {
        create_bucket(1, 0);

        // MaxMembers = 100 in mock. Account 1 is already a member (admin).
        // Add 99 more to hit the limit.
        for i in 2..=100 {
            assert_ok!(StorageProvider::set_member(
                RuntimeOrigin::signed(1),
                0,
                i,
                Role::Reader
            ));
        }

        // 101st member should fail
        assert_noop!(
            StorageProvider::set_member(RuntimeOrigin::signed(1), 0, 101, Role::Reader),
            Error::<Test>::MaxMembersReached
        );
    });
}

#[test]
fn freeze_bucket_fails_min_providers_not_met() {
    new_test_ext().execute_with(|| {
        register_provider(2, 200);
        let bucket_id = setup_agreement(2, 1, 50, 200);

        // Set min_providers to 0 first so we can create a snapshot
        assert_ok!(StorageProvider::set_min_providers(
            RuntimeOrigin::signed(1),
            bucket_id,
            0
        ));

        // Create snapshot with no sigs
        assert_ok!(StorageProvider::checkpoint(
            RuntimeOrigin::signed(1),
            bucket_id,
            Commitment {
                mmr_root: sp_core::H256::repeat_byte(0xAA),
                start_seq: 0,
                leaf_count: 10,
            },
            Default::default(),
        ));

        // Now set min_providers to 2 (but only 0 signers in snapshot)
        // Can't set above primary_providers.len() - which is 1
        // So set to 1 and the snapshot has 0 signers
        assert_ok!(StorageProvider::set_min_providers(
            RuntimeOrigin::signed(1),
            bucket_id,
            1
        ));

        assert_noop!(
            StorageProvider::freeze_bucket(RuntimeOrigin::signed(1), bucket_id),
            Error::<Test>::MinProvidersNotMet
        );
    });
}

#[test]
fn end_agreement_cannot_terminate_replica_early() {
    new_test_ext().execute_with(|| {
        register_provider_with_settings(
            2,
            200,
            ProviderSettings {
                accepting_primary: true,
                replica_sync_price: Some(10),
                ..Default::default()
            },
        );
        register_provider(3, 200);
        let bucket_id = setup_agreement(3, 1, 50, 200);

        // Create replica agreement for provider 2
        setup_replica_agreement(
            2,
            1,
            bucket_id,
            50,
            200,
            storage_primitives::ReplicaTerms {
                sync_balance: 100,
                min_sync_interval: 10,
                sync_price: 10,
            },
        );

        // Admin tries to early-terminate replica
        assert_noop!(
            StorageProvider::end_agreement(
                RuntimeOrigin::signed(1),
                bucket_id,
                2,
                storage_primitives::EndAction::Pay
            ),
            Error::<Test>::CannotTerminateReplica
        );
    });
}

#[test]
fn set_min_providers_fails_not_admin() {
    new_test_ext().execute_with(|| {
        create_bucket(1, 1);

        assert_noop!(
            StorageProvider::set_min_providers(RuntimeOrigin::signed(3), 0, 0),
            Error::<Test>::NotBucketAdmin
        );
    });
}

#[test]
fn set_min_providers_fails_exceeds_providers() {
    new_test_ext().execute_with(|| {
        create_bucket(1, 0);

        // Bucket has 0 primary providers, can't set min_providers to 1
        assert_noop!(
            StorageProvider::set_min_providers(RuntimeOrigin::signed(1), 0, 1),
            Error::<Test>::InvalidMinProviders
        );
    });
}

#[test]
fn freeze_bucket_already_frozen() {
    new_test_ext().execute_with(|| {
        create_bucket(1, 0);

        // Create snapshot
        assert_ok!(StorageProvider::checkpoint(
            RuntimeOrigin::signed(1),
            0,
            Commitment {
                mmr_root: sp_core::H256::repeat_byte(0xAA),
                start_seq: 0,
                leaf_count: 10,
            },
            Default::default(),
        ));

        assert_ok!(StorageProvider::freeze_bucket(RuntimeOrigin::signed(1), 0));

        assert_noop!(
            StorageProvider::freeze_bucket(RuntimeOrigin::signed(1), 0),
            Error::<Test>::BucketFrozen
        );
    });
}
