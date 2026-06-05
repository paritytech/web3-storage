use super::*;
use storage_primitives::Role;

#[test]
fn create_bucket_fails_too_many_buckets() {
    // MaxBucketsPerMember = 100 in mock, so create 100 then fail on 101st
    new_test_ext_with_balances(vec![(1, 1_000_000)]).execute_with(|| {
        // Need to register the keystore extension for this externality
        // Since new_test_ext_with_balances doesn't register it, we'll use new_test_ext
        // and rely on its setup
    });

    // Use the standard ext which has keystore
    new_test_ext().execute_with(|| {
        for _ in 0..100 {
            assert_ok!(StorageProvider::create_bucket(RuntimeOrigin::signed(1), 0));
        }

        assert_noop!(
            StorageProvider::create_bucket(RuntimeOrigin::signed(1), 0),
            Error::<Test>::TooManyBucketsForMember
        );
    });
}

#[test]
fn set_member_fails_max_members() {
    new_test_ext().execute_with(|| {
        assert_ok!(StorageProvider::create_bucket(RuntimeOrigin::signed(1), 0));

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
            sp_core::H256::repeat_byte(0xAA),
            0,
            10,
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
fn reject_agreement_fails_no_request() {
    new_test_ext().execute_with(|| {
        register_provider(2, 200);
        assert_ok!(StorageProvider::create_bucket(RuntimeOrigin::signed(1), 1));

        // No pending request for bucket 0 from provider 2
        assert_noop!(
            StorageProvider::reject_agreement(RuntimeOrigin::signed(2), 0),
            Error::<Test>::AgreementRequestNotFound
        );
    });
}

#[test]
fn withdraw_agreement_request_fails_not_found() {
    new_test_ext().execute_with(|| {
        assert_ok!(StorageProvider::create_bucket(RuntimeOrigin::signed(1), 1));

        assert_noop!(
            StorageProvider::withdraw_agreement_request(RuntimeOrigin::signed(1), 0, 2),
            Error::<Test>::AgreementRequestNotFound
        );
    });
}

#[test]
fn request_primary_fails_no_bucket() {
    new_test_ext().execute_with(|| {
        register_provider(2, 200);

        assert_noop!(
            StorageProvider::request_primary_agreement(
                RuntimeOrigin::signed(1),
                999, // non-existent bucket
                2,
                50,
                100,
                1000
            ),
            Error::<Test>::BucketNotFound
        );
    });
}

#[test]
fn accept_agreement_fails_no_request() {
    new_test_ext().execute_with(|| {
        register_provider(2, 200);
        assert_ok!(StorageProvider::create_bucket(RuntimeOrigin::signed(1), 1));

        assert_noop!(
            StorageProvider::accept_agreement(RuntimeOrigin::signed(2), 0),
            Error::<Test>::AgreementRequestNotFound
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
        assert_ok!(StorageProvider::request_agreement(
            RuntimeOrigin::signed(1),
            bucket_id,
            2,
            50,
            200,
            10000,
            storage_primitives::ReplicaRequestParams {
                sync_balance: 100,
                min_sync_interval: 10,
            }
        ));
        assert_ok!(StorageProvider::accept_agreement(
            RuntimeOrigin::signed(2),
            bucket_id
        ));

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
        assert_ok!(StorageProvider::create_bucket(RuntimeOrigin::signed(1), 1));

        assert_noop!(
            StorageProvider::set_min_providers(RuntimeOrigin::signed(3), 0, 0),
            Error::<Test>::NotBucketAdmin
        );
    });
}

#[test]
fn set_min_providers_fails_exceeds_providers() {
    new_test_ext().execute_with(|| {
        assert_ok!(StorageProvider::create_bucket(RuntimeOrigin::signed(1), 0));

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
        assert_ok!(StorageProvider::create_bucket(RuntimeOrigin::signed(1), 0));

        // Create snapshot
        assert_ok!(StorageProvider::checkpoint(
            RuntimeOrigin::signed(1),
            0,
            sp_core::H256::repeat_byte(0xAA),
            0,
            10,
            Default::default(),
        ));

        assert_ok!(StorageProvider::freeze_bucket(RuntimeOrigin::signed(1), 0));

        assert_noop!(
            StorageProvider::freeze_bucket(RuntimeOrigin::signed(1), 0),
            Error::<Test>::BucketFrozen
        );
    });
}

#[test]
fn request_primary_fails_provider_not_found() {
    new_test_ext().execute_with(|| {
        assert_ok!(StorageProvider::create_bucket(RuntimeOrigin::signed(1), 1));

        assert_noop!(
            StorageProvider::request_primary_agreement(
                RuntimeOrigin::signed(1),
                0,
                99, // non-existent provider
                50,
                100,
                1000
            ),
            Error::<Test>::ProviderNotFound
        );
    });
}

#[test]
fn request_primary_fails_not_accepting_primary() {
    new_test_ext().execute_with(|| {
        register_provider_with_settings(
            2,
            200,
            ProviderSettings {
                accepting_primary: false,
                ..Default::default()
            },
        );
        assert_ok!(StorageProvider::create_bucket(RuntimeOrigin::signed(1), 1));

        assert_noop!(
            StorageProvider::request_primary_agreement(
                RuntimeOrigin::signed(1),
                0,
                2,
                50,
                100,
                1000
            ),
            Error::<Test>::ProviderNotAcceptingPrimary
        );
    });
}

#[test]
fn request_primary_fails_duration_too_long() {
    new_test_ext().execute_with(|| {
        register_provider_with_settings(
            2,
            200,
            ProviderSettings {
                max_duration: 50,
                accepting_primary: true,
                ..Default::default()
            },
        );
        assert_ok!(StorageProvider::create_bucket(RuntimeOrigin::signed(1), 1));

        assert_noop!(
            StorageProvider::request_primary_agreement(
                RuntimeOrigin::signed(1),
                0,
                2,
                50,
                100, // exceeds max_duration of 50
                10000
            ),
            Error::<Test>::DurationTooLong
        );
    });
}

#[test]
fn request_primary_fails_payment_exceeds_max() {
    new_test_ext().execute_with(|| {
        register_provider_with_settings(
            2,
            200,
            ProviderSettings {
                price_per_byte: 1,
                accepting_primary: true,
                ..Default::default()
            },
        );
        assert_ok!(StorageProvider::create_bucket(RuntimeOrigin::signed(1), 1));

        // payment = 1 * 50 * 100 = 5000, max_payment = 1
        assert_noop!(
            StorageProvider::request_primary_agreement(
                RuntimeOrigin::signed(1),
                0,
                2,
                50,
                100,
                1 // too low
            ),
            Error::<Test>::PaymentExceedsMax
        );
    });
}
