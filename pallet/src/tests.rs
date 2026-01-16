//! Tests for the storage provider pallet.

use crate::{mock::*, *};
use frame_support::{assert_noop, assert_ok};
use sp_runtime::traits::BadOrigin;

mod provider_tests {
    use super::*;

    #[test]
    fn register_provider_works() {
        new_test_ext().execute_with(|| {
            let multiaddr = b"/ip4/127.0.0.1/tcp/3000".to_vec();

            assert_ok!(StorageProvider::register_provider(
                RuntimeOrigin::signed(1),
                multiaddr.clone().try_into().unwrap(),
                200
            ));

            let provider = Providers::<Test>::get(1).unwrap();
            assert_eq!(provider.stake, 200);
            assert_eq!(provider.multiaddr.to_vec(), multiaddr);
            assert_eq!(provider.committed_bytes, 0);
        });
    }

    #[test]
    fn register_provider_fails_with_insufficient_stake() {
        new_test_ext().execute_with(|| {
            let multiaddr = b"/ip4/127.0.0.1/tcp/3000".to_vec();

            assert_noop!(
                StorageProvider::register_provider(
                    RuntimeOrigin::signed(1),
                    multiaddr.try_into().unwrap(),
                    50 // Below minimum of 100
                ),
                Error::<Test>::InsufficientStake
            );
        });
    }

    #[test]
    fn register_provider_fails_if_already_registered() {
        new_test_ext().execute_with(|| {
            let multiaddr = b"/ip4/127.0.0.1/tcp/3000".to_vec();

            assert_ok!(StorageProvider::register_provider(
                RuntimeOrigin::signed(1),
                multiaddr.clone().try_into().unwrap(),
                200
            ));

            assert_noop!(
                StorageProvider::register_provider(
                    RuntimeOrigin::signed(1),
                    multiaddr.try_into().unwrap(),
                    200
                ),
                Error::<Test>::ProviderAlreadyRegistered
            );
        });
    }

    #[test]
    fn add_stake_works() {
        new_test_ext().execute_with(|| {
            let multiaddr = b"/ip4/127.0.0.1/tcp/3000".to_vec();

            assert_ok!(StorageProvider::register_provider(
                RuntimeOrigin::signed(1),
                multiaddr.try_into().unwrap(),
                200
            ));

            assert_ok!(StorageProvider::add_stake(RuntimeOrigin::signed(1), 100));

            let provider = Providers::<Test>::get(1).unwrap();
            assert_eq!(provider.stake, 300);
        });
    }

    #[test]
    fn add_stake_fails_if_not_registered() {
        new_test_ext().execute_with(|| {
            assert_noop!(
                StorageProvider::add_stake(RuntimeOrigin::signed(1), 100),
                Error::<Test>::ProviderNotFound
            );
        });
    }

    #[test]
    fn deregister_provider_works() {
        new_test_ext().execute_with(|| {
            let multiaddr = b"/ip4/127.0.0.1/tcp/3000".to_vec();

            assert_ok!(StorageProvider::register_provider(
                RuntimeOrigin::signed(1),
                multiaddr.try_into().unwrap(),
                200
            ));

            let balance_before = Balances::free_balance(1);

            assert_ok!(StorageProvider::deregister_provider(RuntimeOrigin::signed(1)));

            // Check provider removed
            assert!(Providers::<Test>::get(1).is_none());

            // Check stake returned
            assert_eq!(Balances::free_balance(1), balance_before + 200);
        });
    }

    #[test]
    fn deregister_provider_fails_with_active_agreements() {
        new_test_ext().execute_with(|| {
            // Setup provider and bucket
            let multiaddr = b"/ip4/127.0.0.1/tcp/3000".to_vec();
            assert_ok!(StorageProvider::register_provider(
                RuntimeOrigin::signed(2),
                multiaddr.try_into().unwrap(),
                200
            ));
            assert_ok!(StorageProvider::create_bucket(RuntimeOrigin::signed(1), 1));

            // Create agreement
            assert_ok!(StorageProvider::request_primary_agreement(
                RuntimeOrigin::signed(1),
                0,
                2,
                1000,
                100,
                1000
            ));
            assert_ok!(StorageProvider::accept_agreement(RuntimeOrigin::signed(2), 0));

            // Try to deregister
            assert_noop!(
                StorageProvider::deregister_provider(RuntimeOrigin::signed(2)),
                Error::<Test>::ProviderHasActiveAgreements
            );
        });
    }

