use super::*;
use sp_core::H256;
use storage_primitives::{BucketSnapshot, ChallengeId};

/// Setup: register provider, create agreement, and insert a snapshot with provider signed.
fn setup_with_snapshot(provider: u64, client: u64) -> u64 {
    register_provider(provider, 200);
    let bucket_id = setup_agreement(provider, client, 50, 200);

    // Insert a snapshot where the provider has signed
    Buckets::<Test>::mutate(bucket_id, |maybe_bucket| {
        if let Some(bucket) = maybe_bucket {
            bucket.snapshot = Some(BucketSnapshot {
                mmr_root: H256::repeat_byte(0xAB),
                start_seq: 0,
                leaf_count: 10,
                checkpoint_block: 1,
                primary_signers: vec![0x01], // bit 0 set = provider at index 0 signed
            });
        }
    });

    bucket_id
}

#[test]
fn challenge_checkpoint_works() {
    new_test_ext().execute_with(|| {
        frame_system::Pallet::<Test>::set_block_number(1);
        let bucket_id = setup_with_snapshot(2, 1);

        let challenger_balance_before = Balances::free_balance(3);

        assert_ok!(StorageProvider::challenge_checkpoint(
            RuntimeOrigin::signed(3),
            bucket_id,
            2,
            0, // leaf_index
            0, // chunk_index
        ));

        // Challenge deposit (100) should be reserved
        assert_eq!(Balances::free_balance(3), challenger_balance_before - 100);

        // Challenge should exist at deadline = current_block(1) + ChallengeTimeout(100) = 101
        let challenges = Challenges::<Test>::get(101).unwrap();
        assert_eq!(challenges.len(), 1);
        assert_eq!(challenges[0].provider, 2);
        assert_eq!(challenges[0].challenger, 3);
    });
}

#[test]
fn challenge_checkpoint_fails_no_snapshot() {
    new_test_ext().execute_with(|| {
        register_provider(2, 200);
        let bucket_id = setup_agreement(2, 1, 50, 200);

        // No snapshot inserted
        assert_noop!(
            StorageProvider::challenge_checkpoint(RuntimeOrigin::signed(3), bucket_id, 2, 0, 0),
            Error::<Test>::NoSnapshot
        );
    });
}

#[test]
fn challenge_checkpoint_fails_provider_not_signed() {
    new_test_ext().execute_with(|| {
        // Register two providers
        register_provider(2, 200);
        register_provider(3, 200);
        let bucket_id = setup_agreement(2, 1, 50, 200);

        // Add second provider (no extrinsic grows a bucket's primary
        // set, so the shape is synthesized directly)
        add_primary_to_bucket(3, 1, bucket_id, 50);

        // Insert snapshot where only provider at index 0 (account 2) signed
        Buckets::<Test>::mutate(bucket_id, |maybe_bucket| {
            if let Some(bucket) = maybe_bucket {
                bucket.snapshot = Some(BucketSnapshot {
                    mmr_root: H256::repeat_byte(0xAB),
                    start_seq: 0,
                    leaf_count: 10,
                    checkpoint_block: 1,
                    primary_signers: vec![0x01], // only bit 0 set
                });
            }
        });

        // Challenge provider 3 (at index 1, not signed) should fail
        assert_noop!(
            StorageProvider::challenge_checkpoint(RuntimeOrigin::signed(4), bucket_id, 3, 0, 0),
            Error::<Test>::ProviderNotInSnapshot
        );
    });
}

#[test]
fn challenge_offchain_fails_no_agreement() {
    new_test_ext().execute_with(|| {
        register_provider(2, 200);
        create_bucket(1, 1);

        assert_noop!(
            StorageProvider::challenge_offchain(
                RuntimeOrigin::signed(3),
                0,
                2,
                H256::repeat_byte(0xAB),
                0,
                0,
                0,
                sp_runtime::MultiSignature::Sr25519([0u8; 64].into()),
            ),
            Error::<Test>::AgreementNotFound
        );
    });
}

