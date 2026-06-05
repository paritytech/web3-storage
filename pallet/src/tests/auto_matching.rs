use super::*;

#[test]
fn create_bucket_with_storage_works() {
    new_test_ext().execute_with(|| {
        // Register a provider with accepting_primary: true
        let multiaddr = b"/ip4/127.0.0.1/tcp/3000".to_vec();
        assert_ok!(StorageProvider::register_provider(
            RuntimeOrigin::signed(2),
            multiaddr.try_into().unwrap(),
            test_public_key(),
            200
        ));

        // Update settings to accept primary agreements
        // Use price_per_byte: 0 like other tests to avoid balance issues
        let settings = ProviderSettings {
            min_duration: 10u64,
            max_duration: 1000u64,
            price_per_byte: 0u64, // Free storage (like other tests)
            accepting_primary: true,
            replica_sync_price: None,
            accepting_extensions: true,
            max_capacity: 200,
        };
        assert_ok!(StorageProvider::update_provider_settings(
            RuntimeOrigin::signed(2),
            settings
        ));

        // Create bucket with storage requirements
        assert_ok!(StorageProvider::create_bucket_with_storage(
            RuntimeOrigin::signed(1),
            100, // max_bytes
            100, // duration
            10   // max_price_per_byte (higher than provider's price of 0)
        ));

        // Verify bucket was created
        let bucket = Buckets::<Test>::get(0).unwrap();
        assert_eq!(bucket.min_providers, 1);
        assert_eq!(bucket.primary_providers.len(), 1);
        assert_eq!(bucket.primary_providers[0], 2);

        // Verify agreement was created
        let agreement = StorageAgreements::<Test>::get(0, 2).unwrap();
        assert_eq!(agreement.max_bytes, 100);
        assert_eq!(agreement.owner, 1);

        // Verify provider's committed_bytes was updated
        let provider = Providers::<Test>::get(2).unwrap();
        assert_eq!(provider.committed_bytes, 100);
    });
}

#[test]
fn create_bucket_with_storage_fails_no_matching_provider() {
    new_test_ext().execute_with(|| {
        // No providers registered
        assert_noop!(
            StorageProvider::create_bucket_with_storage(RuntimeOrigin::signed(1), 100, 100, 10),
            Error::<Test>::NoMatchingProvider
        );
    });
}

#[test]
fn create_bucket_with_storage_fails_provider_not_accepting() {
    new_test_ext().execute_with(|| {
        // Register a provider but don't set accepting_primary: true
        let multiaddr = b"/ip4/127.0.0.1/tcp/3000".to_vec();
        assert_ok!(StorageProvider::register_provider(
            RuntimeOrigin::signed(2),
            multiaddr.try_into().unwrap(),
            test_public_key(),
            200
        ));

        // Settings have accepting_primary: false by default (need to explicitly enable)
        // Since default is accepting_primary: true, let's set it to false
        let settings = ProviderSettings {
            min_duration: 10u64,
            max_duration: 1000u64,
            price_per_byte: 1u64,
            accepting_primary: false, // Not accepting
            replica_sync_price: None,
            accepting_extensions: true,
            max_capacity: 200,
        };
        assert_ok!(StorageProvider::update_provider_settings(
            RuntimeOrigin::signed(2),
            settings
        ));

        assert_noop!(
            StorageProvider::create_bucket_with_storage(RuntimeOrigin::signed(1), 100, 100, 10),
            Error::<Test>::NoMatchingProvider
        );
    });
}

#[test]
fn create_bucket_with_storage_fails_price_too_high() {
    new_test_ext().execute_with(|| {
        let multiaddr = b"/ip4/127.0.0.1/tcp/3000".to_vec();
        assert_ok!(StorageProvider::register_provider(
            RuntimeOrigin::signed(2),
            multiaddr.try_into().unwrap(),
            test_public_key(),
            200
        ));

        // Provider with high price
        let settings = ProviderSettings {
            min_duration: 10u64,
            max_duration: 1000u64,
            price_per_byte: 100u64, // Very high price
            accepting_primary: true,
            replica_sync_price: None,
            accepting_extensions: true,
            max_capacity: 200,
        };
        assert_ok!(StorageProvider::update_provider_settings(
            RuntimeOrigin::signed(2),
            settings
        ));

        // User's max_price_per_byte is lower than provider's price
        assert_noop!(
            StorageProvider::create_bucket_with_storage(
                RuntimeOrigin::signed(1),
                100,
                100,
                10 // max_price_per_byte is 10, but provider charges 100
            ),
            Error::<Test>::NoMatchingProvider
        );
    });
}