    #[test]
    fn update_provider_settings_works() {
        new_test_ext().execute_with(|| {
            let multiaddr = b"/ip4/127.0.0.1/tcp/3000".to_vec();

            assert_ok!(StorageProvider::register_provider(
                RuntimeOrigin::signed(1),
                multiaddr.try_into().unwrap(),
                200
            ));

            let new_settings = ProviderSettings {
                min_duration: 10u64,
                max_duration: 1000u64,
                price_per_byte: 5u64,
                accepting_primary: true,
                replica_sync_price: Some(10u64),
                accepting_extensions: true,
            };

            assert_ok!(StorageProvider::update_provider_settings(
                RuntimeOrigin::signed(1),
                new_settings.clone()
            ));

            let provider = Providers::<Test>::get(1).unwrap();
            assert_eq!(provider.settings.price_per_byte, 5);
            assert_eq!(provider.settings.replica_sync_price, Some(10));
        });
    }
}

mod bucket_tests {
    use super::*;

    #[test]
    fn create_bucket_works() {
        new_test_ext().execute_with(|| {
            assert_ok!(StorageProvider::create_bucket(RuntimeOrigin::signed(1), 2));

            let bucket = Buckets::<Test>::get(0).unwrap();
            assert_eq!(bucket.min_providers, 2);
            assert_eq!(bucket.members.len(), 1);
            assert_eq!(bucket.members[0].account, 1);
            assert_eq!(bucket.members[0].role, Role::Admin);
            assert!(bucket.snapshot.is_none());
            assert!(bucket.frozen_start_seq.is_none());

            // Check bucket ID incremented
            assert_eq!(NextBucketId::<Test>::get(), 1);
        });
    }

    #[test]
    fn create_multiple_buckets_increments_id() {
        new_test_ext().execute_with(|| {
            assert_ok!(StorageProvider::create_bucket(RuntimeOrigin::signed(1), 1));
            assert_ok!(StorageProvider::create_bucket(RuntimeOrigin::signed(2), 2));
            assert_ok!(StorageProvider::create_bucket(RuntimeOrigin::signed(1), 3));

            assert_eq!(NextBucketId::<Test>::get(), 3);
            assert!(Buckets::<Test>::get(0).is_some());
            assert!(Buckets::<Test>::get(1).is_some());
            assert!(Buckets::<Test>::get(2).is_some());
        });
    }

    #[test]
    fn set_member_works() {
        new_test_ext().execute_with(|| {
            assert_ok!(StorageProvider::create_bucket(RuntimeOrigin::signed(1), 1));

            // Add writer
            assert_ok!(StorageProvider::set_member(
                RuntimeOrigin::signed(1),
                0,
                2,
                Role::Writer
            ));

            let bucket = Buckets::<Test>::get(0).unwrap();
            assert_eq!(bucket.members.len(), 2);

            let writer = bucket.members.iter().find(|m| m.account == 2).unwrap();
            assert_eq!(writer.role, Role::Writer);
        });
    }

    #[test]
    fn set_member_updates_existing_role() {
        new_test_ext().execute_with(|| {
            assert_ok!(StorageProvider::create_bucket(RuntimeOrigin::signed(1), 1));

            // Add as writer
            assert_ok!(StorageProvider::set_member(
                RuntimeOrigin::signed(1),
                0,
                2,
                Role::Writer
            ));

            // Promote to admin
            assert_ok!(StorageProvider::set_member(
                RuntimeOrigin::signed(1),
                0,
                2,
                Role::Admin
            ));

            let bucket = Buckets::<Test>::get(0).unwrap();
            let member = bucket.members.iter().find(|m| m.account == 2).unwrap();
            assert_eq!(member.role, Role::Admin);
        });
    }

    #[test]
    fn set_member_fails_for_non_admin() {
        new_test_ext().execute_with(|| {
            assert_ok!(StorageProvider::create_bucket(RuntimeOrigin::signed(1), 1));

            // Non-admin tries to add member
            assert_noop!(
                StorageProvider::set_member(RuntimeOrigin::signed(2), 0, 3, Role::Writer),
                Error::<Test>::NotBucketAdmin
            );
        });
    }

