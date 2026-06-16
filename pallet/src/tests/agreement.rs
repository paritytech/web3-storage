use super::*;
use sp_core::Pair as _;

#[test]
fn establish_storage_agreement_works() {
    new_test_ext().execute_with(|| {
        register_provider(2, 200);

        let (terms, sig) = signed_primary_terms(2, 1, 100, 100);
        assert_ok!(StorageProvider::establish_storage_agreement(
            RuntimeOrigin::signed(1),
            2,
            terms,
            sig
        ));

        // Bucket created with owner as sole admin and provider as the
        // single primary.
        let bucket = Buckets::<Test>::get(0).unwrap();
        assert_eq!(bucket.members.len(), 1);
        assert_eq!(bucket.members[0].account, 1);
        assert_eq!(bucket.members[0].role, Role::Admin);
        assert_eq!(bucket.primary_providers.to_vec(), vec![2]);

        // Agreement opened at the signed terms.
        let agreement = StorageAgreements::<Test>::get(0, 2).unwrap();
        assert_eq!(agreement.owner, 1);
        assert_eq!(agreement.max_bytes, 100);
        assert!(matches!(agreement.role, ProviderRole::Primary));

        // Provider stats updated.
        let provider = Providers::<Test>::get(2).unwrap();
        assert_eq!(provider.committed_bytes, 100);
        assert_eq!(provider.stats.agreements_total, 1);
    });
}

#[test]
fn establish_storage_agreement_reserves_payment() {
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

        let balance_before = Balances::free_balance(1);

        // payment = price_per_byte(1) * max_bytes(100) * duration(10) = 1000
        let (terms, sig) = signed_primary_terms(2, 1, 100, 10);
        assert_ok!(StorageProvider::establish_storage_agreement(
            RuntimeOrigin::signed(1),
            2,
            terms,
            sig
        ));

        assert_eq!(Balances::free_balance(1), balance_before - 1000);
        let agreement = StorageAgreements::<Test>::get(0, 2).unwrap();
        assert_eq!(agreement.payment_locked, 1000);
    });
}

#[test]
fn establish_storage_agreement_fails_for_wrong_owner() {
    new_test_ext().execute_with(|| {
        register_provider(2, 200);

        // Terms signed for owner 1, redeemed by account 3.
        let (terms, sig) = signed_primary_terms(2, 1, 100, 100);
        assert_noop!(
            StorageProvider::establish_storage_agreement(RuntimeOrigin::signed(3), 2, terms, sig),
            Error::<Test>::TermsOwnerMismatch
        );
    });
}

#[test]
fn establish_storage_agreement_fails_with_bucket_bound_terms() {
    new_test_ext().execute_with(|| {
        register_provider(2, 200);

        // Primary terms must not be bound to a bucket.
        let pair = provider_signer(2);
        let mut terms = primary_terms(1, 100, 100, 0);
        terms.bucket_id = Some(0);
        let sig = sign_terms(&pair, &terms);

        assert_noop!(
            StorageProvider::establish_storage_agreement(RuntimeOrigin::signed(1), 2, terms, sig),
            Error::<Test>::TermsBucketMismatch
        );
    });
}

#[test]
fn establish_storage_agreement_fails_when_terms_expired() {
    new_test_ext().execute_with(|| {
        register_provider(2, 200);
        run_to_block(10);

        let pair = provider_signer(2);
        let mut terms = primary_terms(1, 100, 100, 0);
        terms.valid_until = 5; // already past at block 10
        let sig = sign_terms(&pair, &terms);

        assert_noop!(
            StorageProvider::establish_storage_agreement(RuntimeOrigin::signed(1), 2, terms, sig),
            Error::<Test>::TermsExpired
        );
    });
}

#[test]
fn establish_storage_agreement_fails_with_invalid_signature() {
    new_test_ext().execute_with(|| {
        register_provider(2, 200);

        // Stamp the provider's real key, then sign with a different key.
        provider_signer(2);
        let wrong_pair = sp_core::sr25519::Pair::from_seed(&[99u8; 32]);
        let terms = primary_terms(1, 100, 100, 0);
        let sig = sign_terms(&wrong_pair, &terms);

        assert_noop!(
            StorageProvider::establish_storage_agreement(RuntimeOrigin::signed(1), 2, terms, sig),
            Error::<Test>::InvalidProviderSignature
        );
    });
}

#[test]
fn establish_storage_agreement_fails_on_nonce_replay() {
    new_test_ext().execute_with(|| {
        register_provider(2, 200);

        let (terms, sig) = signed_primary_terms(2, 1, 50, 100);
        assert_ok!(StorageProvider::establish_storage_agreement(
            RuntimeOrigin::signed(1),
            2,
            terms.clone(),
            sig.clone()
        ));

        // Redeeming the exact same quote again is a replay.
        assert_noop!(
            StorageProvider::establish_storage_agreement(RuntimeOrigin::signed(1), 2, terms, sig),
            Error::<Test>::NonceAlreadyUsed
        );
    });
}

#[test]
fn establish_storage_agreement_fails_provider_not_found() {
    new_test_ext().execute_with(|| {
        let terms = primary_terms(1, 50, 100, 0);
        assert_noop!(
            StorageProvider::establish_storage_agreement(
                RuntimeOrigin::signed(1),
                99, // not registered
                terms,
                sp_runtime::MultiSignature::Sr25519([0u8; 64].into()),
            ),
            Error::<Test>::ProviderNotFound
        );
    });
}

#[test]
fn establish_storage_agreement_fails_not_accepting_primary() {
    new_test_ext().execute_with(|| {
        register_provider_with_settings(
            2,
            200,
            ProviderSettings {
                accepting_primary: false,
                ..Default::default()
            },
        );

        // The acceptance check runs after the nonce window advances, so
        // storage is mutated even on failure — assert the error only.
        let (terms, sig) = signed_primary_terms(2, 1, 50, 100);
        assert_err!(
            StorageProvider::establish_storage_agreement(RuntimeOrigin::signed(1), 2, terms, sig),
            Error::<Test>::ProviderNotAcceptingPrimary
        );
    });
}

#[test]
fn establish_storage_agreement_fails_duration_too_long() {
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

        let (terms, sig) = signed_primary_terms(2, 1, 50, 100); // exceeds max_duration
        assert_err!(
            StorageProvider::establish_storage_agreement(RuntimeOrigin::signed(1), 2, terms, sig),
            Error::<Test>::DurationTooLong
        );
    });
}

#[test]
fn establish_storage_agreement_fails_insufficient_stake() {
    new_test_ext().execute_with(|| {
        // Stake of 200 only covers 200 bytes (MinStakePerByte = 1).
        register_provider(2, 200);

        let (terms, sig) = signed_primary_terms(2, 1, 300, 100);
        assert_err!(
            StorageProvider::establish_storage_agreement(RuntimeOrigin::signed(1), 2, terms, sig),
            Error::<Test>::InsufficientStakeForBytes
        );
    });
}