#[test]
fn create_bucket_with_storage_fails_insufficient_capacity() {
    new_test_ext().execute_with(|| {
        let multiaddr = b"/ip4/127.0.0.1/tcp/3000".to_vec();
        assert_ok!(StorageProvider::register_provider(
            RuntimeOrigin::signed(2),
            multiaddr.try_into().unwrap(),
            test_public_key(),
            200
        ));

        let settings = ProviderSettings {
            min_duration: 10u64,
            max_duration: 1000u64,
            price_per_byte: 1u64,
            accepting_primary: true,
            replica_sync_price: None,
            accepting_extensions: true,
            max_capacity: 50, // Only 50 bytes capacity
        };
        assert_ok!(StorageProvider::update_provider_settings(
            RuntimeOrigin::signed(2),
            settings
        ));

        // Request 100 bytes, but provider only has 50
        assert_noop!(
            StorageProvider::create_bucket_with_storage(
                RuntimeOrigin::signed(1),
                100, // Needs 100 bytes
                100,
                10
            ),
            Error::<Test>::NoMatchingProvider
        );
    });
}

#[test]
fn create_bucket_with_storage_fails_duration_mismatch() {
    new_test_ext().execute_with(|| {
        let multiaddr = b"/ip4/127.0.0.1/tcp/3000".to_vec();
        assert_ok!(StorageProvider::register_provider(
            RuntimeOrigin::signed(2),
            multiaddr.try_into().unwrap(),
            test_public_key(),
            200
        ));

        let settings = ProviderSettings {
            min_duration: 500u64, // Minimum 500 blocks
            max_duration: 1000u64,
            price_per_byte: 1u64,
            accepting_primary: true,
            replica_sync_price: None,
            accepting_extensions: true,
            max_capacity: 200,
        };
        assert_ok!(StorageProvider::update_provider_settings(
            RuntimeOrigin::signed(2),
            settings
        ));

        // Request only 100 blocks, but provider requires minimum 500
        assert_noop!(
            StorageProvider::create_bucket_with_storage(
                RuntimeOrigin::signed(1),
                100,
                100, // Duration of 100, below provider's min of 500
                10
            ),
            Error::<Test>::NoMatchingProvider
        );
    });
}

#[test]
fn create_bucket_with_storage_selects_cheapest_provider() {
    new_test_ext().execute_with(|| {
        // Register two providers with different prices
        let multiaddr = b"/ip4/127.0.0.1/tcp/3000".to_vec();

        // Provider 2: expensive (price = 5) - but still affordable
        assert_ok!(StorageProvider::register_provider(
            RuntimeOrigin::signed(2),
            multiaddr.clone().try_into().unwrap(),
            test_public_key(),
            200
        ));
        let settings_expensive = ProviderSettings {
            min_duration: 10u64,
            max_duration: 1000u64,
            price_per_byte: 5u64,
            accepting_primary: true,
            replica_sync_price: None,
            accepting_extensions: true,
            max_capacity: 200,
        };
        assert_ok!(StorageProvider::update_provider_settings(
            RuntimeOrigin::signed(2),
            settings_expensive
        ));

        // Provider 3: cheap (price = 0)
        assert_ok!(StorageProvider::register_provider(
            RuntimeOrigin::signed(3),
            multiaddr.try_into().unwrap(),
            test_public_key(),
            200
        ));
        let settings_cheap = ProviderSettings {
            min_duration: 10u64,
            max_duration: 1000u64,
            price_per_byte: 0u64, // Free
            accepting_primary: true,
            replica_sync_price: None,
            accepting_extensions: true,
            max_capacity: 200,
        };
        assert_ok!(StorageProvider::update_provider_settings(
            RuntimeOrigin::signed(3),
            settings_cheap
        ));

        // Create bucket - should match with cheaper provider (3)
        // Use small values to keep payment low: 10 * 10 * 5 = 500 max
        assert_ok!(StorageProvider::create_bucket_with_storage(
            RuntimeOrigin::signed(1),
            10, // max_bytes
            10, // duration
            10  // max_price_per_byte
        ));

        // Verify matched with provider 3 (the cheaper one)
        let bucket = Buckets::<Test>::get(0).unwrap();
        assert_eq!(bucket.primary_providers[0], 3);
    });
}