    #[test]
    fn cannot_demote_other_admin() {
        new_test_ext().execute_with(|| {
            assert_ok!(StorageProvider::create_bucket(RuntimeOrigin::signed(1), 1));

            // Add second admin
            assert_ok!(StorageProvider::set_member(
                RuntimeOrigin::signed(1),
                0,
                2,
                Role::Admin
            ));

            // Admin 1 tries to demote admin 2
            assert_noop!(
                StorageProvider::set_member(RuntimeOrigin::signed(1), 0, 2, Role::Writer),
                Error::<Test>::CannotDemoteAdmin
            );
        });
    }

    #[test]
    fn admin_can_demote_self() {
        new_test_ext().execute_with(|| {
            assert_ok!(StorageProvider::create_bucket(RuntimeOrigin::signed(1), 1));

            // Add second admin
            assert_ok!(StorageProvider::set_member(
                RuntimeOrigin::signed(1),
                0,
                2,
                Role::Admin
            ));

            // Admin 1 demotes self
            assert_ok!(StorageProvider::set_member(
                RuntimeOrigin::signed(1),
                0,
                1,
                Role::Writer
            ));

            let bucket = Buckets::<Test>::get(0).unwrap();
            let member = bucket.members.iter().find(|m| m.account == 1).unwrap();
            assert_eq!(member.role, Role::Writer);
        });
    }

    #[test]
    fn remove_member_works() {
        new_test_ext().execute_with(|| {
            assert_ok!(StorageProvider::create_bucket(RuntimeOrigin::signed(1), 1));
            assert_ok!(StorageProvider::set_member(
                RuntimeOrigin::signed(1),
                0,
                2,
                Role::Writer
            ));

            assert_ok!(StorageProvider::remove_member(RuntimeOrigin::signed(1), 0, 2));

            let bucket = Buckets::<Test>::get(0).unwrap();
            assert_eq!(bucket.members.len(), 1);
            assert!(bucket.members.iter().find(|m| m.account == 2).is_none());
        });
    }

    #[test]
    fn remove_member_fails_for_non_existent() {
        new_test_ext().execute_with(|| {
            assert_ok!(StorageProvider::create_bucket(RuntimeOrigin::signed(1), 1));

            assert_noop!(
                StorageProvider::remove_member(RuntimeOrigin::signed(1), 0, 99),
                Error::<Test>::MemberNotFound
            );
        });
    }

    #[test]
    fn set_min_providers_works() {
        new_test_ext().execute_with(|| {
            assert_ok!(StorageProvider::create_bucket(RuntimeOrigin::signed(1), 2));

            // Can set to 0 (no minimum)
            assert_ok!(StorageProvider::set_min_providers(
                RuntimeOrigin::signed(1),
                0,
                0
            ));

            let bucket = Buckets::<Test>::get(0).unwrap();
            assert_eq!(bucket.min_providers, 0);
        });
    }

    #[test]
    fn freeze_bucket_requires_snapshot() {
        new_test_ext().execute_with(|| {
            assert_ok!(StorageProvider::create_bucket(RuntimeOrigin::signed(1), 1));

            assert_noop!(
                StorageProvider::freeze_bucket(RuntimeOrigin::signed(1), 0),
                Error::<Test>::NoSnapshot
            );
        });
    }
}

mod agreement_tests {
    use super::*;

    fn setup_provider_and_bucket() {
        let multiaddr = b"/ip4/127.0.0.1/tcp/3000".to_vec();
        assert_ok!(StorageProvider::register_provider(
            RuntimeOrigin::signed(2),
            multiaddr.try_into().unwrap(),
            200
        ));
        assert_ok!(StorageProvider::create_bucket(RuntimeOrigin::signed(1), 1));
    }

    #[test]
    fn request_primary_agreement_works() {
        new_test_ext().execute_with(|| {
            setup_provider_and_bucket();

            assert_ok!(StorageProvider::request_primary_agreement(
                RuntimeOrigin::signed(1),
                0,    // bucket_id
                2,    // provider
                1000, // max_bytes
                100,  // duration
                1000  // max_payment
            ));

            let request = AgreementRequests::<Test>::get(2, 0).unwrap();
            assert_eq!(request.requester, 1);
            assert_eq!(request.max_bytes, 1000);
            assert_eq!(request.duration, 100);
            assert!(request.replica_params.is_none());
        });
    }

    #[test]
    fn request_primary_agreement_fails_for_non_admin() {
        new_test_ext().execute_with(|| {
            setup_provider_and_bucket();

            assert_noop!(
                StorageProvider::request_primary_agreement(
                    RuntimeOrigin::signed(3), // Not admin
                    0,
                    2,
                    1000,
                    100,
                    1000
                ),
                Error::<Test>::NotBucketAdmin
            );
        });
    }

