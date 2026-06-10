use super::*;

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
fn deregister_provider_full_flow_announce_then_complete() {
    new_test_ext().execute_with(|| {
        let multiaddr = b"/ip4/127.0.0.1/tcp/3000".to_vec();

        assert_ok!(StorageProvider::register_provider(
            RuntimeOrigin::signed(1),
            multiaddr.try_into().unwrap(),
            test_public_key(),
            200
        ));

        let balance_before = Balances::free_balance(1);

        // Announce step: provider record stays, stake stays reserved,
        // acceptance flags are forced false, deregister_at is stamped.
        assert_ok!(StorageProvider::deregister_provider(RuntimeOrigin::signed(
            1
        )));
        let provider = Providers::<Test>::get(1).unwrap();
        assert_eq!(
            provider.deregister_at,
            Some(System::block_number() + 100) // DeregisterAnnouncementPeriod in mock
        );
        assert!(!provider.settings.accepting_primary);
        assert!(!provider.settings.accepting_extensions);
        assert_eq!(Balances::free_balance(1), balance_before); // not yet refunded

        // Premature completion is rejected.
        assert_noop!(
            StorageProvider::complete_deregister(RuntimeOrigin::signed(1)),
            Error::<Test>::DeregisterPeriodNotElapsed
        );

        // After the period, complete succeeds and stake comes back.
        let deregister_at = provider.deregister_at.unwrap();
        run_to_block(deregister_at);
        assert_ok!(StorageProvider::complete_deregister(RuntimeOrigin::signed(
            1
        )));
        assert!(Providers::<Test>::get(1).is_none());
        assert_eq!(Balances::free_balance(1), balance_before + 200);
    });
}

#[test]
fn deregister_provider_announcement_is_one_shot() {
    new_test_ext().execute_with(|| {
        let multiaddr = b"/ip4/127.0.0.1/tcp/3000".to_vec();
        assert_ok!(StorageProvider::register_provider(
            RuntimeOrigin::signed(1),
            multiaddr.try_into().unwrap(),
            test_public_key(),
            200
        ));
        assert_ok!(StorageProvider::deregister_provider(RuntimeOrigin::signed(
            1
        )));
        assert_noop!(
            StorageProvider::deregister_provider(RuntimeOrigin::signed(1)),
            Error::<Test>::DeregisterAnnounced
        );
    });
}

#[test]
fn cancel_deregister_clears_announcement() {
    new_test_ext().execute_with(|| {
        let multiaddr = b"/ip4/127.0.0.1/tcp/3000".to_vec();
        assert_ok!(StorageProvider::register_provider(
            RuntimeOrigin::signed(1),
            multiaddr.try_into().unwrap(),
            test_public_key(),
            200
        ));
        assert_ok!(StorageProvider::deregister_provider(RuntimeOrigin::signed(
            1
        )));
        assert!(Providers::<Test>::get(1).unwrap().deregister_at.is_some());

        assert_ok!(StorageProvider::cancel_deregister(RuntimeOrigin::signed(1)));
        let restored = Providers::<Test>::get(1).unwrap();
        assert!(restored.deregister_at.is_none());
        // Cancel mirrors announce: flags that announce forced to false
        // are restored to true.
        assert!(restored.settings.accepting_primary);
        assert!(restored.settings.accepting_extensions);

        // And settings updates work again post-cancel.
        let tweak = ProviderSettings {
            min_duration: 10u64,
            max_duration: 1000u64,
            price_per_byte: 5u64,
            accepting_primary: true,
            replica_sync_price: None,
            accepting_extensions: true,
            max_capacity: 0,
        };
        assert_ok!(StorageProvider::update_provider_settings(
            RuntimeOrigin::signed(1),
            tweak
        ));
    });
}

