use super::*;
use storage_primitives::EndAction;

fn setup_active_agreement(provider: u64, client: u64, max_bytes: u64, duration: u64) -> u64 {
    register_provider(provider, 200);
    setup_agreement(provider, client, max_bytes, duration)
}

#[test]
fn end_agreement_pay_after_expiry() {
    new_test_ext().execute_with(|| {
        // Use non-zero price so there's real payment
        register_provider_with_settings(
            2,
            200,
            ProviderSettings {
                price_per_byte: 1,
                accepting_primary: true,
                ..Default::default()
            },
        );
        let bucket_id = setup_agreement(2, 1, 50, 100);

        let agreement = StorageAgreements::<Test>::get(bucket_id, 2).unwrap();
        let payment = agreement.payment_locked; // 1 * 50 * 100 = 5000
        assert_eq!(payment, 5000);

        let provider_balance_before = Balances::free_balance(2);

        // Wait past expiry but within settlement window
        run_to_block(agreement.expires_at + 1);

        assert_ok!(StorageProvider::end_agreement(
            RuntimeOrigin::signed(1),
            bucket_id,
            2,
            EndAction::Pay
        ));

        // Provider receives full payment
        assert_eq!(Balances::free_balance(2), provider_balance_before + payment);
        // Agreement removed
        assert!(StorageAgreements::<Test>::get(bucket_id, 2).is_none());
        // Provider committed_bytes decreased
        let provider = Providers::<Test>::get(2).unwrap();
        assert_eq!(provider.committed_bytes, 0);
    });
}

#[test]
fn end_agreement_burn_after_expiry() {
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
        let bucket_id = setup_agreement(2, 1, 50, 100);
        let agreement = StorageAgreements::<Test>::get(bucket_id, 2).unwrap();
        let payment = agreement.payment_locked; // 5000

        let provider_balance_before = Balances::free_balance(2);
        let treasury_balance_before = Balances::free_balance(999);

        run_to_block(agreement.expires_at + 1);

        assert_ok!(StorageProvider::end_agreement(
            RuntimeOrigin::signed(1),
            bucket_id,
            2,
            EndAction::Burn { burn_percent: 50 }
        ));

        // 50% burned → treasury, 50% to provider
        let burned = payment / 2;
        let to_provider = payment - burned;
        assert_eq!(
            Balances::free_balance(2),
            provider_balance_before + to_provider
        );
        assert_eq!(
            Balances::free_balance(999),
            treasury_balance_before + burned
        );
    });
}

#[test]
fn end_agreement_early_termination_by_admin() {
    new_test_ext().execute_with(|| {
        let bucket_id = setup_active_agreement(2, 1, 50, 100);

        // Early termination - admin (client 1) can end primary before expiry
        assert_ok!(StorageProvider::end_agreement(
            RuntimeOrigin::signed(1),
            bucket_id,
            2,
            EndAction::Pay
        ));

        assert!(StorageAgreements::<Test>::get(bucket_id, 2).is_none());
        let provider = Providers::<Test>::get(2).unwrap();
        assert_eq!(provider.committed_bytes, 0);
    });
}

#[test]
fn end_agreement_fails_not_admin_early() {
    new_test_ext().execute_with(|| {
        let bucket_id = setup_active_agreement(2, 1, 50, 100);

        // Non-admin tries early termination
        assert_noop!(
            StorageProvider::end_agreement(RuntimeOrigin::signed(3), bucket_id, 2, EndAction::Pay),
            Error::<Test>::NotBucketAdmin
        );
    });
}

#[test]
fn end_agreement_fails_past_settlement() {
    new_test_ext().execute_with(|| {
        let bucket_id = setup_active_agreement(2, 1, 50, 100);
        let agreement = StorageAgreements::<Test>::get(bucket_id, 2).unwrap();

        // Wait past settlement window (expiry + SettlementTimeout(50) + 1)
        run_to_block(agreement.expires_at + 51);

        assert_noop!(
            StorageProvider::end_agreement(RuntimeOrigin::signed(1), bucket_id, 2, EndAction::Pay),
            Error::<Test>::SettlementWindowPassed
        );
    });
}

#[test]
fn end_agreement_fails_not_found() {
    new_test_ext().execute_with(|| {
        register_provider(2, 200);
        create_bucket(1, 1);

        assert_noop!(
            StorageProvider::end_agreement(RuntimeOrigin::signed(1), 0, 2, EndAction::Pay),
            Error::<Test>::AgreementNotFound
        );
    });
}

#[test]
fn end_agreement_fails_not_owner_after_expiry() {
    new_test_ext().execute_with(|| {
        let bucket_id = setup_active_agreement(2, 1, 50, 100);
        let agreement = StorageAgreements::<Test>::get(bucket_id, 2).unwrap();

        run_to_block(agreement.expires_at + 1);

        // Non-owner tries to end after expiry
        assert_noop!(
            StorageProvider::end_agreement(RuntimeOrigin::signed(3), bucket_id, 2, EndAction::Pay),
            Error::<Test>::NotAgreementOwner
        );
    });
}

#[test]
fn claim_expired_agreement_works() {
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
        let bucket_id = setup_agreement(2, 1, 50, 100);
        let agreement = StorageAgreements::<Test>::get(bucket_id, 2).unwrap();
        let payment = agreement.payment_locked;

        let provider_balance_before = Balances::free_balance(2);

        // Wait past settlement deadline (expires_at + SettlementTimeout(50) + 1)
        run_to_block(agreement.expires_at + 51);

        assert_ok!(StorageProvider::claim_expired_agreement(
            RuntimeOrigin::signed(2),
            bucket_id
        ));

        assert_eq!(Balances::free_balance(2), provider_balance_before + payment);
        assert!(StorageAgreements::<Test>::get(bucket_id, 2).is_none());
    });
}

#[test]
fn claim_expired_agreement_fails_not_expired() {
    new_test_ext().execute_with(|| {
        let bucket_id = setup_active_agreement(2, 1, 50, 100);

        assert_noop!(
            StorageProvider::claim_expired_agreement(RuntimeOrigin::signed(2), bucket_id),
            Error::<Test>::AgreementNotExpired
        );
    });
}

#[test]
fn claim_expired_agreement_fails_within_settlement() {
    new_test_ext().execute_with(|| {
        let bucket_id = setup_active_agreement(2, 1, 50, 100);
        let agreement = StorageAgreements::<Test>::get(bucket_id, 2).unwrap();

        // Past expiry but within settlement window
        run_to_block(agreement.expires_at + 1);

        assert_noop!(
            StorageProvider::claim_expired_agreement(RuntimeOrigin::signed(2), bucket_id),
            Error::<Test>::AgreementNotExpired
        );
    });
}
