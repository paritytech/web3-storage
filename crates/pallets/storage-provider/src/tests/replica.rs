// SPDX-License-Identifier: Apache-2.0

use super::*;
use storage_primitives::agreement_term::ReplicaTerms;

fn setup_provider_with_replicas(provider: u64, stake: u64) {
    register_provider_with_settings(
        provider,
        stake,
        ProviderSettingsOf::<Test> {
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
                sync_price: 10,
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
                sync_price: 10,
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
                sync_price: 10,
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
                sync_price: 10,
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
                sync_price: 10,
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
                sync_price: 10,
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

/// Helper: set up a bucket with a primary provider (3) holding a snapshot,
/// and a replica provider (2) with an accepted replica agreement.
fn setup_replica_with_snapshot() -> u64 {
    use sp_core::H256;
    use storage_primitives::{BucketSnapshot, Commitment};

    // Provider 3 = primary
    register_provider(3, 200);
    // Provider 2 = replica
    setup_provider_with_replicas(2, 200);

    // Create bucket with primary agreement on provider 3
    let bucket_id = setup_agreement(3, 1, 50, 200);

    // Insert a snapshot on the bucket (provider 3 signed)
    Buckets::<Test>::mutate(bucket_id, |maybe_bucket| {
        if let Some(bucket) = maybe_bucket {
            bucket.snapshot = Some(BucketSnapshot {
                commitment: Commitment {
                    mmr_root: H256::repeat_byte(0xAB),
                    start_seq: 0,
                    leaf_count: 10,
                },
                checkpoint_block: 1,
                primary_signers: vec![0x01],
                commitment_nonce: 0,
            });
        }
    });

    // Establish a replica agreement for provider 2 via provider-signed terms.
    setup_replica_agreement(
        2,
        1,
        bucket_id,
        50,
        200,
        ReplicaTerms {
            sync_balance: 500,
            min_sync_interval: 10,
            sync_price: 10,
        },
    );

    bucket_id
}

#[test]
fn confirm_replica_sync_happy_path() {
    new_test_ext().execute_with(|| {
        frame_system::Pallet::<Test>::set_block_number(1);
        let bucket_id = setup_replica_with_snapshot();

        let provider_balance_before = Balances::free_balance(2);

        // Sync with current snapshot root at position 0
        let mut roots = [None; 7];
        roots[0] = Some(sp_core::H256::repeat_byte(0xAB));

        assert_ok!(StorageProvider::confirm_replica_sync(
            RuntimeOrigin::signed(2),
            bucket_id,
            roots,
            sp_runtime::MultiSignature::Sr25519([0u8; 64].into()),
        ));

        // Provider should receive sync_price (10)
        assert_eq!(Balances::free_balance(2), provider_balance_before + 10);

        // Sync balance should decrease
        let agreement = StorageAgreements::<Test>::get(bucket_id, 2).unwrap();
        match &agreement.role {
            storage_primitives::ProviderRole::Replica {
                sync_balance,
                last_sync,
                ..
            } => {
                assert_eq!(*sync_balance, 490); // 500 - 10
                assert!(last_sync.is_some());
                let record = last_sync.unwrap();
                assert_eq!(record.commitment.mmr_root, sp_core::H256::repeat_byte(0xAB));
                assert_eq!(record.block, 1);
            }
            _ => panic!("expected replica"),
        }
    });
}

#[test]
fn confirm_replica_sync_fails_sync_too_frequent() {
    new_test_ext().execute_with(|| {
        frame_system::Pallet::<Test>::set_block_number(1);
        let bucket_id = setup_replica_with_snapshot();

        // First sync
        let mut roots = [None; 7];
        roots[0] = Some(sp_core::H256::repeat_byte(0xAB));
        assert_ok!(StorageProvider::confirm_replica_sync(
            RuntimeOrigin::signed(2),
            bucket_id,
            roots,
            sp_runtime::MultiSignature::Sr25519([0u8; 64].into()),
        ));

        // Update snapshot to a new root so we don't hit InvalidSyncRoot
        Buckets::<Test>::mutate(bucket_id, |maybe_bucket| {
            if let Some(bucket) = maybe_bucket {
                if let Some(snapshot) = &mut bucket.snapshot {
                    snapshot.commitment.mmr_root = sp_core::H256::repeat_byte(0xCD);
                }
            }
        });

        // Try to sync again before min_sync_interval (10 blocks)
        run_to_block(5); // Only 4 blocks later, need at least 10

        let mut roots2 = [None; 7];
        roots2[0] = Some(sp_core::H256::repeat_byte(0xCD));
        assert_noop!(
            StorageProvider::confirm_replica_sync(
                RuntimeOrigin::signed(2),
                bucket_id,
                roots2,
                sp_runtime::MultiSignature::Sr25519([0u8; 64].into()),
            ),
            Error::<Test>::SyncTooFrequent
        );
    });
}

#[test]
fn confirm_replica_sync_fails_insufficient_balance() {
    new_test_ext().execute_with(|| {
        frame_system::Pallet::<Test>::set_block_number(1);

        // Set up replica with very low sync_balance
        register_provider(3, 200);
        setup_provider_with_replicas(2, 200);
        let bucket_id = setup_agreement(3, 1, 50, 200);

        Buckets::<Test>::mutate(bucket_id, |maybe_bucket| {
            if let Some(bucket) = maybe_bucket {
                bucket.snapshot = Some(storage_primitives::BucketSnapshot {
                    commitment: storage_primitives::Commitment {
                        mmr_root: sp_core::H256::repeat_byte(0xAB),
                        start_seq: 0,
                        leaf_count: 10,
                    },
                    checkpoint_block: 1,
                    primary_signers: vec![0x01],
                    commitment_nonce: 0,
                });
            }
        });

        // Establish replica with sync_balance < sync_price
        setup_replica_agreement(
            2,
            1,
            bucket_id,
            50,
            200,
            ReplicaTerms {
                sync_balance: 5, // Less than sync_price (10)
                min_sync_interval: 10,
                sync_price: 10,
            },
        );

        let mut roots = [None; 7];
        roots[0] = Some(sp_core::H256::repeat_byte(0xAB));

        assert_noop!(
            StorageProvider::confirm_replica_sync(
                RuntimeOrigin::signed(2),
                bucket_id,
                roots,
                sp_runtime::MultiSignature::Sr25519([0u8; 64].into()),
            ),
            Error::<Test>::InsufficientSyncBalance
        );
    });
}

#[test]
fn confirm_replica_sync_fails_invalid_root() {
    new_test_ext().execute_with(|| {
        frame_system::Pallet::<Test>::set_block_number(1);
        let bucket_id = setup_replica_with_snapshot();

        // Provide a root that doesn't match snapshot or historical roots
        let mut roots = [None; 7];
        roots[0] = Some(sp_core::H256::repeat_byte(0xFF)); // Wrong root

        assert_noop!(
            StorageProvider::confirm_replica_sync(
                RuntimeOrigin::signed(2),
                bucket_id,
                roots,
                sp_runtime::MultiSignature::Sr25519([0u8; 64].into()),
            ),
            Error::<Test>::InvalidSyncRoot
        );
    });
}

#[test]
fn confirm_replica_sync_fails_same_root() {
    new_test_ext().execute_with(|| {
        frame_system::Pallet::<Test>::set_block_number(1);
        let bucket_id = setup_replica_with_snapshot();

        // First sync succeeds
        let mut roots = [None; 7];
        roots[0] = Some(sp_core::H256::repeat_byte(0xAB));
        assert_ok!(StorageProvider::confirm_replica_sync(
            RuntimeOrigin::signed(2),
            bucket_id,
            roots,
            sp_runtime::MultiSignature::Sr25519([0u8; 64].into()),
        ));

        // Advance past min_sync_interval
        run_to_block(12);

        // Try to sync with same root — should fail
        assert_noop!(
            StorageProvider::confirm_replica_sync(
                RuntimeOrigin::signed(2),
                bucket_id,
                roots,
                sp_runtime::MultiSignature::Sr25519([0u8; 64].into()),
            ),
            Error::<Test>::InvalidSyncRoot
        );
    });
}

#[test]
fn confirm_replica_sync_emits_event() {
    new_test_ext().execute_with(|| {
        frame_system::Pallet::<Test>::set_block_number(1);
        let bucket_id = setup_replica_with_snapshot();

        let mut roots = [None; 7];
        roots[0] = Some(sp_core::H256::repeat_byte(0xAB));

        assert_ok!(StorageProvider::confirm_replica_sync(
            RuntimeOrigin::signed(2),
            bucket_id,
            roots,
            sp_runtime::MultiSignature::Sr25519([0u8; 64].into()),
        ));

        let expected = RuntimeEvent::StorageProvider(crate::Event::ReplicaSynced {
            bucket_id,
            provider: 2,
            mmr_root: sp_core::H256::repeat_byte(0xAB),
            position_matched: 0,
            sync_payment: 10,
        });
        assert!(frame_system::Pallet::<Test>::events()
            .iter()
            .any(|r| r.event == expected));
    });
}

#[test]
fn confirm_replica_sync_after_interval_with_new_root() {
    new_test_ext().execute_with(|| {
        frame_system::Pallet::<Test>::set_block_number(1);
        let bucket_id = setup_replica_with_snapshot();

        // First sync
        let mut roots = [None; 7];
        roots[0] = Some(sp_core::H256::repeat_byte(0xAB));
        assert_ok!(StorageProvider::confirm_replica_sync(
            RuntimeOrigin::signed(2),
            bucket_id,
            roots,
            sp_runtime::MultiSignature::Sr25519([0u8; 64].into()),
        ));

        // Update snapshot root
        Buckets::<Test>::mutate(bucket_id, |maybe_bucket| {
            if let Some(bucket) = maybe_bucket {
                if let Some(snapshot) = &mut bucket.snapshot {
                    snapshot.commitment.mmr_root = sp_core::H256::repeat_byte(0xCD);
                }
            }
        });

        // Advance past min_sync_interval (10)
        run_to_block(12);

        // Second sync with new root should succeed
        let mut roots2 = [None; 7];
        roots2[0] = Some(sp_core::H256::repeat_byte(0xCD));
        assert_ok!(StorageProvider::confirm_replica_sync(
            RuntimeOrigin::signed(2),
            bucket_id,
            roots2,
            sp_runtime::MultiSignature::Sr25519([0u8; 64].into()),
        ));

        // Verify last_sync updated
        let agreement = StorageAgreements::<Test>::get(bucket_id, 2).unwrap();
        match &agreement.role {
            storage_primitives::ProviderRole::Replica {
                sync_balance,
                last_sync,
                ..
            } => {
                assert_eq!(*sync_balance, 480); // 500 - 10 - 10
                let record = last_sync.unwrap();
                assert_eq!(record.commitment.mmr_root, sp_core::H256::repeat_byte(0xCD));
                assert_eq!(record.block, 12);
            }
            _ => panic!("expected replica"),
        }
    });
}
