use super::*;
use storage_primitives::ReplicaRequestParams;

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
fn request_agreement_replica_works() {
    new_test_ext().execute_with(|| {
        setup_provider_with_replicas(2, 200);
        assert_ok!(StorageProvider::create_bucket(RuntimeOrigin::signed(1), 1));

        let balance_before = Balances::free_balance(1);

        assert_ok!(StorageProvider::request_agreement(
            RuntimeOrigin::signed(1),
            0,
            2,
            50,
            100,
            10000,
            ReplicaRequestParams {
                sync_balance: 100,
                min_sync_interval: 10,
            }
        ));

        let request = AgreementRequests::<Test>::get(0, 2).unwrap();
        assert!(request.replica_params.is_some());
        // Payment + sync_balance should be reserved
        assert!(Balances::free_balance(1) < balance_before);
    });
}

#[test]
fn request_agreement_fails_no_replica_sync_price() {
    new_test_ext().execute_with(|| {
        // Provider without replica_sync_price
        register_provider(2, 200);
        assert_ok!(StorageProvider::create_bucket(RuntimeOrigin::signed(1), 1));

        assert_noop!(
            StorageProvider::request_agreement(
                RuntimeOrigin::signed(1),
                0,
                2,
                50,
                100,
                10000,
                ReplicaRequestParams {
                    sync_balance: 100,
                    min_sync_interval: 10,
                }
            ),
            Error::<Test>::ProviderNotAcceptingReplicas
        );
    });
}

#[test]
fn request_agreement_fails_deregister_announced() {
    new_test_ext().execute_with(|| {
        setup_provider_with_replicas(2, 200);
        assert_ok!(StorageProvider::create_bucket(RuntimeOrigin::signed(1), 1));

        // Announce deregistration
        assert_ok!(StorageProvider::deregister_provider(RuntimeOrigin::signed(
            2
        )));

        assert_noop!(
            StorageProvider::request_agreement(
                RuntimeOrigin::signed(1),
                0,
                2,
                50,
                100,
                10000,
                ReplicaRequestParams {
                    sync_balance: 100,
                    min_sync_interval: 10,
                }
            ),
            Error::<Test>::DeregisterAnnounced
        );
    });
}

#[test]
fn request_agreement_fails_duplicate() {
    new_test_ext().execute_with(|| {
        setup_provider_with_replicas(2, 200);
        assert_ok!(StorageProvider::create_bucket(RuntimeOrigin::signed(1), 1));

        assert_ok!(StorageProvider::request_agreement(
            RuntimeOrigin::signed(1),
            0,
            2,
            50,
            100,
            10000,
            ReplicaRequestParams {
                sync_balance: 100,
                min_sync_interval: 10,
            }
        ));

        assert_noop!(
            StorageProvider::request_agreement(
                RuntimeOrigin::signed(1),
                0,
                2,
                50,
                100,
                10000,
                ReplicaRequestParams {
                    sync_balance: 100,
                    min_sync_interval: 10,
                }
            ),
            Error::<Test>::AgreementRequestAlreadyExists
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

        // Request replica agreement
        assert_ok!(StorageProvider::request_agreement(
            RuntimeOrigin::signed(1),
            bucket_id,
            2,
            50,
            200,
            10000,
            ReplicaRequestParams {
                sync_balance: 100,
                min_sync_interval: 10,
            }
        ));
        assert_ok!(StorageProvider::accept_agreement(
            RuntimeOrigin::signed(2),
            bucket_id
        ));

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
        assert_ok!(StorageProvider::create_bucket(RuntimeOrigin::signed(1), 1));

        assert_noop!(
            StorageProvider::top_up_replica_sync_balance(RuntimeOrigin::signed(1), 0, 2, 100),
            Error::<Test>::AgreementNotFound
        );
    });
}
