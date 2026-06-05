use super::*;
use storage_primitives::agreement_term::ReplicaTerms;

fn setup_provider_with_replicas(provider: u64, stake: u64) {
    register_provider_with_settings(
        provider,
        stake,
        ProviderSettings {
            accepting_primary: true,
            replica_sync_price: Some(10),
            ..Default::default()
        },
    );
}

#[test]
fn establish_replica_agreement_works() {
    new_test_ext().execute_with(|| {
        setup_provider_with_replicas(2, 200);
        let bucket_id = create_bucket(1, 0);

        let balance_before = Balances::free_balance(1);

        setup_replica_agreement(
            2,
            1,
            bucket_id,
            50,
            100,
            ReplicaTerms {
                sync_balance: 100,
                min_sync_interval: 10,
            },
        );

        let agreement = StorageAgreements::<Test>::get(bucket_id, 2).unwrap();
        assert!(matches!(
            agreement.role,
            storage_primitives::ProviderRole::Replica { .. }
        ));
        // sync_balance (price is 0, so only the sync balance) is reserved.
        assert_eq!(Balances::free_balance(1), balance_before - 100);
    });
}

#[test]
fn establish_replica_agreement_fails_no_replica_sync_price() {
    new_test_ext().execute_with(|| {
        // Provider without replica_sync_price
        register_provider(2, 200);
        let bucket_id = create_bucket(1, 0);

        // The sync-price check runs after the nonce window advances, so
        // storage is mutated even on failure — assert the error only.
        let (terms, sig) = signed_replica_terms(
            2,
            1,
            bucket_id,
            50,
            100,
            ReplicaTerms {
                sync_balance: 100,
                min_sync_interval: 10,
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
            Error::<Test>::ProviderNotAcceptingReplicas
        );
    });
}

#[test]
fn establish_replica_agreement_fails_deregister_announced() {
    new_test_ext().execute_with(|| {
        setup_provider_with_replicas(2, 200);
        let bucket_id = create_bucket(1, 0);

        // Announce deregistration
        assert_ok!(StorageProvider::deregister_provider(RuntimeOrigin::signed(
            2
        )));

        let (terms, sig) = signed_replica_terms(
            2,
            1,
            bucket_id,
            50,
            100,
            ReplicaTerms {
                sync_balance: 100,
                min_sync_interval: 10,
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
fn establish_replica_agreement_fails_duplicate() {
    new_test_ext().execute_with(|| {
        setup_provider_with_replicas(2, 200);
        let bucket_id = create_bucket(1, 0);

        setup_replica_agreement(
            2,
            1,
            bucket_id,
            50,
            100,
            ReplicaTerms {
                sync_balance: 100,
                min_sync_interval: 10,
            },
        );

        // A second quote (fresh nonce) for the same (bucket, provider)
        // pair is rejected before any state changes.
        let (terms, sig) = signed_replica_terms(
            2,
            1,
            bucket_id,
            50,
            100,
            ReplicaTerms {
                sync_balance: 100,
                min_sync_interval: 10,
            },
        );
        assert_noop!(
            StorageProvider::establish_replica_agreement(
                RuntimeOrigin::signed(1),
                bucket_id,
                2,
                terms,
                sig
            ),
            Error::<Test>::AgreementAlreadyExists
        );
    });
}

#[test]
fn establish_replica_agreement_fails_bucket_not_found() {
    new_test_ext().execute_with(|| {
        setup_provider_with_replicas(2, 200);

        let (terms, sig) = signed_replica_terms(
            2,
            1,
            999, // non-existent bucket
            50,
            100,
            ReplicaTerms {
                sync_balance: 100,
                min_sync_interval: 10,
            },
        );
        assert_noop!(
            StorageProvider::establish_replica_agreement(
                RuntimeOrigin::signed(1),
                999,
                2,
                terms,
                sig
            ),
            Error::<Test>::BucketNotFound
        );
    });
}

#[test]
fn establish_replica_agreement_fails_terms_bucket_mismatch() {
    new_test_ext().execute_with(|| {
        setup_provider_with_replicas(2, 200);
        let bucket_id = create_bucket(1, 0);
        let other_bucket = create_bucket(1, 0);

        // Quote bound to a different bucket than the extrinsic targets.
        let (terms, sig) = signed_replica_terms(
            2,
            1,
            other_bucket,
            50,
            100,
            ReplicaTerms {
                sync_balance: 100,
                min_sync_interval: 10,
            },
        );
        assert_noop!(
            StorageProvider::establish_replica_agreement(
                RuntimeOrigin::signed(1),
                bucket_id,
                2,
                terms,
                sig
            ),
            Error::<Test>::TermsBucketMismatch
        );
    });
}

#[test]
fn establish_replica_agreement_fails_missing_replica_terms() {
    new_test_ext().execute_with(|| {
        setup_provider_with_replicas(2, 200);
        let bucket_id = create_bucket(1, 0);

        // Bucket-bound terms without replica params.
        let pair = provider_signer(2);
        let mut terms = primary_terms(1, 50, 100, 0);
        terms.bucket_id = Some(bucket_id);
        let sig = sign_terms(&pair, &terms);

        assert_noop!(
            StorageProvider::establish_replica_agreement(
                RuntimeOrigin::signed(1),
                bucket_id,
                2,
                terms,
                sig
            ),
            Error::<Test>::MissingReplicaTerms
        );
    });
}

#[test]
fn confirm_replica_sync_fails_not_replica() {
    new_test_ext().execute_with(|| {
        register_provider(2, 200);
        let bucket_id = setup_agreement(2, 1, 50, 200);

        assert_noop!(
            StorageProvider::confirm_replica_sync(
                RuntimeOrigin::signed(2),
                bucket_id,
                [None; 7],
                sp_runtime::MultiSignature::Sr25519([0u8; 64].into()),
            ),
            Error::<Test>::NotReplica
        );
    });
}

#[test]
fn top_up_replica_sync_balance_fails_not_replica() {
    new_test_ext().execute_with(|| {
        register_provider(2, 200);
        let bucket_id = setup_agreement(2, 1, 50, 200);

        assert_noop!(
            StorageProvider::top_up_replica_sync_balance(
                RuntimeOrigin::signed(1),
                bucket_id,
                2,
                100
            ),
            Error::<Test>::NotReplica
        );
    });
}

#[test]
fn top_up_replica_sync_balance_works() {
    new_test_ext().execute_with(|| {
        setup_provider_with_replicas(2, 200);

        // Need a primary provider first for the bucket
        register_provider(3, 200);
        let bucket_id = setup_agreement(3, 1, 50, 200);

        setup_replica_agreement(
            2,
            1,
            bucket_id,
            50,
            200,
            ReplicaTerms {
                sync_balance: 100,
                min_sync_interval: 10,
            },
        );

        let agreement_before = StorageAgreements::<Test>::get(bucket_id, 2).unwrap();
        let sync_balance_before = match &agreement_before.role {
            storage_primitives::ProviderRole::Replica { sync_balance, .. } => *sync_balance,
            _ => panic!("expected replica"),
        };

        assert_ok!(StorageProvider::top_up_replica_sync_balance(
            RuntimeOrigin::signed(1),
            bucket_id,
            2,
            50
        ));

        let agreement_after = StorageAgreements::<Test>::get(bucket_id, 2).unwrap();
        let sync_balance_after = match &agreement_after.role {
            storage_primitives::ProviderRole::Replica { sync_balance, .. } => *sync_balance,
            _ => panic!("expected replica"),
        };
        assert_eq!(sync_balance_after, sync_balance_before + 50);
    });
}

#[test]
fn top_up_replica_sync_balance_fails_no_agreement() {
    new_test_ext().execute_with(|| {
        let bucket_id = create_bucket(1, 0);

        assert_noop!(
            StorageProvider::top_up_replica_sync_balance(
                RuntimeOrigin::signed(1),
                bucket_id,
                2,
                100
            ),
            Error::<Test>::AgreementNotFound
        );
    });
}
