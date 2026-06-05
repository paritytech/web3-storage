use super::*;

fn setup_provider_and_bucket() {
    let multiaddr = b"/ip4/127.0.0.1/tcp/3000".to_vec();
    assert_ok!(StorageProvider::register_provider(
        RuntimeOrigin::signed(2),
        multiaddr.try_into().unwrap(),
        test_public_key(),
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

        let request = AgreementRequests::<Test>::get(0, 2).unwrap();
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

        // max_bytes = 100 fits within stake of 200 (MinStakePerByte = 1)
        // payment = price_per_byte(0) * max_bytes * duration = 0
        assert_ok!(StorageProvider::request_primary_agreement(
            RuntimeOrigin::signed(1),
            0,
            2,
            100,
            100,
            1000
        ));

        assert_ok!(StorageProvider::accept_agreement(
            RuntimeOrigin::signed(2),
            0
        ));

        // Check agreement created
        let agreement = StorageAgreements::<Test>::get(0, 2).unwrap();
        assert_eq!(agreement.owner, 1);
        assert_eq!(agreement.max_bytes, 100);
        assert!(matches!(agreement.role, ProviderRole::Primary));

        // Check provider added to bucket
        let bucket = Buckets::<Test>::get(0).unwrap();
        assert!(bucket.primary_providers.contains(&2));

        // Check provider stats updated
        let provider = Providers::<Test>::get(2).unwrap();
        assert_eq!(provider.committed_bytes, 100);
        assert_eq!(provider.stats.agreements_total, 1);

        // Check request removed
        assert!(AgreementRequests::<Test>::get(0, 2).is_none());
    });
}

#[test]
fn reject_agreement_returns_funds() {
    new_test_ext().execute_with(|| {
        // Setup provider with non-zero price so funds are actually reserved
        let multiaddr = b"/ip4/127.0.0.1/tcp/3000".to_vec();
        assert_ok!(StorageProvider::register_provider(
            RuntimeOrigin::signed(2),
            multiaddr.try_into().unwrap(),
            test_public_key(),
            200
        ));

        let settings = ProviderSettings {
            min_duration: 0u64,
            max_duration: 1000u64,
            price_per_byte: 1u64, // Non-zero price
            accepting_primary: true,
            replica_sync_price: None,
            accepting_extensions: true,
            max_capacity: 0,
        };
        assert_ok!(StorageProvider::update_provider_settings(
            RuntimeOrigin::signed(2),
            settings
        ));

        assert_ok!(StorageProvider::create_bucket(RuntimeOrigin::signed(1), 1));

        let balance_before = Balances::free_balance(1);

        // Request agreement for 100 bytes at price 1, duration 10
        // payment = 1 * 100 * 10 = 1000
        assert_ok!(StorageProvider::request_primary_agreement(
            RuntimeOrigin::signed(1),
            0,
            2,
            100,
            10,
            1000
        ));

        // Some funds should be reserved (1000)
        assert!(Balances::free_balance(1) < balance_before);
        assert_eq!(Balances::free_balance(1), balance_before - 1000);

        assert_ok!(StorageProvider::reject_agreement(
            RuntimeOrigin::signed(2),
            0
        ));

        // Funds should be returned
        assert_eq!(Balances::free_balance(1), balance_before);

        // Request should be removed
        assert!(AgreementRequests::<Test>::get(0, 2).is_none());
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
        assert!(AgreementRequests::<Test>::get(0, 2).is_none());
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
                test_public_key(),
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
            assert_ok!(StorageProvider::accept_agreement(
                RuntimeOrigin::signed(i),
                0
            ));
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