#[test]
fn cancel_deregister_fails_without_announcement() {
    new_test_ext().execute_with(|| {
        let multiaddr = b"/ip4/127.0.0.1/tcp/3000".to_vec();
        assert_ok!(StorageProvider::register_provider(
            RuntimeOrigin::signed(1),
            multiaddr.try_into().unwrap(),
            test_public_key(),
            200
        ));
        assert_noop!(
            StorageProvider::cancel_deregister(RuntimeOrigin::signed(1)),
            Error::<Test>::DeregisterNotAnnounced
        );
    });
}

#[test]
fn complete_deregister_fails_without_announcement() {
    new_test_ext().execute_with(|| {
        let multiaddr = b"/ip4/127.0.0.1/tcp/3000".to_vec();
        assert_ok!(StorageProvider::register_provider(
            RuntimeOrigin::signed(1),
            multiaddr.try_into().unwrap(),
            test_public_key(),
            200
        ));
        assert_noop!(
            StorageProvider::complete_deregister(RuntimeOrigin::signed(1)),
            Error::<Test>::DeregisterNotAnnounced
        );
    });
}

#[test]
fn update_provider_settings_blocked_while_announcement_pending() {
    new_test_ext().execute_with(|| {
        let multiaddr = b"/ip4/127.0.0.1/tcp/3000".to_vec();
        assert_ok!(StorageProvider::register_provider(
            RuntimeOrigin::signed(1),
            multiaddr.try_into().unwrap(),
            test_public_key(),
            200
        ));
        assert_ok!(StorageProvider::deregister_provider(RuntimeOrigin::signed(
            1
        )));

        let resumed = ProviderSettings {
            min_duration: 10u64,
            max_duration: 1000u64,
            price_per_byte: 5u64,
            accepting_primary: true, // attempts to un-freeze
            replica_sync_price: None,
            accepting_extensions: true,
            max_capacity: 0,
        };
        assert_noop!(
            StorageProvider::update_provider_settings(RuntimeOrigin::signed(1), resumed),
            Error::<Test>::DeregisterAnnounced
        );
    });
}

#[test]
fn agreement_entry_points_reject_deregistering_provider() {
    new_test_ext().execute_with(|| {
        register_provider(2, 200);

        // Set up an existing agreement — this happens BEFORE announce.
        let bucket_id = setup_agreement(2, 1, 50, 100);

        // End the agreement so committed_bytes drops back to 0 and
        // announce is allowed. Wait past expires_at + SettlementTimeout
        // so claim_expired_agreement succeeds.
        run_to_block(200);
        assert_ok!(StorageProvider::claim_expired_agreement(
            RuntimeOrigin::signed(2),
            bucket_id
        ));

        // Now announce.
        assert_ok!(StorageProvider::deregister_provider(RuntimeOrigin::signed(
            2
        )));

        // Every agreement-creating entry point now rejects with
        // DeregisterAnnounced. The check runs after the nonce window
        // advances, so assert the error only.
        let (terms, sig) = signed_primary_terms(2, 1, 50, 100);
        assert_err!(
            StorageProvider::establish_storage_agreement(RuntimeOrigin::signed(1), 2, terms, sig),
            Error::<Test>::DeregisterAnnounced
        );

        let (terms, sig) = signed_replica_terms(
            2,
            1,
            bucket_id,
            50,
            100,
            storage_primitives::ReplicaTerms {
                sync_balance: 0,
                min_sync_interval: 10,
                sync_price: 10,
            },
        );
        assert_err!(
            StorageProvider::establish_replica_agreement(
                RuntimeOrigin::signed(1),
                bucket_id,
                2,
                terms,
                sig
            ),
            Error::<Test>::DeregisterAnnounced
        );
    });
}

