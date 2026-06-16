// SPDX-License-Identifier: Apache-2.0

use super::*;

#[test]
fn update_provider_multiaddr_works() {
    new_test_ext().execute_with(|| {
        frame_system::Pallet::<Test>::set_block_number(1);
        register_provider(1, 200);

        let new_multiaddr = b"/ip4/192.168.0.1/tcp/4000".to_vec();
        assert_ok!(StorageProvider::update_provider_multiaddr(
            RuntimeOrigin::signed(1),
            new_multiaddr.clone().try_into().unwrap(),
        ));

        let provider = Providers::<Test>::get(1).unwrap();
        assert_eq!(provider.multiaddr.to_vec(), new_multiaddr);

        // Verify event emitted
        let expected =
            RuntimeEvent::StorageProvider(crate::Event::ProviderMultiaddrUpdated { provider: 1 });
        assert!(frame_system::Pallet::<Test>::events()
            .iter()
            .any(|r| r.event == expected));
    });
}

#[test]
fn update_provider_multiaddr_fails_not_registered() {
    new_test_ext().execute_with(|| {
        let new_multiaddr = b"/ip4/192.168.0.1/tcp/4000".to_vec();
        assert_noop!(
            StorageProvider::update_provider_multiaddr(
                RuntimeOrigin::signed(1),
                new_multiaddr.try_into().unwrap(),
            ),
            Error::<Test>::ProviderNotFound
        );
    });
}

#[test]
fn remove_slashed_works() {
    new_test_ext().execute_with(|| {
        register_provider(2, 200);
        let bucket_id = setup_agreement(2, 1, 50, 200);

        // Slash provider's entire reserved stake (mirrors production slash_provider_for_failed_challenge)
        Providers::<Test>::mutate(2, |maybe_provider| {
            if let Some(provider) = maybe_provider {
                let stake = provider.stake;
                let (_, remaining) =
                    <Balances as frame_support::traits::ReservableCurrency<u64>>::slash_reserved(
                        &2, stake,
                    );
                assert_eq!(remaining, 0, "entire stake should have been slashed");
                provider.stake = 0;
            }
        });

        let owner_balance_before = Balances::free_balance(1);
        let agreement = StorageAgreements::<Test>::get(bucket_id, 2).unwrap();
        let payment_locked = agreement.payment_locked;

        // Anyone can call remove_slashed
        assert_ok!(StorageProvider::remove_slashed(
            RuntimeOrigin::signed(3),
            bucket_id,
            2
        ));

        // Agreement removed
        assert!(StorageAgreements::<Test>::get(bucket_id, 2).is_none());
        // Payment returned to owner
        assert_eq!(
            Balances::free_balance(1),
            owner_balance_before + payment_locked
        );
        // Provider removed from bucket
        let bucket = Buckets::<Test>::get(bucket_id).unwrap();
        assert!(!bucket.primary_providers.contains(&2));
    });
}

#[test]
fn remove_slashed_fails_not_slashed() {
    new_test_ext().execute_with(|| {
        register_provider(2, 200);
        let bucket_id = setup_agreement(2, 1, 50, 200);

        // Provider has stake > 0
        assert_noop!(
            StorageProvider::remove_slashed(RuntimeOrigin::signed(3), bucket_id, 2),
            Error::<Test>::ProviderNotSlashed
        );
    });
}

#[test]
fn remove_slashed_fails_no_agreement() {
    new_test_ext().execute_with(|| {
        register_provider(2, 200);
        create_bucket(1, 1);

        // Zero out stake
        Providers::<Test>::mutate(2, |maybe_provider| {
            if let Some(provider) = maybe_provider {
                <Balances as frame_support::traits::ReservableCurrency<u64>>::unreserve(
                    &2,
                    provider.stake,
                );
                provider.stake = 0;
            }
        });

        assert_noop!(
            StorageProvider::remove_slashed(RuntimeOrigin::signed(3), 0, 2),
            Error::<Test>::AgreementNotFound
        );
    });
}

#[test]
fn remove_slashed_fails_provider_not_found() {
    new_test_ext().execute_with(|| {
        create_bucket(1, 1);

        assert_noop!(
            StorageProvider::remove_slashed(RuntimeOrigin::signed(3), 0, 99),
            Error::<Test>::ProviderNotFound
        );
    });
}

#[test]
fn set_extensions_blocked_fails_no_agreement() {
    new_test_ext().execute_with(|| {
        register_provider(2, 200);
        create_bucket(1, 1);

        assert_noop!(
            StorageProvider::set_extensions_blocked(RuntimeOrigin::signed(2), 0, true),
            Error::<Test>::AgreementNotFound
        );
    });
}

#[test]
fn set_extensions_blocked_emits_event() {
    new_test_ext().execute_with(|| {
        frame_system::Pallet::<Test>::set_block_number(1);
        register_provider(2, 200);
        let bucket_id = setup_agreement(2, 1, 50, 100);

        assert_ok!(StorageProvider::set_extensions_blocked(
            RuntimeOrigin::signed(2),
            bucket_id,
            true
        ));

        let expected = RuntimeEvent::StorageProvider(crate::Event::ExtensionsBlocked {
            bucket_id,
            provider: 2,
            blocked: true,
        });
        assert!(frame_system::Pallet::<Test>::events()
            .iter()
            .any(|r| r.event == expected));
    });
}

