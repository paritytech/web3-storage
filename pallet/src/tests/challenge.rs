// SPDX-License-Identifier: Apache-2.0

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
        // run_to_block(102) finalises block 101, triggering pallet on_finalize
        run_to_block(102);

        // Provider should be slashed
        let provider = Providers::<Test>::get(2).unwrap();
        assert!(provider.stake < provider_stake_before);
        assert_eq!(provider.stats.challenges_failed, 1);
    });
}

#[test]
fn respond_to_challenge_superseded_fails_leaf_beyond_canonical() {
    new_test_ext().execute_with(|| {
        frame_system::Pallet::<Test>::set_block_number(1);
        let bucket_id = setup_with_snapshot(2, 1);

        // Challenge at leaf_index 10 against snapshot with leaf_count=10, start_seq=0
        // challenged_seq = 0 + 10 = 10, canonical_end = 0 + 10 = 10
        // 10 < 10 is false → LeafBeyondCanonical
        assert_ok!(StorageProvider::challenge_checkpoint(
            RuntimeOrigin::signed(3),
            bucket_id,
            2,
            10, // leaf_index at boundary
            0,
        ));

        let challenge_id = ChallengeId {
            deadline: 101,
            index: 0,
        };

        assert_noop!(
            StorageProvider::respond_to_challenge(
                RuntimeOrigin::signed(2),
                challenge_id,
                crate::ChallengeResponse::Superseded,
            ),
            Error::<Test>::LeafBeyondCanonical
        );
    });
}

#[test]
fn respond_to_challenge_superseded_cost_split_block_1() {
    new_test_ext().execute_with(|| {
        frame_system::Pallet::<Test>::set_block_number(1);
        let bucket_id = setup_with_snapshot(2, 1);

        let challenger_balance_before = Balances::free_balance(3);
        let provider_stake_before = Providers::<Test>::get(2).unwrap().stake;

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

        // Respond at block 2 → response_time = 2 - (101-100) = 2 - 1 = 1
        run_to_block(2);

        assert_ok!(StorageProvider::respond_to_challenge(
            RuntimeOrigin::signed(2),
            challenge_id,
            crate::ChallengeResponse::Superseded,
        ));

        // Block 1: challenger 90%, provider 10%
        // deposit = 100, challenger_cost = 90, provider_cost = 10
        // Challenger gets unreserved (100 - 90) = 10 back
        assert_eq!(Balances::free_balance(3), challenger_balance_before - 90);

        // Provider stake decreased by 10
        let provider_stake_after = Providers::<Test>::get(2).unwrap().stake;
        assert_eq!(provider_stake_after, provider_stake_before - 10);
    });
}

#[test]
fn respond_to_challenge_superseded_cost_split_block_5() {
    new_test_ext().execute_with(|| {
        frame_system::Pallet::<Test>::set_block_number(1);
        let bucket_id = setup_with_snapshot(2, 1);

        let challenger_balance_before = Balances::free_balance(3);
        let provider_stake_before = Providers::<Test>::get(2).unwrap().stake;

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

        // Respond at block 6 → response_time = 6 - 1 = 5
        // Blocks 2-5: challenger 80%, provider 20%
        run_to_block(6);

        assert_ok!(StorageProvider::respond_to_challenge(
            RuntimeOrigin::signed(2),
            challenge_id,
            crate::ChallengeResponse::Superseded,
        ));

        // challenger_cost = 80, provider_cost = 20
        assert_eq!(Balances::free_balance(3), challenger_balance_before - 80);
        let provider_stake_after = Providers::<Test>::get(2).unwrap().stake;
        assert_eq!(provider_stake_after, provider_stake_before - 20);
    });
}