#[test]
fn complete_deregister_drains_checkpoint_rewards() {
    new_test_ext().execute_with(|| {
        let multiaddr = b"/ip4/127.0.0.1/tcp/3000".to_vec();
        assert_ok!(StorageProvider::register_provider(
            RuntimeOrigin::signed(1),
            multiaddr.try_into().unwrap(),
            test_public_key(),
            200
        ));

        // Seed pending rewards across two buckets for this provider. We
        // poke storage directly because the on-chain reward-credit path
        // requires a full checkpoint setup that's orthogonal to this
        // test.
        CheckpointRewards::<Test>::insert(1, 100u64, 30u64);
        CheckpointRewards::<Test>::insert(1, 200u64, 70u64);
        // Unrelated provider's reward in another bucket — must survive.
        CheckpointRewards::<Test>::insert(2, 100u64, 999u64);

        let free_before = Balances::free_balance(1);

        assert_ok!(StorageProvider::deregister_provider(RuntimeOrigin::signed(
            1
        )));
        let deregister_at = Providers::<Test>::get(1).unwrap().deregister_at.unwrap();
        run_to_block(deregister_at);
        assert_ok!(StorageProvider::complete_deregister(RuntimeOrigin::signed(
            1
        )));

        // 200 (stake) + 30 + 70 (drained rewards) = 300 added to free balance.
        assert_eq!(Balances::free_balance(1), free_before + 300);
        // Provider's reward entries are gone.
        assert_eq!(CheckpointRewards::<Test>::iter_prefix(1u64).count(), 0);
        // Unrelated provider's reward is untouched.
        assert_eq!(CheckpointRewards::<Test>::get(2u64, 100u64), 999);
    });
}

#[test]
fn deregister_provider_fails_with_active_agreements() {
    new_test_ext().execute_with(|| {
        register_provider(2, 200);

        // Create agreement (max_bytes = 100 fits within stake of 200)
        setup_agreement(2, 1, 100, 100);

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
        register_provider(2, 200);

        // Create agreement for 100 bytes
        setup_agreement(2, 1, 100, 100);

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
        register_provider(2, 200);
        setup_agreement(2, 1, 100, 100);

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
        register_provider(2, 200);
        // duration = 100 → expires_at = current_block + 100
        setup_agreement(2, 1, 100, 100);

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
fn establish_agreement_fails_when_capacity_exceeded() {
    new_test_ext().execute_with(|| {
        // Setup provider with max_capacity of 50 bytes (stake of 200 can
        // back this)
        register_provider_with_settings(
            2,
            200,
            ProviderSettings {
                min_duration: 0u64,
                max_duration: 1000u64,
                price_per_byte: 1u64,
                accepting_primary: true,
                replica_sync_price: None,
                accepting_extensions: true,
                max_capacity: 50,
            },
        );

        // Terms for 60 bytes exceed max_capacity of 50. The capacity
        // check runs after the nonce window advances, so assert the
        // error only.
        let (terms, sig) = signed_primary_terms(2, 1, 60, 10);
        assert_err!(
            StorageProvider::establish_storage_agreement(RuntimeOrigin::signed(1), 2, terms, sig),
            Error::<Test>::CapacityExceeded
        );
    });
}

#[test]
fn establish_agreement_works_with_unlimited_capacity() {
    new_test_ext().execute_with(|| {
        // Default settings have max_capacity = 0 which means unlimited
        register_provider(2, 200);

        // Agreement for 100 bytes succeeds (capacity is unlimited, stake
        // of 200 covers 100 bytes)
        setup_agreement(2, 1, 100, 10);

        let provider = Providers::<Test>::get(2).unwrap();
        assert_eq!(provider.committed_bytes, 100);
    });
}

#[test]
fn establish_agreement_works_within_capacity() {
    new_test_ext().execute_with(|| {
        // Set max_capacity to 150 bytes (stake of 200 covers this)
        register_provider_with_settings(
            2,
            200,
            ProviderSettings {
                min_duration: 0u64,
                max_duration: 1000u64,
                price_per_byte: 1u64,
                accepting_primary: true,
                replica_sync_price: None,
                accepting_extensions: true,
                max_capacity: 150,
            },
        );

        // Agreement for 100 bytes (within capacity) succeeds
        setup_agreement(2, 1, 100, 10);

        let provider = Providers::<Test>::get(2).unwrap();
        assert_eq!(provider.committed_bytes, 100);
        assert_eq!(provider.settings.max_capacity, 150);
    });
}
