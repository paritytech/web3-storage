// SPDX-License-Identifier: Apache-2.0

use super::*;

#[test]
fn extend_agreement_works() {
    new_test_ext().execute_with(|| {
        register_provider(2, 200);
        let bucket_id = setup_agreement(2, 1, 50, 100);
        let agreement_before = StorageAgreements::<Test>::get(bucket_id, 2).unwrap();

        // Extend by 50 blocks
        assert_ok!(StorageProvider::extend_agreement(
            RuntimeOrigin::signed(1),
            bucket_id,
            2,
            50,
            10000 // generous max_payment
        ));

        let agreement_after = StorageAgreements::<Test>::get(bucket_id, 2).unwrap();
        // New expires_at = current_block + additional_duration
        // Current block is 0, so new expires_at = 0 + 50 = 50
        assert!(agreement_after.expires_at > agreement_before.started_at);
    });
}

#[test]
fn extend_agreement_fails_extensions_blocked() {
    new_test_ext().execute_with(|| {
        register_provider(2, 200);
        let bucket_id = setup_agreement(2, 1, 50, 100);

        // Provider blocks extensions on this agreement
        assert_ok!(StorageProvider::set_extensions_blocked(
            RuntimeOrigin::signed(2),
            bucket_id,
            true
        ));

        assert_noop!(
            StorageProvider::extend_agreement(RuntimeOrigin::signed(1), bucket_id, 2, 50, 10000),
            Error::<Test>::AgreementExtensionsBlocked
        );
    });
}

#[test]
fn extend_agreement_fails_provider_not_accepting_extensions() {
    new_test_ext().execute_with(|| {
        register_provider(2, 200);
        let bucket_id = setup_agreement(2, 1, 50, 100);

        // Provider disables extensions globally
        assert_ok!(StorageProvider::update_provider_settings(
            RuntimeOrigin::signed(2),
            ProviderSettings {
                accepting_extensions: false,
                accepting_primary: true,
                ..Default::default()
            }
        ));

        assert_noop!(
            StorageProvider::extend_agreement(RuntimeOrigin::signed(1), bucket_id, 2, 50, 10000),
            Error::<Test>::ProviderNotAcceptingExtensions
        );
    });
}

#[test]
fn extend_agreement_fails_payment_too_low() {
    new_test_ext().execute_with(|| {
        register_provider_with_settings(
            2,
            200,
            ProviderSettings {
                price_per_byte: 1,
                accepting_primary: true,
                accepting_extensions: true,
                ..Default::default()
            },
        );
        let bucket_id = setup_agreement(2, 1, 50, 100);

        // Extension payment = 1 * 50 * 50 = 2500, but max_payment = 1
        assert_noop!(
            StorageProvider::extend_agreement(
                RuntimeOrigin::signed(1),
                bucket_id,
                2,
                50,
                1 // way too low
            ),
            Error::<Test>::PaymentExceedsMax
        );
    });
}

#[test]
fn extend_agreement_fails_duration_too_short() {
    new_test_ext().execute_with(|| {
        register_provider_with_settings(
            2,
            200,
            ProviderSettings {
                min_duration: 50,
                max_duration: 1000,
                accepting_primary: true,
                accepting_extensions: true,
                ..Default::default()
            },
        );
        let bucket_id = setup_agreement(2, 1, 50, 100);

        assert_noop!(
            StorageProvider::extend_agreement(
                RuntimeOrigin::signed(1),
                bucket_id,
                2,
                10, // below min_duration of 50
                10000
            ),
            Error::<Test>::DurationTooShort
        );
    });
}

#[test]
fn extend_agreement_fails_no_agreement() {
    new_test_ext().execute_with(|| {
        register_provider(2, 200);
        create_bucket(1, 1);

        assert_noop!(
            StorageProvider::extend_agreement(RuntimeOrigin::signed(1), 0, 2, 50, 10000),
            Error::<Test>::AgreementNotFound
        );
    });
}

#[test]
fn top_up_agreement_works() {
    new_test_ext().execute_with(|| {
        register_provider(2, 200);
        let bucket_id = setup_agreement(2, 1, 50, 100);

        let agreement_before = StorageAgreements::<Test>::get(bucket_id, 2).unwrap();

        assert_ok!(StorageProvider::top_up_agreement(
            RuntimeOrigin::signed(1),
            bucket_id,
            2,
            20,    // additional bytes
            10000  // max_payment
        ));

        let agreement_after = StorageAgreements::<Test>::get(bucket_id, 2).unwrap();
        assert_eq!(agreement_after.max_bytes, agreement_before.max_bytes + 20);
        // Provider committed_bytes should increase
        let provider = Providers::<Test>::get(2).unwrap();
        assert_eq!(provider.committed_bytes, 50 + 20);
    });
}

#[test]
fn top_up_agreement_fails_expired() {
    new_test_ext().execute_with(|| {
        register_provider(2, 200);
        let bucket_id = setup_agreement(2, 1, 50, 100);
        let agreement = StorageAgreements::<Test>::get(bucket_id, 2).unwrap();

        run_to_block(agreement.expires_at);

        assert_noop!(
            StorageProvider::top_up_agreement(RuntimeOrigin::signed(1), bucket_id, 2, 20, 10000),
            Error::<Test>::AgreementExpired
        );
    });
}

#[test]
fn top_up_agreement_fails_not_owner() {
    new_test_ext().execute_with(|| {
        register_provider(2, 200);
        let bucket_id = setup_agreement(2, 1, 50, 100);

        assert_noop!(
            StorageProvider::top_up_agreement(RuntimeOrigin::signed(3), bucket_id, 2, 20, 10000),
            Error::<Test>::NotAgreementOwner
        );
    });
}

#[test]
fn top_up_agreement_fails_payment_exceeds_max() {
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

        // additional payment = price_per_byte(1) * additional_bytes(20) * remaining
        // remaining ~ 100, so payment ~ 2000
        assert_noop!(
            StorageProvider::top_up_agreement(
                RuntimeOrigin::signed(1),
                bucket_id,
                2,
                20,
                1 // way too low
            ),
            Error::<Test>::PaymentExceedsMax
        );
    });
}
