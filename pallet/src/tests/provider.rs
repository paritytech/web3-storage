use super::*;
use storage_primitives::EndAction;

#[test]
fn register_provider_works() {
    new_test_ext().execute_with(|| {
        let multiaddr = b"/ip4/127.0.0.1/tcp/3000".to_vec();

        assert_ok!(StorageProvider::register_provider(
            RuntimeOrigin::signed(1),
            multiaddr.clone().try_into().unwrap(),
            test_public_key(),
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
                test_public_key(),
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
            test_public_key(),
            200
        ));

        assert_noop!(
            StorageProvider::register_provider(
                RuntimeOrigin::signed(1),
                multiaddr.try_into().unwrap(),
                test_public_key(),
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
            test_public_key(),
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
fn deregister_provider_then_removes_provider_and_returns_stake() {
    new_test_ext().execute_with(|| {
        let multiaddr = b"/ip4/127.0.0.1/tcp/3000".to_vec();
        assert_ok!(StorageProvider::register_provider(
            RuntimeOrigin::signed(1),
            multiaddr.try_into().unwrap(),
            test_public_key(),
            200
        ));

        // Seed pending rewards across two buckets. Poke storage directly
        // because the reward-credit path requires a full checkpoint setup
        // orthogonal to this test.
        CheckpointRewards::<Test>::insert(1, 100u64, 30u64);
        CheckpointRewards::<Test>::insert(1, 200u64, 70u64);
        // Unrelated provider's reward — must survive.
        CheckpointRewards::<Test>::insert(2, 100u64, 999u64);

        let free_before = Balances::free_balance(1);

        assert_ok!(StorageProvider::deregister_provider(RuntimeOrigin::signed(
            1
        )));

        // Provider is gone immediately.
        assert!(Providers::<Test>::get(1).is_none());
        // Stake (200) + rewards (100) returned in one call.
        assert_eq!(Balances::free_balance(1), free_before + 300);
        // Provider's reward entries drained.
        assert_eq!(CheckpointRewards::<Test>::iter_prefix(1u64).count(), 0);
        // Unrelated provider's reward untouched.
        assert_eq!(CheckpointRewards::<Test>::get(2u64, 100u64), 999);
    });
}

#[test]
fn withdraw_agreement_request_works_after_provider_deregisters() {
    // A pending request created before the provider deregistered must
    // still be withdrawable — the owner's locked funds should not be
    // stranded just because the provider left.
    new_test_ext().execute_with(|| {
        let multiaddr = b"/ip4/127.0.0.1/tcp/3000".to_vec();
        assert_ok!(StorageProvider::register_provider(
            RuntimeOrigin::signed(2),
            multiaddr.try_into().unwrap(),
            test_public_key(),
            200
        ));
        assert_ok!(StorageProvider::create_bucket(RuntimeOrigin::signed(1), 1));

        // Owner creates request → payment locked.
        assert_ok!(StorageProvider::request_primary_agreement(
            RuntimeOrigin::signed(1),
            0,
            2,
            50,
            100,
            1000
        ));

        // Provider deregisters immediately (committed_bytes == 0 because
        // the request was never accepted).
        assert_ok!(StorageProvider::deregister_provider(RuntimeOrigin::signed(
            2
        )));
        assert!(Providers::<Test>::get(2).is_none());

        // Owner can still withdraw their pending request.
        assert_ok!(StorageProvider::withdraw_agreement_request(
            RuntimeOrigin::signed(1),
            0,
            2,
        ));
        assert!(AgreementRequests::<Test>::get(0, 2).is_none());
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
            test_public_key(),
            200
        ));
        assert_ok!(StorageProvider::create_bucket(RuntimeOrigin::signed(1), 1));

        // Create agreement (max_bytes = 100 fits within stake of 200)
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
            test_public_key(),
            200
        ));

        let new_settings = ProviderSettings {
            min_duration: 10u64,
            max_duration: 1000u64,
            price_per_byte: 5u64,
            accepting_primary: true,
            replica_sync_price: Some(10u64),
            accepting_extensions: true,
            max_capacity: 0, // Unlimited
        };

        assert_ok!(StorageProvider::update_provider_settings(
            RuntimeOrigin::signed(1),
            new_settings.clone()
        ));

        let provider = Providers::<Test>::get(1).unwrap();
        assert_eq!(provider.settings.price_per_byte, 5);
        assert_eq!(provider.settings.replica_sync_price, Some(10));
        assert_eq!(provider.settings.max_capacity, 0);
    });
}

#[test]
fn update_provider_settings_with_max_capacity_works() {
    new_test_ext().execute_with(|| {
        let multiaddr = b"/ip4/127.0.0.1/tcp/3000".to_vec();

        // Register with enough stake for 10000 bytes (stake >= bytes * MinStakePerByte)
        // MinStakePerByte = 1 in mock, so stake of 200 covers 200 bytes
        assert_ok!(StorageProvider::register_provider(
            RuntimeOrigin::signed(1),
            multiaddr.try_into().unwrap(),
            test_public_key(),
            200
        ));

        let new_settings = ProviderSettings {
            min_duration: 10u64,
            max_duration: 1000u64,
            price_per_byte: 5u64,
            accepting_primary: true,
            replica_sync_price: None,
            accepting_extensions: true,
            max_capacity: 200, // Up to 200 bytes (within stake limit)
        };

        assert_ok!(StorageProvider::update_provider_settings(
            RuntimeOrigin::signed(1),
            new_settings.clone()
        ));

        let provider = Providers::<Test>::get(1).unwrap();
        assert_eq!(provider.settings.max_capacity, 200);
    });
}

#[test]
fn update_provider_settings_fails_with_capacity_below_committed() {
    new_test_ext().execute_with(|| {
        // Setup provider and bucket
        let multiaddr = b"/ip4/127.0.0.1/tcp/3000".to_vec();
        assert_ok!(StorageProvider::register_provider(
            RuntimeOrigin::signed(2),
            multiaddr.try_into().unwrap(),
            test_public_key(),
            200
        ));
        assert_ok!(StorageProvider::create_bucket(RuntimeOrigin::signed(1), 1));

        // Create agreement for 100 bytes
        assert_ok!(StorageProvider::request_primary_agreement(
            RuntimeOrigin::signed(1),
            0,
            2,
            100, // max_bytes
            100,
            1000
        ));
        assert_ok!(StorageProvider::accept_agreement(
            RuntimeOrigin::signed(2),
            0
        ));

        // Try to set max_capacity below committed_bytes
        let new_settings = ProviderSettings {
            min_duration: 10u64,
            max_duration: 1000u64,
            price_per_byte: 5u64,
            accepting_primary: true,
            replica_sync_price: None,
            accepting_extensions: true,
            max_capacity: 50, // Below committed 100 bytes
        };

        assert_noop!(
            StorageProvider::update_provider_settings(RuntimeOrigin::signed(2), new_settings),
            Error::<Test>::CapacityBelowCommitted
        );
    });
}

#[test]
fn update_provider_settings_fails_with_insufficient_stake_for_capacity() {
    new_test_ext().execute_with(|| {
        let multiaddr = b"/ip4/127.0.0.1/tcp/3000".to_vec();

        // Register with stake of 200
        assert_ok!(StorageProvider::register_provider(
            RuntimeOrigin::signed(1),
            multiaddr.try_into().unwrap(),
            test_public_key(),
            200
        ));

        // Try to set capacity that requires more stake than available
        // MinStakePerByte = 1 in mock, so 200 stake only covers 200 bytes
        let new_settings = ProviderSettings {
            min_duration: 10u64,
            max_duration: 1000u64,
            price_per_byte: 5u64,
            accepting_primary: true,
            replica_sync_price: None,
            accepting_extensions: true,
            max_capacity: 1000, // Requires 1000 stake, but only have 200
        };

        assert_noop!(
            StorageProvider::update_provider_settings(RuntimeOrigin::signed(1), new_settings),
            Error::<Test>::InsufficientStakeForCapacity
        );
    });
}

#[test]
fn update_provider_settings_fails_when_min_duration_above_max() {
    new_test_ext().execute_with(|| {
        let multiaddr = b"/ip4/127.0.0.1/tcp/3000".to_vec();

        assert_ok!(StorageProvider::register_provider(
            RuntimeOrigin::signed(1),
            multiaddr.try_into().unwrap(),
            test_public_key(),
            200
        ));

        // min_duration > max_duration would silently brick the provider
        // out of `find_matching_provider`; reject it at the entry point.
        let bad_settings = ProviderSettings {
            min_duration: 1000u64,
            max_duration: 10u64,
            price_per_byte: 5u64,
            accepting_primary: true,
            replica_sync_price: None,
            accepting_extensions: true,
            max_capacity: 0,
        };

        assert_noop!(
            StorageProvider::update_provider_settings(RuntimeOrigin::signed(1), bad_settings),
            Error::<Test>::MinDurationExceedsMaxDuration
        );

        // Equal endpoints are allowed (single-duration providers).
        let edge_settings = ProviderSettings {
            min_duration: 100u64,
            max_duration: 100u64,
            price_per_byte: 5u64,
            accepting_primary: true,
            replica_sync_price: None,
            accepting_extensions: true,
            max_capacity: 0,
        };
        assert_ok!(StorageProvider::update_provider_settings(
            RuntimeOrigin::signed(1),
            edge_settings
        ));
    });
}

#[test]
fn update_provider_settings_emits_event_with_new_settings() {
    new_test_ext().execute_with(|| {
        // System events are only collected after block 0.
        frame_system::Pallet::<Test>::set_block_number(1);

        let multiaddr = b"/ip4/127.0.0.1/tcp/3000".to_vec();
        assert_ok!(StorageProvider::register_provider(
            RuntimeOrigin::signed(1),
            multiaddr.try_into().unwrap(),
            test_public_key(),
            200
        ));

        let new_settings = ProviderSettings {
            min_duration: 10u64,
            max_duration: 1000u64,
            price_per_byte: 5u64,
            accepting_primary: true,
            replica_sync_price: Some(10u64),
            accepting_extensions: true,
            max_capacity: 0,
        };

        assert_ok!(StorageProvider::update_provider_settings(
            RuntimeOrigin::signed(1),
            new_settings.clone()
        ));

        // Indexers should not need a follow-up storage read — the event
        // carries the full new settings payload.
        let expected = RuntimeEvent::StorageProvider(crate::Event::ProviderSettingsUpdated {
            provider: 1,
            settings: new_settings,
        });
        assert!(
            frame_system::Pallet::<Test>::events()
                .iter()
                .any(|r| r.event == expected),
            "ProviderSettingsUpdated event with full settings was not emitted"
        );
    });
}

#[test]
fn set_extensions_blocked_works_on_active_agreement() {
    new_test_ext().execute_with(|| {
        let multiaddr = b"/ip4/127.0.0.1/tcp/3000".to_vec();
        assert_ok!(StorageProvider::register_provider(
            RuntimeOrigin::signed(2),
            multiaddr.try_into().unwrap(),
            test_public_key(),
            200
        ));
        assert_ok!(StorageProvider::create_bucket(RuntimeOrigin::signed(1), 1));
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

        assert_ok!(StorageProvider::set_extensions_blocked(
            RuntimeOrigin::signed(2),
            0,
            true
        ));
        let agreement = StorageAgreements::<Test>::get(0, 2).unwrap();
        assert!(agreement.extensions_blocked);

        assert_ok!(StorageProvider::set_extensions_blocked(
            RuntimeOrigin::signed(2),
            0,
            false
        ));
        let agreement = StorageAgreements::<Test>::get(0, 2).unwrap();
        assert!(!agreement.extensions_blocked);
    });
}

#[test]
fn set_extensions_blocked_fails_after_agreement_expires() {
    new_test_ext().execute_with(|| {
        let multiaddr = b"/ip4/127.0.0.1/tcp/3000".to_vec();
        assert_ok!(StorageProvider::register_provider(
            RuntimeOrigin::signed(2),
            multiaddr.try_into().unwrap(),
            test_public_key(),
            200
        ));
        assert_ok!(StorageProvider::create_bucket(RuntimeOrigin::signed(1), 1));
        assert_ok!(StorageProvider::request_primary_agreement(
            RuntimeOrigin::signed(1),
            0,
            2,
            100,
            100, // duration = 100 → expires_at = current_block + 100
            1000
        ));
        assert_ok!(StorageProvider::accept_agreement(
            RuntimeOrigin::signed(2),
            0
        ));

        let agreement = StorageAgreements::<Test>::get(0, 2).unwrap();

        // At expires_at exactly, the agreement is no longer extendable
        // (strict `<` in the pallet guard).
        run_to_block(agreement.expires_at);
        assert_noop!(
            StorageProvider::set_extensions_blocked(RuntimeOrigin::signed(2), 0, true),
            Error::<Test>::AgreementExpired
        );

        // Past expiry, same rejection.
        run_to_block(agreement.expires_at + 1);
        assert_noop!(
            StorageProvider::set_extensions_blocked(RuntimeOrigin::signed(2), 0, true),
            Error::<Test>::AgreementExpired
        );
    });
}

#[test]
fn accept_agreement_fails_when_capacity_exceeded() {
    new_test_ext().execute_with(|| {
        // Setup provider with limited capacity
        let multiaddr = b"/ip4/127.0.0.1/tcp/3000".to_vec();
        assert_ok!(StorageProvider::register_provider(
            RuntimeOrigin::signed(2),
            multiaddr.try_into().unwrap(),
            test_public_key(),
            200
        ));

        // Set max_capacity to 50 bytes (stake of 200 can back this)
        let settings = ProviderSettings {
            min_duration: 0u64,
            max_duration: 1000u64,
            price_per_byte: 1u64,
            accepting_primary: true,
            replica_sync_price: None,
            accepting_extensions: true,
            max_capacity: 50,
        };
        assert_ok!(StorageProvider::update_provider_settings(
            RuntimeOrigin::signed(2),
            settings
        ));

        // Create bucket
        assert_ok!(StorageProvider::create_bucket(RuntimeOrigin::signed(1), 1));

        // Request agreement for 60 bytes (exceeds max_capacity of 50)
        // payment = price_per_byte * max_bytes * duration = 1 * 60 * 10 = 600
        assert_ok!(StorageProvider::request_primary_agreement(
            RuntimeOrigin::signed(1),
            0,
            2,
            60, // Exceeds max_capacity of 50
            10,
            600
        ));

        // Accept should fail due to capacity exceeded
        assert_noop!(
            StorageProvider::accept_agreement(RuntimeOrigin::signed(2), 0),
            Error::<Test>::CapacityExceeded
        );
    });
}

#[test]
fn accept_agreement_works_with_unlimited_capacity() {
    new_test_ext().execute_with(|| {
        // Setup provider with unlimited capacity (max_capacity = 0)
        let multiaddr = b"/ip4/127.0.0.1/tcp/3000".to_vec();
        assert_ok!(StorageProvider::register_provider(
            RuntimeOrigin::signed(2),
            multiaddr.try_into().unwrap(),
            test_public_key(),
            200
        ));

        // Settings with unlimited capacity (default)
        // Default max_capacity is 0 which means unlimited

        // Create bucket
        assert_ok!(StorageProvider::create_bucket(RuntimeOrigin::signed(1), 1));

        // Request agreement for 100 bytes
        // payment = 1 * 100 * 10 = 1000
        assert_ok!(StorageProvider::request_primary_agreement(
            RuntimeOrigin::signed(1),
            0,
            2,
            100,
            10,
            1000
        ));

        // Accept should succeed (capacity is unlimited, stake of 200 covers 100 bytes)
        assert_ok!(StorageProvider::accept_agreement(
            RuntimeOrigin::signed(2),
            0
        ));

        let provider = Providers::<Test>::get(2).unwrap();
        assert_eq!(provider.committed_bytes, 100);
    });
}

#[test]
fn accept_agreement_works_within_capacity() {
    new_test_ext().execute_with(|| {
        // Setup provider with limited capacity
        let multiaddr = b"/ip4/127.0.0.1/tcp/3000".to_vec();
        assert_ok!(StorageProvider::register_provider(
            RuntimeOrigin::signed(2),
            multiaddr.try_into().unwrap(),
            test_public_key(),
            200
        ));

        // Set max_capacity to 150 bytes (stake of 200 covers this)
        let settings = ProviderSettings {
            min_duration: 0u64,
            max_duration: 1000u64,
            price_per_byte: 1u64,
            accepting_primary: true,
            replica_sync_price: None,
            accepting_extensions: true,
            max_capacity: 150,
        };
        assert_ok!(StorageProvider::update_provider_settings(
            RuntimeOrigin::signed(2),
            settings
        ));

        // Create bucket
        assert_ok!(StorageProvider::create_bucket(RuntimeOrigin::signed(1), 1));

        // Request agreement for 100 bytes (within capacity)
        // payment = 1 * 100 * 10 = 1000
        assert_ok!(StorageProvider::request_primary_agreement(
            RuntimeOrigin::signed(1),
            0,
            2,
            100,
            10,
            1000
        ));

        // Accept should succeed
        assert_ok!(StorageProvider::accept_agreement(
            RuntimeOrigin::signed(2),
            0
        ));

        let provider = Providers::<Test>::get(2).unwrap();
        assert_eq!(provider.committed_bytes, 100);
        assert_eq!(provider.settings.max_capacity, 150);
    });
}

#[test]
fn deregister_provider_full_flow() {
    // Full happy path: provider registers, user creates bucket + establishes
    // an agreement, user early-terminates it, then provider deregisters.
    new_test_ext().execute_with(|| {
        let multiaddr = b"/ip4/127.0.0.1/tcp/3000".to_vec();
        assert_ok!(StorageProvider::register_provider(
            RuntimeOrigin::signed(2),
            multiaddr.try_into().unwrap(),
            test_public_key(),
            200
        ));

        assert_ok!(StorageProvider::create_bucket(RuntimeOrigin::signed(1), 1));

        // default price_per_byte == 0, so payment == 0; max_payment 1000 is just buffer.
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
        assert_eq!(Providers::<Test>::get(2).unwrap().committed_bytes, 100);

        // Acct 1 is bucket admin; current_block < expires_at so this is early termination.
        assert_ok!(StorageProvider::end_agreement(
            RuntimeOrigin::signed(1),
            0,
            2,
            EndAction::Pay
        ));
        assert_eq!(Providers::<Test>::get(2).unwrap().committed_bytes, 0);
        assert!(StorageAgreements::<Test>::get(0, 2).is_none());

        let reserved_before = Balances::reserved_balance(2);
        assert_ok!(StorageProvider::deregister_provider(RuntimeOrigin::signed(
            2
        )));
        assert!(Providers::<Test>::get(2).is_none());
        assert_eq!(Balances::reserved_balance(2), 0);
        assert!(reserved_before > 0); // confirms stake was actually released
    });
}