#[test]
fn respond_to_challenge_superseded_cost_split_block_24() {
    new_test_ext().execute_with(|| {
        frame_system::Pallet::<Test>::set_block_number(1);
        let bucket_id = setup_with_snapshot(2, 1);

        let challenger_balance_before = Balances::free_balance(3);
        let provider_stake_before = Providers::<Test>::get(2).unwrap().stake;

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

        // Respond at block 25 → response_time = 25 - 1 = 24
        // Blocks 6-24: challenger 70%, provider 30%
        run_to_block(25);

        assert_ok!(StorageProvider::respond_to_challenge(
            RuntimeOrigin::signed(2),
            challenge_id,
            crate::ChallengeResponse::Superseded,
        ));

        // challenger_cost = 70, provider_cost = 30
        assert_eq!(Balances::free_balance(3), challenger_balance_before - 70);
        let provider_stake_after = Providers::<Test>::get(2).unwrap().stake;
        assert_eq!(provider_stake_after, provider_stake_before - 30);
    });
}

#[test]
fn respond_to_challenge_superseded_cost_split_block_95() {
    new_test_ext().execute_with(|| {
        frame_system::Pallet::<Test>::set_block_number(1);
        let bucket_id = setup_with_snapshot(2, 1);

        let challenger_balance_before = Balances::free_balance(3);
        let provider_stake_before = Providers::<Test>::get(2).unwrap().stake;

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

        // Respond at block 96 → response_time = 96 - 1 = 95
        // Blocks 25-95: challenger 60%, provider 40%
        run_to_block(96);

        assert_ok!(StorageProvider::respond_to_challenge(
            RuntimeOrigin::signed(2),
            challenge_id,
            crate::ChallengeResponse::Superseded,
        ));

        // challenger_cost = 60, provider_cost = 40
        assert_eq!(Balances::free_balance(3), challenger_balance_before - 60);
        let provider_stake_after = Providers::<Test>::get(2).unwrap().stake;
        assert_eq!(provider_stake_after, provider_stake_before - 40);
    });
}

#[test]
fn respond_to_challenge_superseded_cost_split_block_96_plus() {
    new_test_ext().execute_with(|| {
        frame_system::Pallet::<Test>::set_block_number(1);
        let bucket_id = setup_with_snapshot(2, 1);

        let challenger_balance_before = Balances::free_balance(3);
        let provider_stake_before = Providers::<Test>::get(2).unwrap().stake;

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

        // Respond at block 100 → response_time = 100 - 1 = 99
        // Blocks 96+: challenger 50%, provider 50%
        run_to_block(100);

        assert_ok!(StorageProvider::respond_to_challenge(
            RuntimeOrigin::signed(2),
            challenge_id,
            crate::ChallengeResponse::Superseded,
        ));

        // challenger_cost = 50, provider_cost = 50
        assert_eq!(Balances::free_balance(3), challenger_balance_before - 50);
        let provider_stake_after = Providers::<Test>::get(2).unwrap().stake;
        assert_eq!(provider_stake_after, provider_stake_before - 50);
    });
}

#[test]
fn challenge_slashes_multiple_challenges_on_finalize() {
    new_test_ext().execute_with(|| {
        frame_system::Pallet::<Test>::set_block_number(1);

        // Setup two providers with agreements
        register_provider(2, 200);
        register_provider(3, 200);
        let bucket_id = setup_agreement(2, 1, 50, 200);

        // Add second provider to same bucket (establish_storage_agreement always
        // creates a fresh single-primary bucket, so the shape is synthesized).
        add_primary_to_bucket(3, 1, bucket_id, 50);

        // Insert snapshot where both providers signed
        Buckets::<Test>::mutate(bucket_id, |maybe_bucket| {
            if let Some(bucket) = maybe_bucket {
                bucket.snapshot = Some(BucketSnapshot {
                    mmr_root: H256::repeat_byte(0xAB),
                    start_seq: 0,
                    leaf_count: 10,
                    checkpoint_block: 1,
                    primary_signers: vec![0x03], // bits 0 and 1 set
                });
            }
        });

        // Challenge both providers — both expire at same deadline
        assert_ok!(StorageProvider::challenge_checkpoint(
            RuntimeOrigin::signed(4),
            bucket_id,
            2,
            0,
            0,
        ));
        assert_ok!(StorageProvider::challenge_checkpoint(
            RuntimeOrigin::signed(5),
            bucket_id,
            3,
            0,
            0,
        ));

        // Both challenges at deadline 101
        let challenges = Challenges::<Test>::get(101).unwrap();
        assert_eq!(challenges.len(), 2);

        // Advance past deadline — run_to_block(102) finalises block 101
        run_to_block(102);

        // Both providers should be slashed
        let provider2 = Providers::<Test>::get(2).unwrap();
        assert_eq!(provider2.stake, 0);
        assert_eq!(provider2.stats.challenges_failed, 1);

        let provider3 = Providers::<Test>::get(3).unwrap();
        assert_eq!(provider3.stake, 0);
        assert_eq!(provider3.stats.challenges_failed, 1);

        // Challenges should be removed
        assert!(Challenges::<Test>::get(101).is_none());
    });
}

