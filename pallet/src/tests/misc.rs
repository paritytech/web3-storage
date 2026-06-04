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

        // Slash provider's stake to zero by directly manipulating storage
        Providers::<Test>::mutate(2, |maybe_provider| {
            if let Some(provider) = maybe_provider {
                // Unreserve existing stake and zero it out
                <Balances as frame_support::traits::ReservableCurrency<u64>>::unreserve(
                    &2,
                    provider.stake,
                );
                // Slash from free balance by reserving and slashing
                let _ =
                    <Balances as frame_support::traits::ReservableCurrency<u64>>::slash_reserved(
                        &2,
                        provider.stake,
                    );
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
        assert_ok!(StorageProvider::create_bucket(RuntimeOrigin::signed(1), 1));

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
        assert_ok!(StorageProvider::create_bucket(RuntimeOrigin::signed(1), 1));

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
        assert_ok!(StorageProvider::create_bucket(RuntimeOrigin::signed(1), 1));

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