    #[test]
    fn accept_agreement_works() {
        new_test_ext().execute_with(|| {
            setup_provider_and_bucket();

            assert_ok!(StorageProvider::request_primary_agreement(
                RuntimeOrigin::signed(1),
                0,
                2,
                1000,
                100,
                1000
            ));

            assert_ok!(StorageProvider::accept_agreement(RuntimeOrigin::signed(2), 0));

            // Check agreement created
            let agreement = StorageAgreements::<Test>::get(0, 2).unwrap();
            assert_eq!(agreement.owner, 1);
            assert_eq!(agreement.max_bytes, 1000);
            assert!(matches!(agreement.role, ProviderRole::Primary));

            // Check provider added to bucket
            let bucket = Buckets::<Test>::get(0).unwrap();
            assert!(bucket.primary_providers.contains(&2));

            // Check provider stats updated
            let provider = Providers::<Test>::get(2).unwrap();
            assert_eq!(provider.committed_bytes, 1000);
            assert_eq!(provider.stats.agreements_total, 1);

            // Check request removed
            assert!(AgreementRequests::<Test>::get(2, 0).is_none());
        });
    }

    #[test]
    fn reject_agreement_returns_funds() {
        new_test_ext().execute_with(|| {
            setup_provider_and_bucket();

            let balance_before = Balances::free_balance(1);

            assert_ok!(StorageProvider::request_primary_agreement(
                RuntimeOrigin::signed(1),
                0,
                2,
                1000,
                100,
                1000
            ));

            // Some funds should be reserved
            assert!(Balances::free_balance(1) < balance_before);

            assert_ok!(StorageProvider::reject_agreement(RuntimeOrigin::signed(2), 0));

            // Funds should be returned
            assert_eq!(Balances::free_balance(1), balance_before);

            // Request should be removed
            assert!(AgreementRequests::<Test>::get(2, 0).is_none());
        });
    }

    #[test]
    fn withdraw_agreement_request_works() {
        new_test_ext().execute_with(|| {
            setup_provider_and_bucket();

            let balance_before = Balances::free_balance(1);

            assert_ok!(StorageProvider::request_primary_agreement(
                RuntimeOrigin::signed(1),
                0,
                2,
                1000,
                100,
                1000
            ));

            assert_ok!(StorageProvider::withdraw_agreement_request(
                RuntimeOrigin::signed(1),
                0,
                2
            ));

            // Funds returned
            assert_eq!(Balances::free_balance(1), balance_before);

            // Request removed
            assert!(AgreementRequests::<Test>::get(2, 0).is_none());
        });
    }

    #[test]
    fn withdraw_fails_for_non_requester() {
        new_test_ext().execute_with(|| {
            setup_provider_and_bucket();

            assert_ok!(StorageProvider::request_primary_agreement(
                RuntimeOrigin::signed(1),
                0,
                2,
                1000,
                100,
                1000
            ));

            assert_noop!(
                StorageProvider::withdraw_agreement_request(
                    RuntimeOrigin::signed(3), // Not the requester
                    0,
                    2
                ),
                Error::<Test>::NotAgreementOwner
            );
        });
    }

    #[test]
    fn max_primary_providers_enforced() {
        new_test_ext().execute_with(|| {
            assert_ok!(StorageProvider::create_bucket(RuntimeOrigin::signed(1), 1));

            // Register 6 providers (max is 5)
            for i in 2..=7 {
                let multiaddr = format!("/ip4/127.0.0.1/tcp/{}", 3000 + i);
                assert_ok!(StorageProvider::register_provider(
                    RuntimeOrigin::signed(i),
                    multiaddr.as_bytes().to_vec().try_into().unwrap(),
                    200
                ));
            }

            // Add 5 providers (should all succeed)
            for i in 2..=6 {
                assert_ok!(StorageProvider::request_primary_agreement(
                    RuntimeOrigin::signed(1),
                    0,
                    i,
                    100,
                    100,
                    1000
                ));
                assert_ok!(StorageProvider::accept_agreement(RuntimeOrigin::signed(i), 0));
            }

            // 6th provider should fail
            assert_noop!(
                StorageProvider::request_primary_agreement(
                    RuntimeOrigin::signed(1),
                    0,
                    7,
                    100,
                    100,
                    1000
                ),
                Error::<Test>::MaxPrimaryProvidersReached
            );
        });
    }
}