#[test]
fn challenge_slashes_emits_event_and_rewards_challenger() {
    new_test_ext().execute_with(|| {
        frame_system::Pallet::<Test>::set_block_number(1);
        let bucket_id = setup_with_snapshot(2, 1);

        let provider_stake = Providers::<Test>::get(2).unwrap().stake;
        let challenger_balance_before = Balances::free_balance(3);

        assert_ok!(StorageProvider::challenge_checkpoint(
            RuntimeOrigin::signed(3),
            bucket_id,
            2,
            0,
            0,
        ));

        // Challenger deposit (100) was reserved
        assert_eq!(Balances::free_balance(3), challenger_balance_before - 100);

        // run_to_block(102) finalises block 101, triggering slash
        run_to_block(102);

        // Challenger gets deposit back + 10% of slashed amount
        let challenger_reward = provider_stake / 10;
        assert_eq!(
            Balances::free_balance(3),
            challenger_balance_before + challenger_reward
        );

        // Verify ChallengeSlashed event
        let expected_event = RuntimeEvent::StorageProvider(crate::Event::ChallengeSlashed {
            challenge_id: ChallengeId {
                deadline: 101,
                index: 0,
            },
            provider: 2,
            slashed_amount: provider_stake,
            challenger_reward,
        });
        assert!(frame_system::Pallet::<Test>::events()
            .iter()
            .any(|r| r.event == expected_event));
    });
}

#[test]
fn respond_to_challenge_superseded_emits_defended_event() {
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

        // Respond at block 2 → response_time = 1
        run_to_block(2);

        assert_ok!(StorageProvider::respond_to_challenge(
            RuntimeOrigin::signed(2),
            challenge_id,
            crate::ChallengeResponse::Superseded,
        ));

        // Verify ChallengeDefended event
        let expected_event = RuntimeEvent::StorageProvider(crate::Event::ChallengeDefended {
            challenge_id,
            provider: 2,
            response_time_blocks: 1,
            challenger_cost: 90,
            provider_cost: 10,
        });
        assert!(frame_system::Pallet::<Test>::events()
            .iter()
            .any(|r| r.event == expected_event));
    });
}

// ─────────────────────────────────────────────────────────────────────────
// Challenge deposit escalation tests
// ─────────────────────────────────────────────────────────────────────────

#[test]
fn challenge_deposit_escalates_with_active() {
    new_test_ext().execute_with(|| {
        frame_system::Pallet::<Test>::set_block_number(1);
        let bucket_id = setup_with_snapshot(2, 1);

        let bal_before = Balances::free_balance(3);

        // First challenge: deposit = 100 * 2^0 = 100
        assert_ok!(StorageProvider::challenge_checkpoint(
            RuntimeOrigin::signed(3),
            bucket_id,
            2,
            0,
            0,
        ));
        assert_eq!(Balances::free_balance(3), bal_before - 100);

        // Second challenge (same challenger → provider, active=1): deposit = 100 * 2^1 = 200
        assert_ok!(StorageProvider::challenge_checkpoint(
            RuntimeOrigin::signed(3),
            bucket_id,
            2,
            1, // different leaf_index
            0,
        ));
        assert_eq!(Balances::free_balance(3), bal_before - 100 - 200);
    });
}