#[test]
fn challenge_replica_fails_not_replica() {
    new_test_ext().execute_with(|| {
        register_provider(2, 200);
        let bucket_id = setup_agreement(2, 1, 50, 200);

        // Provider 2 has a Primary agreement, not Replica
        assert_noop!(
            StorageProvider::challenge_replica(RuntimeOrigin::signed(3), bucket_id, 2, 0, 0),
            Error::<Test>::NotReplica
        );
    });
}

#[test]
fn respond_to_challenge_fails_not_provider() {
    new_test_ext().execute_with(|| {
        frame_system::Pallet::<Test>::set_block_number(1);
        let bucket_id = setup_with_snapshot(2, 1);

        assert_ok!(StorageProvider::challenge_checkpoint(
            RuntimeOrigin::signed(3),
            bucket_id,
            2,
            0,
            0,
        ));

        let challenge_id = ChallengeId {
            deadline: 101, // block 1 + ChallengeTimeout(100)
            index: 0,
        };

        // Account 4 is not the challenged provider
        assert_noop!(
            StorageProvider::respond_to_challenge(
                RuntimeOrigin::signed(4),
                challenge_id,
                crate::ChallengeResponse::Superseded,
            ),
            Error::<Test>::NotChallengeProvider
        );
    });
}

#[test]
fn respond_to_challenge_fails_expired() {
    new_test_ext().execute_with(|| {
        frame_system::Pallet::<Test>::set_block_number(1);
        let bucket_id = setup_with_snapshot(2, 1);

        assert_ok!(StorageProvider::challenge_checkpoint(
            RuntimeOrigin::signed(3),
            bucket_id,
            2,
            0,
            0,
        ));

        let challenge_id = ChallengeId {
            deadline: 101,
            index: 0,
        };

        // Advance past deadline (run_to_block only calls System hooks,
        // not the pallet's on_finalize, so the challenge still exists)
        run_to_block(102);

        assert_noop!(
            StorageProvider::respond_to_challenge(
                RuntimeOrigin::signed(2),
                challenge_id,
                crate::ChallengeResponse::Superseded,
            ),
            Error::<Test>::ChallengeExpired
        );
    });
}

#[test]
fn respond_to_challenge_superseded_works() {
    new_test_ext().execute_with(|| {
        frame_system::Pallet::<Test>::set_block_number(1);
        let bucket_id = setup_with_snapshot(2, 1);

        // Challenge at leaf_index 0 against snapshot with leaf_count 10
        assert_ok!(StorageProvider::challenge_checkpoint(
            RuntimeOrigin::signed(3),
            bucket_id,
            2,
            0, // leaf_index
            0,
        ));

        let challenge_id = ChallengeId {
            deadline: 101,
            index: 0,
        };

        // The snapshot has leaf_count=10, start_seq=0, so canonical_end = 10.
        // challenged_seq = start_seq(0) + leaf_index(0) = 0, which is < 10, so Superseded works.
        assert_ok!(StorageProvider::respond_to_challenge(
            RuntimeOrigin::signed(2),
            challenge_id,
            crate::ChallengeResponse::Superseded,
        ));

        // Challenge should be removed
        assert!(Challenges::<Test>::get(101).is_none());
    });
}

#[test]
fn challenge_slashes_provider_on_timeout() {
    new_test_ext().execute_with(|| {
        frame_system::Pallet::<Test>::set_block_number(1);
        let bucket_id = setup_with_snapshot(2, 1);

        let provider_stake_before = Providers::<Test>::get(2).unwrap().stake;

        assert_ok!(StorageProvider::challenge_checkpoint(
            RuntimeOrigin::signed(3),
            bucket_id,
            2,
            0,
            0,
        ));

        // Challenge deadline = block 1 + ChallengeTimeout(100) = 101
        // run_to_block only calls System hooks, so manually call pallet on_finalize
        run_to_block(101);
        <StorageProvider as frame_support::traits::Hooks<u64>>::on_finalize(101);

        // Provider should be slashed
        let provider = Providers::<Test>::get(2).unwrap();
        assert!(provider.stake < provider_stake_before);
        assert_eq!(provider.stats.challenges_failed, 1);
    });
}