#[test]
fn register_provider_emits_event() {
    new_test_ext().execute_with(|| {
        frame_system::Pallet::<Test>::set_block_number(1);
        register_provider(2, 200);

        let expected = RuntimeEvent::StorageProvider(crate::Event::ProviderRegistered {
            provider: 2,
            stake: 200,
        });
        assert!(frame_system::Pallet::<Test>::events()
            .iter()
            .any(|r| r.event == expected));
    });
}

#[test]
fn create_bucket_emits_event() {
    new_test_ext().execute_with(|| {
        frame_system::Pallet::<Test>::set_block_number(1);
        let bucket_id = create_bucket(1, 1);

        let expected = RuntimeEvent::StorageProvider(crate::Event::BucketCreated {
            bucket_id,
            admin: 1,
        });
        assert!(frame_system::Pallet::<Test>::events()
            .iter()
            .any(|r| r.event == expected));
    });
}

#[test]
fn establish_agreement_emits_event() {
    new_test_ext().execute_with(|| {
        frame_system::Pallet::<Test>::set_block_number(1);
        register_provider(2, 200);
        let bucket_id = setup_agreement(2, 1, 50, 100);

        // `setup_agreement` redeems provider-signed terms via
        // `establish_storage_agreement`, which emits StorageAgreementEstablished.
        let events = frame_system::Pallet::<Test>::events();
        let found = events.iter().any(|r| {
            matches!(
                &r.event,
                RuntimeEvent::StorageProvider(crate::Event::StorageAgreementEstablished {
                    bucket_id: bid,
                    provider: 2,
                    owner: 1,
                    expires_at: 101, // block 1 + duration 100
                    ..
                }) if *bid == bucket_id
            )
        });
        assert!(found);
    });
}

#[test]
fn challenge_checkpoint_emits_event() {
    new_test_ext().execute_with(|| {
        frame_system::Pallet::<Test>::set_block_number(1);
        register_provider(2, 200);
        let bucket_id = setup_agreement(2, 1, 50, 200);

        // Insert snapshot
        Buckets::<Test>::mutate(bucket_id, |maybe_bucket| {
            if let Some(bucket) = maybe_bucket {
                bucket.snapshot = Some(storage_primitives::BucketSnapshot {
                    mmr_root: sp_core::H256::repeat_byte(0xAB),
                    start_seq: 0,
                    leaf_count: 10,
                    checkpoint_block: 1,
                    primary_signers: vec![0x01],
                });
            }
        });

        assert_ok!(StorageProvider::challenge_checkpoint(
            RuntimeOrigin::signed(3),
            bucket_id,
            2,
            0,
            0,
        ));

        let expected = RuntimeEvent::StorageProvider(crate::Event::ChallengeCreated {
            challenge_id: storage_primitives::ChallengeId {
                deadline: 101,
                index: 0,
            },
            bucket_id,
            provider: 2,
            challenger: 3,
            respond_by: 101,
        });
        assert!(frame_system::Pallet::<Test>::events()
            .iter()
            .any(|r| r.event == expected));
    });
}

#[test]
fn checkpoint_emits_event() {
    new_test_ext().execute_with(|| {
        frame_system::Pallet::<Test>::set_block_number(1);
        let bucket_id = create_bucket(1, 0);

        assert_ok!(StorageProvider::checkpoint(
            RuntimeOrigin::signed(1),
            bucket_id,
            sp_core::H256::repeat_byte(0xAA),
            0,
            10,
            Default::default(),
        ));

        let expected = RuntimeEvent::StorageProvider(crate::Event::BucketCheckpointed {
            bucket_id,
            mmr_root: sp_core::H256::repeat_byte(0xAA),
            start_seq: 0,
            leaf_count: 10,
            providers: vec![],
        });
        assert!(frame_system::Pallet::<Test>::events()
            .iter()
            .any(|r| r.event == expected));
    });
}

#[test]
fn end_agreement_emits_event() {
    new_test_ext().execute_with(|| {
        frame_system::Pallet::<Test>::set_block_number(1);
        register_provider(2, 200);
        let bucket_id = setup_agreement(2, 1, 50, 100);

        // Advance past agreement expiry (expires_at = 100)
        run_to_block(101);

        // End agreement after expiry (owner can end within settlement window)
        assert_ok!(StorageProvider::end_agreement(
            RuntimeOrigin::signed(1),
            bucket_id,
            2,
            storage_primitives::EndAction::Pay,
        ));

        // Verify AgreementEnded event emitted
        let events = frame_system::Pallet::<Test>::events();
        let found = events.iter().any(|r| {
            matches!(
                &r.event,
                RuntimeEvent::StorageProvider(crate::Event::AgreementEnded {
                    bucket_id: bid,
                    provider: 2,
                    ..
                }) if *bid == bucket_id
            )
        });
        assert!(found);
    });
}