#[test]
fn challenge_deposit_escalates_after_defense() {
    new_test_ext().execute_with(|| {
        frame_system::Pallet::<Test>::set_block_number(1);
        let bucket_id = setup_with_snapshot(2, 1);

        // First challenge: deposit = 100
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

        // Provider defends (Superseded)
        run_to_block(2);
        assert_ok!(StorageProvider::respond_to_challenge(
            RuntimeOrigin::signed(2),
            challenge_id,
            crate::ChallengeResponse::Superseded,
        ));

        // active=0, failed=1 → next deposit = 100 * 2^1 = 200
        let bal_after_defense = Balances::free_balance(3);
        assert_ok!(StorageProvider::challenge_checkpoint(
            RuntimeOrigin::signed(3),
            bucket_id,
            2,
            0,
            0,
        ));
        assert_eq!(Balances::free_balance(3), bal_after_defense - 200);
    });
}

#[test]
fn challenge_deposit_resets_on_slash() {
    new_test_ext().execute_with(|| {
        frame_system::Pallet::<Test>::set_block_number(1);
        let bucket_id = setup_with_snapshot(2, 1);

        // First challenge: deposit = 100
        assert_ok!(StorageProvider::challenge_checkpoint(
            RuntimeOrigin::signed(3),
            bucket_id,
            2,
            0,
            0,
        ));

        // Provider fails to respond → slashed at deadline
        run_to_block(102);

        // History should be cleared
        assert_eq!(
            crate::ChallengeHistory::<Test>::get(3, 2),
            crate::ChallengerRecord::default()
        );

        // Need a new provider+agreement since provider 2 was slashed
        register_provider(5, 200);
        let bucket_id2 = setup_agreement(5, 1, 50, 200);
        Buckets::<Test>::mutate(bucket_id2, |maybe_bucket| {
            if let Some(bucket) = maybe_bucket {
                bucket.snapshot = Some(BucketSnapshot {
                    mmr_root: H256::repeat_byte(0xCD),
                    start_seq: 0,
                    leaf_count: 10,
                    checkpoint_block: 102,
                    primary_signers: vec![0x01],
                });
            }
        });

        // Challenge against a fresh provider starts at base again
        let bal_before = Balances::free_balance(3);
        assert_ok!(StorageProvider::challenge_checkpoint(
            RuntimeOrigin::signed(3),
            bucket_id2,
            5,
            0,
            0,
        ));
        assert_eq!(Balances::free_balance(3), bal_before - 100);
    });
}

#[test]
fn challenge_deposit_capped_at_256x() {
    new_test_ext().execute_with(|| {
        frame_system::Pallet::<Test>::set_block_number(1);
        let bucket_id = setup_with_snapshot(2, 1);

        // Give challenger enough balance for the escalated deposit
        let _ = <Balances as frame_support::traits::Currency<u64>>::deposit_creating(&3, 100_000);

        // Artificially set failed to 9 (exponent 9 → capped at 8 → 256×)
        crate::ChallengeHistory::<Test>::insert(
            3u64,
            2u64,
            crate::ChallengerRecord {
                active: 0,
                failed: 9,
            },
        );

        let bal_before = Balances::free_balance(3);
        assert_ok!(StorageProvider::challenge_checkpoint(
            RuntimeOrigin::signed(3),
            bucket_id,
            2,
            0,
            0,
        ));
        // 100 * 2^min(9, 8) = 100 * 256 = 25600
        assert_eq!(Balances::free_balance(3), bal_before - 25_600);
    });
}

#[test]
fn challenge_history_cleaned_up_on_slash() {
    new_test_ext().execute_with(|| {
        frame_system::Pallet::<Test>::set_block_number(1);
        let bucket_id = setup_with_snapshot(2, 1);

        // Create challenge
        assert_ok!(StorageProvider::challenge_checkpoint(
            RuntimeOrigin::signed(3),
            bucket_id,
            2,
            0,
            0,
        ));

        // Verify history exists
        let record = crate::ChallengeHistory::<Test>::get(3, 2);
        assert_eq!(record.active, 1);

        // Provider fails → slashed
        run_to_block(102);

        // History entry should be fully removed
        assert!(!crate::ChallengeHistory::<Test>::contains_key(3, 2));
    });
}
