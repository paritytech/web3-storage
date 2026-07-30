// SPDX-License-Identifier: Apache-2.0

use super::*;
use sp_core::H256;
use storage_primitives::{BucketSnapshot, ChallengeId, ChunkLocation, Commitment, Role};

/// Setup: register provider, create agreement, and insert a snapshot with provider signed.
pub(super) fn setup_with_snapshot(provider: u64, client: u64) -> u64 {
    register_provider(provider, 200);
    let bucket_id = setup_agreement(provider, client, 50, 200);

    // Insert a snapshot where the provider has signed
    Buckets::<Test>::mutate(bucket_id, |maybe_bucket| {
        if let Some(bucket) = maybe_bucket {
            bucket.snapshot = Some(BucketSnapshot {
                commitment: Commitment {
                    mmr_root: H256::repeat_byte(0xAB),
                    start_seq: 0,
                    leaf_count: 10,
                },
                checkpoint_block: 1,
                primary_signers: vec![0x01], // bit 0 set = provider at index 0 signed
                commitment_nonce: 0,
            });
        }
    });

    bucket_id
}

/// Replace a bucket's snapshot with a NEW canonical root (still covering seq
/// 0..10) so a previously created challenge — bound to the old `0xAB` root — is
/// genuinely superseded. Required for a `Superseded` defense to be valid under
/// the tightened soundness rule (challenged root must differ from the live
/// canonical root, and the challenged seq must still be in range).
pub(super) fn advance_snapshot_root(bucket_id: u64) {
    Buckets::<Test>::mutate(bucket_id, |maybe_bucket| {
        let bucket = maybe_bucket.as_mut().expect("bucket exists");
        bucket.snapshot = Some(BucketSnapshot {
            commitment: Commitment {
                mmr_root: H256::repeat_byte(0xCD),
                start_seq: 0,
                leaf_count: 10,
            },
            checkpoint_block: 1,
            primary_signers: vec![0x01],
            commitment_nonce: 1,
        });
    });
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
            ChunkLocation {
                leaf_index: 0,
                chunk_index: 0,
            },
        ));

        // Challenge deposit (100) should be reserved
        assert_eq!(Balances::free_balance(3), challenger_balance_before - 100);

        // Challenge should exist at deadline = current_block(1) + ChallengeTimeout(100) = 101,
        // at stable index 0 (first challenge allocated for this deadline).
        let challenge = Challenges::<Test>::get(101, 0).unwrap();
        assert_eq!(challenge.provider, 2);
        assert_eq!(challenge.challenger, 3);
        assert_eq!(Challenges::<Test>::iter_prefix(101).count(), 1);
    });
}

#[test]
fn challenge_checkpoint_fails_expired_agreement() {
    new_test_ext().execute_with(|| {
        frame_system::Pallet::<Test>::set_block_number(1);
        // Agreement opened at block 1 with duration 200 → expires_at == 201.
        let bucket_id = setup_with_snapshot(2, 1);

        // Past the agreement's expiry: challengeability tracks genuine
        // obligation, so an expired (unswept) checkpoint can no longer be
        // challenged.
        frame_system::Pallet::<Test>::set_block_number(202);
        assert_noop!(
            StorageProvider::challenge_checkpoint(
                RuntimeOrigin::signed(3),
                bucket_id,
                2,
                ChunkLocation {
                    leaf_index: 0,
                    chunk_index: 0
                }
            ),
            Error::<Test>::AgreementExpired
        );
    });
}

#[test]
fn challenge_checkpoint_fails_no_snapshot() {
    new_test_ext().execute_with(|| {
        register_provider(2, 200);
        let bucket_id = setup_agreement(2, 1, 50, 200);

        // No snapshot inserted
        assert_noop!(
            StorageProvider::challenge_checkpoint(
                RuntimeOrigin::signed(3),
                bucket_id,
                2,
                ChunkLocation {
                    leaf_index: 0,
                    chunk_index: 0
                }
            ),
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
                    commitment: Commitment {
                        mmr_root: H256::repeat_byte(0xAB),
                        start_seq: 0,
                        leaf_count: 10,
                    },
                    checkpoint_block: 1,
                    primary_signers: vec![0x01], // only bit 0 set
                    commitment_nonce: 0,
                });
            }
        });

        // Challenge provider 3 (at index 1, not signed) should fail
        assert_noop!(
            StorageProvider::challenge_checkpoint(
                RuntimeOrigin::signed(4),
                bucket_id,
                3,
                ChunkLocation {
                    leaf_index: 0,
                    chunk_index: 0
                }
            ),
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
                Commitment {
                    mmr_root: H256::repeat_byte(0xAB),
                    start_seq: 0,
                    leaf_count: 0,
                },
                ChunkLocation {
                    leaf_index: 0,
                    chunk_index: 0,
                },
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
            StorageProvider::challenge_replica(
                RuntimeOrigin::signed(3),
                bucket_id,
                2,
                ChunkLocation {
                    leaf_index: 0,
                    chunk_index: 0
                }
            ),
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
            ChunkLocation {
                leaf_index: 0,
                chunk_index: 0,
            },
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
            ChunkLocation {
                leaf_index: 0,
                chunk_index: 0,
            },
        ));

        let challenge_id = ChallengeId {
            deadline: 101,
            index: 0,
        };

        // Advance the canonical snapshot to a NEW root so the challenged
        // commitment (root 0xAB) is genuinely superseded. The challenged seq 0
        // still sits inside the new canonical range 0..10, so `Superseded` is a
        // valid defense.
        Buckets::<Test>::mutate(bucket_id, |maybe_bucket| {
            let bucket = maybe_bucket.as_mut().unwrap();
            bucket.snapshot = Some(BucketSnapshot {
                commitment: Commitment {
                    mmr_root: H256::repeat_byte(0xCD),
                    start_seq: 0,
                    leaf_count: 10,
                },
                checkpoint_block: 1,
                primary_signers: vec![0x01],
                commitment_nonce: 1,
            });
        });

        assert_ok!(StorageProvider::respond_to_challenge(
            RuntimeOrigin::signed(2),
            challenge_id,
            crate::ChallengeResponse::Superseded,
        ));

        // Challenge should be removed
        assert!(Challenges::<Test>::get(101, 0).is_none());
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
            ChunkLocation {
                leaf_index: 0,
                chunk_index: 0,
            },
        ));

        // Challenge deadline = block 1 + ChallengeTimeout(100) = 101
        // run_to_block(102): the on_initialize sweep at 102 covers deadline 101
        run_to_block(102);

        // Provider should be slashed
        let provider = Providers::<Test>::get(2).unwrap();
        assert!(provider.stake < provider_stake_before);
        assert_eq!(provider.stats.challenges_failed, 1);
    });
}

// NOTE: dev's `respond_to_challenge_superseded_fails_leaf_beyond_canonical`
// was dropped here. PR #125 changed an out-of-range `Superseded` claim from
// erroring with `LeafBeyondCanonical` to slashing the provider immediately.
// The equivalent merged-behavior test is
// `challenge_tests::respond_with_bogus_superseded_claim_slashes_immediately`.

#[test]
fn respond_to_challenge_superseded_cost_split_block_1() {
    new_test_ext().execute_with(|| {
        frame_system::Pallet::<Test>::set_block_number(1);
        let bucket_id = setup_with_snapshot(2, 1);

        // Authorize the challenger (Reader member): the split table applies
        // only to the authorized tier — a public stranger pays 100%.
        assert_ok!(StorageProvider::set_member(
            RuntimeOrigin::signed(1),
            bucket_id,
            3,
            Role::Reader,
        ));

        // Treasury account is 999 in the mock (TestTreasury).
        const TREASURY: u64 = 999;

        let challenger_balance_before = Balances::free_balance(3);
        let provider_balance_before = Balances::free_balance(2);
        let provider_stake_before = Providers::<Test>::get(2).unwrap().stake;
        let treasury_balance_before = Balances::free_balance(TREASURY);
        let total_issuance_before = Balances::total_issuance();

        assert_ok!(StorageProvider::challenge_checkpoint(
            RuntimeOrigin::signed(3),
            bucket_id,
            2,
            ChunkLocation {
                leaf_index: 0,
                chunk_index: 0,
            },
        ));

        let challenge_id = ChallengeId {
            deadline: 101,
            index: 0,
        };

        // Respond at block 2 → response_time = 2 - (101-100) = 2 - 1 = 1
        run_to_block(2);

        advance_snapshot_root(bucket_id);

        assert_ok!(StorageProvider::respond_to_challenge(
            RuntimeOrigin::signed(2),
            challenge_id,
            crate::ChallengeResponse::Superseded,
        ));

        // Block 1: challenger 90%, provider 10%
        // deposit = 100, challenger_cost = 90, provider_cost = 10
        // Challenger gets unreserved (100 - 90) = 10 back
        assert_eq!(Balances::free_balance(3), challenger_balance_before - 90);

        // No deposit stays stuck in the challenger's reserved balance.
        assert_eq!(Balances::reserved_balance(3), 0);

        // The forfeited challenger_cost (90) is repatriated to the provider's
        // free balance as compensation for responding.
        assert_eq!(Balances::free_balance(2), provider_balance_before + 90);

        // A valid response never touches the provider's stake: the provider
        // bears its 10% share by not being reimbursed for it.
        let provider_stake_after = Providers::<Test>::get(2).unwrap().stake;
        assert_eq!(provider_stake_after, provider_stake_before);

        // Nothing flows to the Treasury on a defense.
        assert_eq!(Balances::free_balance(TREASURY), treasury_balance_before);

        // The repatriated challenger_cost is an internal transfer: total
        // issuance is unchanged.
        assert_eq!(Balances::total_issuance(), total_issuance_before);

        // The defense is counted, at resolution, under the authorized tier.
        let stats = Providers::<Test>::get(2).unwrap().stats;
        assert_eq!(stats.challenges_received_authorized, 1);
        assert_eq!(stats.challenges_received_public, 0);
    });
}

#[test]
fn respond_to_challenge_superseded_cost_split_block_5() {
    new_test_ext().execute_with(|| {
        frame_system::Pallet::<Test>::set_block_number(1);
        let bucket_id = setup_with_snapshot(2, 1);

        // Authorize the challenger (Reader member): the split table applies
        // only to the authorized tier — a public stranger pays 100%.
        assert_ok!(StorageProvider::set_member(
            RuntimeOrigin::signed(1),
            bucket_id,
            3,
            Role::Reader,
        ));

        let challenger_balance_before = Balances::free_balance(3);
        let provider_stake_before = Providers::<Test>::get(2).unwrap().stake;

        assert_ok!(StorageProvider::challenge_checkpoint(
            RuntimeOrigin::signed(3),
            bucket_id,
            2,
            ChunkLocation {
                leaf_index: 0,
                chunk_index: 0,
            },
        ));

        let challenge_id = ChallengeId {
            deadline: 101,
            index: 0,
        };

        // Respond at block 6 → response_time = 6 - 1 = 5
        // Blocks 2-5: challenger 80%, provider 20%
        run_to_block(6);

        advance_snapshot_root(bucket_id);

        assert_ok!(StorageProvider::respond_to_challenge(
            RuntimeOrigin::signed(2),
            challenge_id,
            crate::ChallengeResponse::Superseded,
        ));

        // challenger_cost = 80, provider_cost = 20
        assert_eq!(Balances::free_balance(3), challenger_balance_before - 80);
        // Stake untouched: the provider bears its share by not being
        // reimbursed for it.
        let provider_stake_after = Providers::<Test>::get(2).unwrap().stake;
        assert_eq!(provider_stake_after, provider_stake_before);
    });
}

#[test]
fn respond_to_challenge_superseded_cost_split_block_24() {
    new_test_ext().execute_with(|| {
        frame_system::Pallet::<Test>::set_block_number(1);
        let bucket_id = setup_with_snapshot(2, 1);

        // Authorize the challenger (Reader member): the split table applies
        // only to the authorized tier — a public stranger pays 100%.
        assert_ok!(StorageProvider::set_member(
            RuntimeOrigin::signed(1),
            bucket_id,
            3,
            Role::Reader,
        ));

        let challenger_balance_before = Balances::free_balance(3);
        let provider_stake_before = Providers::<Test>::get(2).unwrap().stake;

        assert_ok!(StorageProvider::challenge_checkpoint(
            RuntimeOrigin::signed(3),
            bucket_id,
            2,
            ChunkLocation {
                leaf_index: 0,
                chunk_index: 0,
            },
        ));

        let challenge_id = ChallengeId {
            deadline: 101,
            index: 0,
        };

        // Respond at block 25 → response_time = 25 - 1 = 24
        // Blocks 6-24: challenger 70%, provider 30%
        run_to_block(25);

        advance_snapshot_root(bucket_id);

        assert_ok!(StorageProvider::respond_to_challenge(
            RuntimeOrigin::signed(2),
            challenge_id,
            crate::ChallengeResponse::Superseded,
        ));

        // challenger_cost = 70, provider_cost = 30
        assert_eq!(Balances::free_balance(3), challenger_balance_before - 70);
        // Stake untouched: the provider bears its share by not being
        // reimbursed for it.
        let provider_stake_after = Providers::<Test>::get(2).unwrap().stake;
        assert_eq!(provider_stake_after, provider_stake_before);
    });
}

#[test]
fn respond_to_challenge_superseded_cost_split_block_95() {
    new_test_ext().execute_with(|| {
        frame_system::Pallet::<Test>::set_block_number(1);
        let bucket_id = setup_with_snapshot(2, 1);

        // Authorize the challenger (Reader member): the split table applies
        // only to the authorized tier — a public stranger pays 100%.
        assert_ok!(StorageProvider::set_member(
            RuntimeOrigin::signed(1),
            bucket_id,
            3,
            Role::Reader,
        ));

        let challenger_balance_before = Balances::free_balance(3);
        let provider_stake_before = Providers::<Test>::get(2).unwrap().stake;

        assert_ok!(StorageProvider::challenge_checkpoint(
            RuntimeOrigin::signed(3),
            bucket_id,
            2,
            ChunkLocation {
                leaf_index: 0,
                chunk_index: 0,
            },
        ));

        let challenge_id = ChallengeId {
            deadline: 101,
            index: 0,
        };

        // Respond at block 96 → response_time = 96 - 1 = 95
        // Blocks 25-95: challenger 60%, provider 40%
        run_to_block(96);

        advance_snapshot_root(bucket_id);

        assert_ok!(StorageProvider::respond_to_challenge(
            RuntimeOrigin::signed(2),
            challenge_id,
            crate::ChallengeResponse::Superseded,
        ));

        // challenger_cost = 60, provider_cost = 40
        assert_eq!(Balances::free_balance(3), challenger_balance_before - 60);
        // Stake untouched: the provider bears its share by not being
        // reimbursed for it.
        let provider_stake_after = Providers::<Test>::get(2).unwrap().stake;
        assert_eq!(provider_stake_after, provider_stake_before);
    });
}

#[test]
fn respond_to_challenge_superseded_cost_split_block_96_plus() {
    new_test_ext().execute_with(|| {
        frame_system::Pallet::<Test>::set_block_number(1);
        let bucket_id = setup_with_snapshot(2, 1);

        // Authorize the challenger (Reader member): the split table applies
        // only to the authorized tier — a public stranger pays 100%.
        assert_ok!(StorageProvider::set_member(
            RuntimeOrigin::signed(1),
            bucket_id,
            3,
            Role::Reader,
        ));

        let challenger_balance_before = Balances::free_balance(3);
        let provider_balance_before = Balances::free_balance(2);
        let provider_stake_before = Providers::<Test>::get(2).unwrap().stake;

        assert_ok!(StorageProvider::challenge_checkpoint(
            RuntimeOrigin::signed(3),
            bucket_id,
            2,
            ChunkLocation {
                leaf_index: 0,
                chunk_index: 0,
            },
        ));

        let challenge_id = ChallengeId {
            deadline: 101,
            index: 0,
        };

        // Respond at block 100 → response_time = 100 - 1 = 99
        // Blocks 96+: challenger 50%, provider 50%
        run_to_block(100);

        advance_snapshot_root(bucket_id);

        assert_ok!(StorageProvider::respond_to_challenge(
            RuntimeOrigin::signed(2),
            challenge_id,
            crate::ChallengeResponse::Superseded,
        ));

        // challenger_cost = 50, provider_cost = 50
        assert_eq!(Balances::free_balance(3), challenger_balance_before - 50);

        // No deposit stays stuck in the challenger's reserved balance.
        assert_eq!(Balances::reserved_balance(3), 0);

        // The forfeited challenger_cost (50) is repatriated to the provider's
        // free balance as compensation for responding.
        assert_eq!(Balances::free_balance(2), provider_balance_before + 50);

        // Stake untouched: the provider bears its share by not being
        // reimbursed for it.
        let provider_stake_after = Providers::<Test>::get(2).unwrap().stake;
        assert_eq!(provider_stake_after, provider_stake_before);
    });
}

#[test]
fn challenge_slashes_multiple_challenges_in_sweep() {
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
                    commitment: Commitment {
                        mmr_root: H256::repeat_byte(0xAB),
                        start_seq: 0,
                        leaf_count: 10,
                    },
                    checkpoint_block: 1,
                    primary_signers: vec![0x03], // bits 0 and 1 set
                    commitment_nonce: 0,
                });
            }
        });

        // Challenge both providers — both expire at same deadline
        assert_ok!(StorageProvider::challenge_checkpoint(
            RuntimeOrigin::signed(4),
            bucket_id,
            2,
            ChunkLocation {
                leaf_index: 0,
                chunk_index: 0,
            },
        ));
        assert_ok!(StorageProvider::challenge_checkpoint(
            RuntimeOrigin::signed(5),
            bucket_id,
            3,
            ChunkLocation {
                leaf_index: 0,
                chunk_index: 0,
            },
        ));

        // Both challenges at deadline 101, at stable indices 0 and 1.
        assert_eq!(Challenges::<Test>::iter_prefix(101).count(), 2);
        assert!(Challenges::<Test>::get(101, 0).is_some());
        assert!(Challenges::<Test>::get(101, 1).is_some());

        // Advance past deadline — the sweep at block 102 covers deadline 101
        run_to_block(102);

        // Both providers should be slashed
        let provider2 = Providers::<Test>::get(2).unwrap();
        assert_eq!(provider2.stake, 0);
        assert_eq!(provider2.stats.challenges_failed, 1);

        let provider3 = Providers::<Test>::get(3).unwrap();
        assert_eq!(provider3.stake, 0);
        assert_eq!(provider3.stats.challenges_failed, 1);

        // Challenges should be removed and the index allocator cleared.
        assert_eq!(Challenges::<Test>::iter_prefix(101).count(), 0);
        assert_eq!(NextChallengeIndex::<Test>::get(101), 0);
    });
}

/// Regression for the Vec-shift bug: two challenges sharing a deadline must
/// keep their original `ChallengeId.index` after a sibling is resolved.
/// Responding to the first sibling must NOT make the second one
/// unaddressable or shift it to a different index.
#[test]
fn responding_to_sibling_preserves_other_challenge_index() {
    new_test_ext().execute_with(|| {
        frame_system::Pallet::<Test>::set_block_number(1);

        register_provider(2, 200);
        register_provider(3, 200);
        let bucket_id = setup_agreement(2, 1, 50, 200);
        add_primary_to_bucket(3, 1, bucket_id, 50);

        // Snapshot signed by both providers (bits 0 and 1).
        Buckets::<Test>::mutate(bucket_id, |maybe_bucket| {
            if let Some(bucket) = maybe_bucket {
                bucket.snapshot = Some(BucketSnapshot {
                    commitment: Commitment {
                        mmr_root: H256::repeat_byte(0xAB),
                        start_seq: 0,
                        leaf_count: 10,
                    },
                    checkpoint_block: 1,
                    primary_signers: vec![0x03],
                    commitment_nonce: 0,
                });
            }
        });

        // Two challenges at the SAME deadline (101): index 0 -> provider 2,
        // index 1 -> provider 3.
        assert_ok!(StorageProvider::challenge_checkpoint(
            RuntimeOrigin::signed(4),
            bucket_id,
            2,
            ChunkLocation {
                leaf_index: 0,
                chunk_index: 0,
            },
        ));
        assert_ok!(StorageProvider::challenge_checkpoint(
            RuntimeOrigin::signed(5),
            bucket_id,
            3,
            ChunkLocation {
                leaf_index: 0,
                chunk_index: 0,
            },
        ));
        assert_eq!(NextChallengeIndex::<Test>::get(101), 2);
        assert_eq!(Challenges::<Test>::get(101, 0).unwrap().provider, 2);
        assert_eq!(Challenges::<Test>::get(101, 1).unwrap().provider, 3);

        // Supersede the challenged root (0xAB) with a new canonical snapshot so
        // a `Superseded` defense is valid for both challenges.
        Buckets::<Test>::mutate(bucket_id, |maybe_bucket| {
            let bucket = maybe_bucket.as_mut().unwrap();
            bucket.snapshot = Some(BucketSnapshot {
                commitment: Commitment {
                    mmr_root: H256::repeat_byte(0xCD),
                    start_seq: 0,
                    leaf_count: 10,
                },
                checkpoint_block: 1,
                primary_signers: vec![0x03],
                commitment_nonce: 1,
            });
        });

        // Respond to the FIRST sibling (index 0, provider 2).
        assert_ok!(StorageProvider::respond_to_challenge(
            RuntimeOrigin::signed(2),
            ChallengeId {
                deadline: 101,
                index: 0,
            },
            crate::ChallengeResponse::Superseded,
        ));

        // Index 0 is gone; the OTHER challenge is still at its ORIGINAL index 1
        // (the Vec layout would have shifted it down to 0 and broken this).
        assert!(Challenges::<Test>::get(101, 0).is_none());
        let sibling =
            Challenges::<Test>::get(101, 1).expect("sibling still addressable at index 1");
        assert_eq!(sibling.provider, 3);

        // The index allocator is unaffected by removal — indices are never
        // reused, so the next allocation would still be 2.
        assert_eq!(NextChallengeIndex::<Test>::get(101), 2);

        // The sibling resolves correctly by its original `ChallengeId`.
        assert_ok!(StorageProvider::respond_to_challenge(
            RuntimeOrigin::signed(3),
            ChallengeId {
                deadline: 101,
                index: 1,
            },
            crate::ChallengeResponse::Superseded,
        ));
        assert!(Challenges::<Test>::get(101, 1).is_none());
        assert_eq!(Challenges::<Test>::iter_prefix(101).count(), 0);
        // Allocator still untouched by responses (only the sweep clears it).
        assert_eq!(NextChallengeIndex::<Test>::get(101), 2);
    });
}

#[test]
fn challenge_slashes_routes_full_slash_to_treasury() {
    new_test_ext().execute_with(|| {
        frame_system::Pallet::<Test>::set_block_number(1);
        let bucket_id = setup_with_snapshot(2, 1);

        // Treasury account is 999 in the mock (TestTreasury).
        const TREASURY: u64 = 999;

        let provider_stake = Providers::<Test>::get(2).unwrap().stake;
        let challenger_balance_before = Balances::free_balance(3);
        let treasury_balance_before = Balances::free_balance(TREASURY);
        let total_issuance_before = Balances::total_issuance();

        assert_ok!(StorageProvider::challenge_checkpoint(
            RuntimeOrigin::signed(3),
            bucket_id,
            2,
            ChunkLocation {
                leaf_index: 0,
                chunk_index: 0,
            },
        ));

        // Challenger deposit (100) was reserved
        assert_eq!(Balances::free_balance(3), challenger_balance_before - 100);

        // run_to_block(102): the sweep covers deadline 101, triggering the slash
        run_to_block(102);

        // Per the design the challenger receives NO reward — only their
        // deposit back — so their free balance returns to where it started.
        assert_eq!(Balances::free_balance(3), challenger_balance_before);

        // The entire slashed amount is routed to the Treasury (not burned,
        // not shared with the challenger).
        assert_eq!(
            Balances::free_balance(TREASURY),
            treasury_balance_before + provider_stake
        );

        // Slash + resolve is net-zero: total issuance is unchanged (no mint,
        // no burn — the slash is moved to the Treasury, not destroyed).
        assert_eq!(Balances::total_issuance(), total_issuance_before);

        // Verify ChallengeSlashed event (challenger_reward is always zero).
        let expected_event = RuntimeEvent::StorageProvider(crate::Event::ChallengeSlashed {
            challenge_id: ChallengeId {
                deadline: 101,
                index: 0,
            },
            provider: 2,
            slashed_amount: provider_stake,
            challenger_reward: 0,
            reason: storage_primitives::SlashReason::Timeout,
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
            ChunkLocation {
                leaf_index: 0,
                chunk_index: 0,
            },
        ));

        let challenge_id = ChallengeId {
            deadline: 101,
            index: 0,
        };

        // Respond at block 2 → response_time = 1
        run_to_block(2);

        // Advance the canonical snapshot to a NEW root so the challenged
        // commitment (root 0xAB) is genuinely superseded — the precondition for
        // a valid `Superseded` defense. Challenged seq 0 remains inside the new
        // canonical range 0..10.
        Buckets::<Test>::mutate(bucket_id, |maybe_bucket| {
            let bucket = maybe_bucket.as_mut().unwrap();
            bucket.snapshot = Some(BucketSnapshot {
                commitment: Commitment {
                    mmr_root: H256::repeat_byte(0xCD),
                    start_seq: 0,
                    leaf_count: 10,
                },
                checkpoint_block: 1,
                primary_signers: vec![0x01],
                commitment_nonce: 1,
            });
        });

        assert_ok!(StorageProvider::respond_to_challenge(
            RuntimeOrigin::signed(2),
            challenge_id,
            crate::ChallengeResponse::Superseded,
        ));

        // Verify ChallengeDefended event. Challenger 3 is a public stranger,
        // so the deposit reimburses the provider in full and the provider
        // bears nothing.
        let expected_event = RuntimeEvent::StorageProvider(crate::Event::ChallengeDefended {
            challenge_id,
            provider: 2,
            response_time_blocks: 1,
            challenger_cost: 100,
            provider_cost: 0,
        });
        assert!(frame_system::Pallet::<Test>::events()
            .iter()
            .any(|r| r.event == expected_event));
    });
}

// ─────────────────────────────────────────────────────────────────────────────
// Snapshot signer-bitfield re-indexing on primary-provider removal.
//
// `primary_signers` is positional: bit `i` belongs to `primary_providers[i]`.
// Removing a primary shifts every later provider down one index, so the
// bitfield must be re-indexed to match — otherwise `has_provider_signed` reads
// the wrong bit for every surviving provider after the removed one.
//
// Both removal paths (`finalize_agreement` via admin early-termination, and
// `remove_slashed`) build a bucket with primaries [A, B, C] = [2, 3, 4] where
// only A and C signed (bits {0, 2}). After A is removed the ordering becomes
// [B, C] and the bitfield must report B (now index 0) as not-signed and C (now
// index 1) as signed.
// ─────────────────────────────────────────────────────────────────────────────

/// Build a bucket whose primaries are [2, 3, 4] with a snapshot in which 2 and
/// 4 signed (bits {0, 2}) but 3 did not. Returns the bucket id.
fn setup_three_primaries_snapshot() -> u64 {
    register_provider(2, 200);
    register_provider(3, 200);
    register_provider(4, 200);
    let bucket_id = setup_agreement(2, 1, 50, 200);
    add_primary_to_bucket(3, 1, bucket_id, 50);
    add_primary_to_bucket(4, 1, bucket_id, 50);

    Buckets::<Test>::mutate(bucket_id, |maybe_bucket| {
        let bucket = maybe_bucket.as_mut().expect("bucket exists");
        // Sanity: ordering is exactly [2, 3, 4].
        assert_eq!(bucket.primary_providers.to_vec(), vec![2, 3, 4]);
        bucket.snapshot = Some(BucketSnapshot {
            commitment: Commitment {
                mmr_root: H256::repeat_byte(0xAB),
                start_seq: 0,
                leaf_count: 10,
            },
            checkpoint_block: 1,
            // bits {0, 2} set: provider 2 (idx 0) and 4 (idx 2) signed.
            primary_signers: vec![0b0000_0101],
            commitment_nonce: 0,
        });
    });

    bucket_id
}

/// Assert that after the lead primary (2) was removed, the stored snapshot's
/// bitfield matches the new [3, 4] ordering: 3 not-signed, 4 signed.
fn assert_reindexed_after_removing_lead(bucket_id: u64) {
    let bucket = Buckets::<Test>::get(bucket_id).unwrap();
    assert_eq!(bucket.primary_providers.to_vec(), vec![3, 4]);
    let snapshot = bucket.snapshot.expect("snapshot retained");
    // New layout: idx 0 -> old bit 1 (provider 3, unsigned) = clear;
    // idx 1 -> old bit 2 (provider 4, signed) = set => 0b0000_0010.
    assert!(
        !snapshot.has_provider_signed(0),
        "provider 3 (new index 0) must read as not-signed"
    );
    assert!(
        snapshot.has_provider_signed(1),
        "provider 4 (new index 1) must read as signed"
    );

    // And the challenge gate now resolves correctly for the new ordering:
    // provider 3 (unsigned) cannot be challenged, provider 4 (signed) can.
    assert_noop!(
        StorageProvider::challenge_checkpoint(
            RuntimeOrigin::signed(5),
            bucket_id,
            3,
            ChunkLocation {
                leaf_index: 0,
                chunk_index: 0
            }
        ),
        Error::<Test>::ProviderNotInSnapshot
    );
    assert_ok!(StorageProvider::challenge_checkpoint(
        RuntimeOrigin::signed(5),
        bucket_id,
        4,
        ChunkLocation {
            leaf_index: 0,
            chunk_index: 0,
        },
    ));
}

#[test]
fn finalize_agreement_reindexes_snapshot_bitfield() {
    use storage_primitives::EndAction;
    new_test_ext().execute_with(|| {
        frame_system::Pallet::<Test>::set_block_number(1);
        let bucket_id = setup_three_primaries_snapshot();

        // Admin (client 1) early-terminates provider 2's primary agreement,
        // which routes through `finalize_agreement` and removes 2 (index 0).
        assert_ok!(StorageProvider::end_agreement(
            RuntimeOrigin::signed(1),
            bucket_id,
            2,
            EndAction::Pay,
        ));

        assert_reindexed_after_removing_lead(bucket_id);
    });
}

#[test]
fn remove_slashed_reindexes_snapshot_bitfield() {
    new_test_ext().execute_with(|| {
        frame_system::Pallet::<Test>::set_block_number(1);
        let bucket_id = setup_three_primaries_snapshot();

        // Slash provider 2's entire reserved stake to zero, then remove it.
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

        assert_ok!(StorageProvider::remove_slashed(
            RuntimeOrigin::signed(5),
            bucket_id,
            2
        ));

        assert_reindexed_after_removing_lead(bucket_id);
    });
}

/// The per-deadline challenge count is capped by `MaxChallengesPerDeadline`
/// (5 in this mock) so the `on_initialize` slash sweep stays bounded. All
/// challenges created at the same clock reading share a deadline, so once the
/// cap is reached in block 1 the next `challenge_checkpoint` for that deadline
/// must be rejected with `TooManyChallengesThisBlock`.
#[test]
fn challenge_count_per_deadline_is_capped() {
    new_test_ext().execute_with(|| {
        use frame_support::traits::Get;
        frame_system::Pallet::<Test>::set_block_number(1);
        let bucket_id = setup_with_snapshot(2, 1);

        let cap = <Test as Config>::MaxChallengesPerDeadline::get();
        assert_eq!(cap, 5, "mock cap is sized small to keep this test cheap");

        // Fill the deadline to the cap. All land at deadline 101 (block 1 +
        // ChallengeTimeout 100) with stable indices 0..cap.
        for _ in 0..cap {
            assert_ok!(StorageProvider::challenge_checkpoint(
                RuntimeOrigin::signed(3),
                bucket_id,
                2,
                ChunkLocation {
                    leaf_index: 0,
                    chunk_index: 0,
                },
            ));
        }
        assert_eq!(Challenges::<Test>::iter_prefix(101).count(), cap as usize);
        assert_eq!(NextChallengeIndex::<Test>::get(101), cap);

        // The next challenge for the same deadline is rejected.
        assert_noop!(
            StorageProvider::challenge_checkpoint(
                RuntimeOrigin::signed(3),
                bucket_id,
                2,
                ChunkLocation {
                    leaf_index: 0,
                    chunk_index: 0
                }
            ),
            Error::<Test>::TooManyChallengesThisBlock
        );
        // The rejection left the count untouched.
        assert_eq!(NextChallengeIndex::<Test>::get(101), cap);
    });
}

// ─────────────────────────────────────────────────────────────────────────────
// PR #125 challenge-overhaul tests.
//
// Kept in a nested module so they coexist with dev's top-level challenge tests
// above without name collisions. Adaptations from the original PR #125 file:
//   * The replay-recency gate surfaces `Error::CommitmentNonceTooOld` in the
//     merged pallet (PR #125 originally named it `NonceTooOld`), so the three
//     nonce-rejection tests assert `CommitmentNonceTooOld`.
// ─────────────────────────────────────────────────────────────────────────────
mod challenge_tests {
    use super::*;
    use codec::Encode;
    use frame_support::{traits::Hooks, BoundedVec};
    use sp_core::{Pair, H256};
    use storage_primitives::{
        blake2_256, BucketSnapshot, ChallengeId, ChallengerStatRecord, ChunkLocation, Commitment,
        EndAction, MerkleProof, MmrLeaf, MmrProof, ProviderRole, ReplicaSyncRecord,
    };

    // ─────────────────────────────────────────────────────────────────────────
    // Helpers
    // ─────────────────────────────────────────────────────────────────────────

    /// Build a valid (MMR root, MMR proof, chunk merkle proof) for a single-leaf
    /// MMR containing a single chunk. With one chunk, the data_root collapses to
    /// the chunk hash and the MMR root collapses to the leaf hash.
    fn single_chunk_proof(chunk_data: &[u8]) -> (H256, MmrProof, MerkleProof) {
        let chunk_hash = blake2_256(chunk_data);
        // Single chunk → data_root is the chunk hash (no intermediate nodes).
        let data_root = chunk_hash;
        let leaf = MmrLeaf {
            data_root,
            data_size: chunk_data.len() as u64,
            total_size: chunk_data.len() as u64,
        };
        let leaf_hash = blake2_256(&leaf.encode());
        // Single leaf → MMR root is the leaf hash.
        let mmr_root = leaf_hash;
        let mmr_proof = MmrProof {
            peaks: vec![leaf_hash],
            leaf,
            leaf_proof: MerkleProof {
                siblings: vec![],
                path: vec![],
            },
        };
        let chunk_proof = MerkleProof {
            siblings: vec![],
            path: vec![],
        };
        (mmr_root, mmr_proof, chunk_proof)
    }

    fn make_chunk_bv(data: &[u8]) -> BoundedVec<u8, <Test as Config>::MaxChunkSize> {
        data.to_vec()
            .try_into()
            .expect("chunk fits in MaxChunkSize")
    }

    /// Register provider 2, create bucket 0 owned by 1 with a primary
    /// agreement, then write a snapshot for `(mmr_root, start_seq, leaf_count)`
    /// signed by provider 2. Returns nothing — state is in storage.
    fn setup_primary_with_snapshot(mmr_root: H256, start_seq: u64, leaf_count: u64) {
        // Provider 2 stakes 200; min stake is 100. The merged pallet creates
        // the bucket together with its primary agreement when redeeming
        // provider-signed terms (`setup_agreement`), rather than the old
        // request/accept extrinsic pair.
        register_provider(2, 200);
        let bucket_id = setup_agreement(2, 1, 100, 100);
        debug_assert_eq!(bucket_id, 0);

        // Inject a snapshot directly. Provider 2 is at index 0 in the primaries
        // BoundedVec (only provider in bucket), so bit 0 is set.
        let snapshot = BucketSnapshot {
            commitment: Commitment {
                mmr_root,
                start_seq,
                leaf_count,
            },
            checkpoint_block: System::block_number(),
            primary_signers: vec![0b0000_0001],
            commitment_nonce: System::block_number(),
        };
        Buckets::<Test>::mutate(0u64, |bucket| {
            let bucket = bucket.as_mut().expect("bucket exists");
            bucket.snapshot = Some(snapshot);
        });
    }

    // ─────────────────────────────────────────────────────────────────────────
    // challenge_checkpoint — challenge creation
    // ─────────────────────────────────────────────────────────────────────────

    #[test]
    fn challenge_checkpoint_creates_challenge_in_storage() {
        new_test_ext().execute_with(|| {
            System::set_block_number(1);
            let (mmr_root, _, _) = single_chunk_proof(b"chunk-0");
            setup_primary_with_snapshot(mmr_root, 0, 1);

            assert_ok!(StorageProvider::challenge_checkpoint(
                RuntimeOrigin::signed(3),
                0,
                2,
                ChunkLocation {
                    leaf_index: 0,
                    chunk_index: 0,
                },
            ));

            // Challenge stored at deadline = block(1) + ChallengeTimeout(100) = 101, index 0.
            assert_eq!(Challenges::<Test>::iter_prefix(101).count(), 1);
            let challenge = Challenges::<Test>::get(101, 0).expect("challenge present");
            assert_eq!(challenge.provider, 2);
            assert_eq!(challenge.challenger, 3);
            assert_eq!(challenge.mmr_root, mmr_root);
            // Currently a fixed `100u32`; commit 4 will change this.
            assert_eq!(challenge.deposit, 100);
            // Challenger 3 is a stranger (not a member, owns no agreement).
            assert!(!challenge.authorized);

            // Received counters move at resolution, never at creation.
            let provider = Providers::<Test>::get(2).unwrap();
            assert_eq!(provider.stats.challenges_received_authorized, 0);
            assert_eq!(provider.stats.challenges_received_public, 0);
        });
    }

    #[test]
    fn challenge_checkpoint_fails_without_snapshot() {
        new_test_ext().execute_with(|| {
            System::set_block_number(1);
            register_provider(2, 200);
            // Bucket 0 with provider 2 as primary, but no snapshot written.
            let bucket_id = setup_agreement(2, 1, 100, 100);

            assert_noop!(
                StorageProvider::challenge_checkpoint(
                    RuntimeOrigin::signed(3),
                    bucket_id,
                    2,
                    ChunkLocation {
                        leaf_index: 0,
                        chunk_index: 0
                    }
                ),
                Error::<Test>::NoSnapshot
            );
        });
    }

    #[test]
    fn challenge_checkpoint_fails_if_provider_not_in_primaries() {
        new_test_ext().execute_with(|| {
            System::set_block_number(1);
            let (mmr_root, _, _) = single_chunk_proof(b"chunk-0");
            setup_primary_with_snapshot(mmr_root, 0, 1);

            // Provider 5 is not registered / not in the bucket.
            assert_noop!(
                StorageProvider::challenge_checkpoint(
                    RuntimeOrigin::signed(3),
                    0,
                    5,
                    ChunkLocation {
                        leaf_index: 0,
                        chunk_index: 0
                    }
                ),
                Error::<Test>::ProviderNotInSnapshot
            );
        });
    }

    // ─────────────────────────────────────────────────────────────────────────
    // respond_to_challenge — happy paths and rejections
    // ─────────────────────────────────────────────────────────────────────────

    #[test]
    fn respond_with_valid_proof_defends_challenge() {
        new_test_ext().execute_with(|| {
            System::set_block_number(1);
            let chunk_data = b"chunk-0".to_vec();
            let (mmr_root, mmr_proof, chunk_proof) = single_chunk_proof(&chunk_data);
            setup_primary_with_snapshot(mmr_root, 0, 1);

            assert_ok!(StorageProvider::challenge_checkpoint(
                RuntimeOrigin::signed(3),
                0,
                2,
                ChunkLocation {
                    leaf_index: 0,
                    chunk_index: 0,
                },
            ));
            let challenge_id = ChallengeId {
                deadline: 101u64,
                index: 0u16,
            };

            assert_ok!(StorageProvider::respond_to_challenge(
                RuntimeOrigin::signed(2),
                challenge_id,
                ChallengeResponse::Proof {
                    chunk_data: make_chunk_bv(&chunk_data),
                    mmr_proof,
                    chunk_proof,
                },
            ));

            // Challenge cleared from storage.
            assert!(Challenges::<Test>::get(101, 0).is_none());
            // A valid defense never touches stake. Challenger 3 is a public
            // stranger, so the whole deposit reimburses the provider and the
            // defense is counted under the public tier.
            let provider = Providers::<Test>::get(2).unwrap();
            assert_eq!(provider.stake, 200);
            assert_eq!(provider.stats.challenges_failed, 0);
            assert_eq!(provider.stats.challenges_received_public, 1);
            assert_eq!(provider.stats.challenges_received_authorized, 0);
        });
    }

    #[test]
    fn respond_to_nonexistent_challenge_fails() {
        new_test_ext().execute_with(|| {
            System::set_block_number(1);
            let (_, mmr_proof, chunk_proof) = single_chunk_proof(b"chunk-0");
            assert_noop!(
                StorageProvider::respond_to_challenge(
                    RuntimeOrigin::signed(2),
                    ChallengeId {
                        deadline: 999u64,
                        index: 0u16,
                    },
                    ChallengeResponse::Proof {
                        chunk_data: make_chunk_bv(b"chunk-0"),
                        mmr_proof,
                        chunk_proof,
                    },
                ),
                Error::<Test>::ChallengeNotFound
            );
        });
    }

    #[test]
    fn respond_with_wrong_provider_fails() {
        new_test_ext().execute_with(|| {
            System::set_block_number(1);
            let chunk_data = b"chunk-0".to_vec();
            let (mmr_root, mmr_proof, chunk_proof) = single_chunk_proof(&chunk_data);
            setup_primary_with_snapshot(mmr_root, 0, 1);
            assert_ok!(StorageProvider::challenge_checkpoint(
                RuntimeOrigin::signed(3),
                0,
                2,
                ChunkLocation {
                    leaf_index: 0,
                    chunk_index: 0,
                },
            ));

            // Account 4 is not the challenged provider.
            assert_noop!(
                StorageProvider::respond_to_challenge(
                    RuntimeOrigin::signed(4),
                    ChallengeId {
                        deadline: 101u64,
                        index: 0u16,
                    },
                    ChallengeResponse::Proof {
                        chunk_data: make_chunk_bv(&chunk_data),
                        mmr_proof,
                        chunk_proof,
                    },
                ),
                Error::<Test>::NotChallengeProvider
            );
        });
    }

    /// A response whose chunk-Merkle proof doesn't verify is a clear-cut
    /// lie — the provider is slashed immediately rather than the extrinsic
    /// erroring out (which previously let them stall until timeout).
    #[test]
    fn respond_with_invalid_chunk_proof_slashes_immediately() {
        new_test_ext().execute_with(|| {
            System::set_block_number(1);
            let chunk_data = b"chunk-0".to_vec();
            let (mmr_root, mmr_proof, _good_proof) = single_chunk_proof(&chunk_data);
            setup_primary_with_snapshot(mmr_root, 0, 1);
            assert_ok!(StorageProvider::challenge_checkpoint(
                RuntimeOrigin::signed(3),
                0,
                2,
                ChunkLocation {
                    leaf_index: 0,
                    chunk_index: 0,
                },
            ));

            // Bogus chunk proof — extra sibling won't recombine to the data root.
            let bad_chunk_proof = MerkleProof {
                siblings: vec![H256::repeat_byte(0xab)],
                path: vec![true],
            };
            assert_ok!(StorageProvider::respond_to_challenge(
                RuntimeOrigin::signed(2),
                ChallengeId {
                    deadline: 101u64,
                    index: 0u16,
                },
                ChallengeResponse::Proof {
                    chunk_data: make_chunk_bv(&chunk_data),
                    mmr_proof,
                    chunk_proof: bad_chunk_proof,
                },
            ));

            // Challenge gone, provider stake gone, stats reflect the loss.
            assert!(Challenges::<Test>::get(101, 0).is_none());
            let provider = Providers::<Test>::get(2).unwrap();
            assert_eq!(provider.stake, 0);
            assert_eq!(provider.stats.challenges_failed, 1);
        });
    }

    /// Same for an MMR proof that doesn't bag to the challenged root.
    #[test]
    fn respond_with_invalid_mmr_proof_slashes_immediately() {
        new_test_ext().execute_with(|| {
            System::set_block_number(1);
            let chunk_data = b"chunk-0".to_vec();
            let (mmr_root, _, chunk_proof) = single_chunk_proof(&chunk_data);
            setup_primary_with_snapshot(mmr_root, 0, 1);
            assert_ok!(StorageProvider::challenge_checkpoint(
                RuntimeOrigin::signed(3),
                0,
                2,
                ChunkLocation {
                    leaf_index: 0,
                    chunk_index: 0,
                },
            ));

            // Construct an MMR proof for a different leaf — its bagged root
            // won't match the challenged root.
            let bad_leaf = MmrLeaf {
                data_root: H256::repeat_byte(0xff),
                data_size: 1,
                total_size: 1,
            };
            let bad_peak = blake2_256(&bad_leaf.encode());
            let bad_mmr_proof = MmrProof {
                peaks: vec![bad_peak],
                leaf: bad_leaf,
                leaf_proof: MerkleProof {
                    siblings: vec![],
                    path: vec![],
                },
            };
            assert_ok!(StorageProvider::respond_to_challenge(
                RuntimeOrigin::signed(2),
                ChallengeId {
                    deadline: 101u64,
                    index: 0u16,
                },
                ChallengeResponse::Proof {
                    chunk_data: make_chunk_bv(&chunk_data),
                    mmr_proof: bad_mmr_proof,
                    chunk_proof,
                },
            ));
            let provider = Providers::<Test>::get(2).unwrap();
            assert_eq!(provider.stake, 0);
        });
    }

    /// `ChallengeResponse::Superseded` claimed against a leaf the snapshot
    /// doesn't actually cover is a lie — slash.
    #[test]
    fn respond_with_bogus_superseded_claim_slashes_immediately() {
        new_test_ext().execute_with(|| {
            System::set_block_number(1);
            let (mmr_root, _, _) = single_chunk_proof(b"chunk-0");
            // Snapshot covers seq 0..1. Pick leaf 5 — way beyond canonical.
            setup_primary_with_snapshot(mmr_root, 0, 1);

            assert_ok!(StorageProvider::challenge_checkpoint(
                RuntimeOrigin::signed(3),
                0,
                2,
                ChunkLocation {
                    leaf_index: 5,
                    chunk_index: 0,
                },
            ));
            assert_ok!(StorageProvider::respond_to_challenge(
                RuntimeOrigin::signed(2),
                ChallengeId {
                    deadline: 101u64,
                    index: 0u16,
                },
                ChallengeResponse::Superseded,
            ));
            let provider = Providers::<Test>::get(2).unwrap();
            assert_eq!(provider.stake, 0);
            assert_eq!(provider.stats.challenges_failed, 1);
        });
    }

    #[test]
    fn respond_with_superseded_succeeds_when_canonical_advanced() {
        new_test_ext().execute_with(|| {
            System::set_block_number(1);
            let chunk_data = b"chunk-0".to_vec();
            let (mmr_root, _, _) = single_chunk_proof(&chunk_data);
            setup_primary_with_snapshot(mmr_root, 0, 1);
            // Challenge leaf 0 in a checkpoint that has been superseded.
            assert_ok!(StorageProvider::challenge_checkpoint(
                RuntimeOrigin::signed(3),
                0,
                2,
                ChunkLocation {
                    leaf_index: 0,
                    chunk_index: 0,
                },
            ));

            // Bump snapshot to cover seq 0..10 — the challenged seq 0 is now
            // within the canonical window, so `Superseded` is valid.
            Buckets::<Test>::mutate(0u64, |bucket| {
                let bucket = bucket.as_mut().unwrap();
                bucket.snapshot = Some(BucketSnapshot {
                    commitment: Commitment {
                        mmr_root: H256::repeat_byte(0x77),
                        start_seq: 0,
                        leaf_count: 10,
                    },
                    checkpoint_block: 1,
                    primary_signers: vec![0b0000_0001],
                    commitment_nonce: 1,
                });
            });

            assert_ok!(StorageProvider::respond_to_challenge(
                RuntimeOrigin::signed(2),
                ChallengeId {
                    deadline: 101u64,
                    index: 0u16,
                },
                ChallengeResponse::Superseded,
            ));
            assert!(Challenges::<Test>::get(101, 0).is_none());
        });
    }

    /// A `Superseded` claim made against the CURRENT canonical snapshot (the
    /// challenged root still equals the live snapshot root) is unsound: the
    /// data is live and the provider must answer with a `Proof`. Even though
    /// the challenged seq sits inside the canonical range, the matching root
    /// means the claim is rejected and the provider is slashed.
    #[test]
    fn superseded_slashes_when_root_matches_canonical() {
        new_test_ext().execute_with(|| {
            System::set_block_number(1);
            let (mmr_root, _, _) = single_chunk_proof(b"chunk-0");
            // Snapshot covers seq 0..1; challenge leaf 0 is in range. The
            // snapshot root is left untouched, so the challenge's root equals
            // the canonical root.
            setup_primary_with_snapshot(mmr_root, 0, 1);

            assert_ok!(StorageProvider::challenge_checkpoint(
                RuntimeOrigin::signed(3),
                0,
                2,
                ChunkLocation {
                    leaf_index: 0,
                    chunk_index: 0,
                },
            ));
            assert_ok!(StorageProvider::respond_to_challenge(
                RuntimeOrigin::signed(2),
                ChallengeId {
                    deadline: 101u64,
                    index: 0u16,
                },
                ChallengeResponse::Superseded,
            ));
            let provider = Providers::<Test>::get(2).unwrap();
            assert_eq!(provider.stake, 0);
            assert_eq!(provider.stats.challenges_failed, 1);
        });
    }

    /// Data that rolled off the front of the canonical range (challenged seq
    /// below the advanced snapshot `start_seq`) must go through the admin-signed
    /// `Deleted` path, not `Superseded`. Even with a freshly advanced root, a
    /// seq below `start_seq` fails the `contains_seq` lower bound and slashes.
    #[test]
    fn superseded_slashes_when_seq_below_canonical_start() {
        new_test_ext().execute_with(|| {
            System::set_block_number(1);
            let (mmr_root, _, _) = single_chunk_proof(b"chunk-0");
            // Initial snapshot covers seq 0..1; challenge leaf 0.
            setup_primary_with_snapshot(mmr_root, 0, 1);
            assert_ok!(StorageProvider::challenge_checkpoint(
                RuntimeOrigin::signed(3),
                0,
                2,
                ChunkLocation {
                    leaf_index: 0,
                    chunk_index: 0,
                },
            ));

            // Advance the snapshot: new root (so (a) passes) but start_seq
            // rolled past the challenged seq 0, so the canonical range is
            // 5..15 and seq 0 is below the front.
            Buckets::<Test>::mutate(0u64, |bucket| {
                let bucket = bucket.as_mut().unwrap();
                bucket.snapshot = Some(BucketSnapshot {
                    commitment: Commitment {
                        mmr_root: H256::repeat_byte(0x77),
                        start_seq: 5,
                        leaf_count: 10,
                    },
                    checkpoint_block: 1,
                    primary_signers: vec![0b0000_0001],
                    commitment_nonce: 1,
                });
            });

            assert_ok!(StorageProvider::respond_to_challenge(
                RuntimeOrigin::signed(2),
                ChallengeId {
                    deadline: 101u64,
                    index: 0u16,
                },
                ChallengeResponse::Superseded,
            ));
            let provider = Providers::<Test>::get(2).unwrap();
            assert_eq!(provider.stake, 0);
            assert_eq!(provider.stats.challenges_failed, 1);
        });
    }

    #[test]
    fn respond_after_deadline_fails() {
        new_test_ext().execute_with(|| {
            System::set_block_number(1);
            let chunk_data = b"chunk-0".to_vec();
            let (mmr_root, mmr_proof, chunk_proof) = single_chunk_proof(&chunk_data);
            setup_primary_with_snapshot(mmr_root, 0, 1);
            assert_ok!(StorageProvider::challenge_checkpoint(
                RuntimeOrigin::signed(3),
                0,
                2,
                ChunkLocation {
                    leaf_index: 0,
                    chunk_index: 0,
                },
            ));

            // Walk forward one block past the deadline. (Don't invoke pallet
            // hooks here — we want to observe the rejection branch in
            // `respond_to_challenge`, not the timeout slashing in the sweep.)
            System::set_block_number(102);
            assert_noop!(
                StorageProvider::respond_to_challenge(
                    RuntimeOrigin::signed(2),
                    ChallengeId {
                        deadline: 101u64,
                        index: 0u16,
                    },
                    ChallengeResponse::Proof {
                        chunk_data: make_chunk_bv(&chunk_data),
                        mmr_proof,
                        chunk_proof,
                    },
                ),
                Error::<Test>::ChallengeExpired
            );
        });
    }

    // ─────────────────────────────────────────────────────────────────────────
    // Timeout slashing via the on_initialize sweep
    // ─────────────────────────────────────────────────────────────────────────

    #[test]
    fn challenge_timeout_slashes_provider() {
        new_test_ext().execute_with(|| {
            System::set_block_number(1);
            let (mmr_root, _, _) = single_chunk_proof(b"chunk-0");
            setup_primary_with_snapshot(mmr_root, 0, 1);

            // Challenger 3 starts with 10_000 (genesis), then `create_challenge`
            // reserves 100 deposit.
            let challenger_before = Balances::free_balance(3);
            assert_ok!(StorageProvider::challenge_checkpoint(
                RuntimeOrigin::signed(3),
                0,
                2,
                ChunkLocation {
                    leaf_index: 0,
                    chunk_index: 0,
                },
            ));
            assert_eq!(Balances::reserved_balance(3), 100);

            // Provider has stake of 200 reserved at registration.
            assert_eq!(Balances::reserved_balance(2), 200);

            // Advance one block past the deadline; the sweep at 102 slashes.
            run_to_block(102);

            // Provider stake is zero post-slash.
            let provider = Providers::<Test>::get(2).unwrap();
            assert_eq!(provider.stake, 0);
            assert_eq!(provider.stats.challenges_failed, 1);

            // Challenger deposit is unreserved (refunded); no reward is paid
            // (design: refund only), so free balance returns to where it began.
            assert_eq!(Balances::reserved_balance(3), 0);
            assert_eq!(Balances::free_balance(3), challenger_before);

            // Challenge cleared from storage.
            assert!(Challenges::<Test>::get(101, 0).is_none());
        });
    }

    /// The sweep drains a range of deadline keys, so a deadline skipped over
    /// by a block-number jump (relay numbers can advance by more than one
    /// between parachain blocks) is still slashed.
    #[test]
    fn challenge_timeout_sweep_catches_skipped_deadlines() {
        new_test_ext().execute_with(|| {
            System::set_block_number(1);
            let (mmr_root, _, _) = single_chunk_proof(b"chunk-0");
            setup_primary_with_snapshot(mmr_root, 0, 1);
            assert_ok!(StorageProvider::challenge_checkpoint(
                RuntimeOrigin::signed(3),
                0,
                2,
                ChunkLocation {
                    leaf_index: 0,
                    chunk_index: 0,
                },
            ));

            // Walk close to the deadline (101), then jump well past it in a
            // single step and run one on_initialize.
            run_to_block(100);
            assert_eq!(
                Providers::<Test>::get(2).unwrap().stats.challenges_failed,
                0
            );
            System::set_block_number(110);
            <StorageProvider as Hooks<u64>>::on_initialize(110);

            // Deadline 101 was never the "current" block, yet the range sweep
            // (cursor 99 → 109) drained and slashed it.
            let provider = Providers::<Test>::get(2).unwrap();
            assert_eq!(provider.stats.challenges_failed, 1);
            assert!(Challenges::<Test>::get(101, 0).is_none());
            assert_eq!(LastSweptChallengeBlock::<Test>::get(), Some(109));
        });
    }

    /// A single sweep drains at most `MAX_SWEEP_SPAN` (32) keys; the
    /// remainder carries over via the cursor and drains on later blocks, so
    /// a huge gap cannot blow one block but slashes still land.
    #[test]
    fn challenge_timeout_sweep_is_capped_and_carries_over() {
        new_test_ext().execute_with(|| {
            System::set_block_number(1);
            let (mmr_root, _, _) = single_chunk_proof(b"chunk-0");
            setup_primary_with_snapshot(mmr_root, 0, 1);
            assert_ok!(StorageProvider::challenge_checkpoint(
                RuntimeOrigin::signed(3),
                0,
                2,
                ChunkLocation {
                    leaf_index: 0,
                    chunk_index: 0,
                },
            ));

            // Anchor the cursor at 1, then jump far past the deadline (101).
            run_to_block(2);
            assert_eq!(LastSweptChallengeBlock::<Test>::get(), Some(1));
            System::set_block_number(500);

            // Each sweep advances the cursor by at most 32 keys: 33, 65, 97 —
            // deadline 101 still pending after three sweeps.
            for expected_cursor in [33, 65, 97] {
                <StorageProvider as Hooks<u64>>::on_initialize(500);
                assert_eq!(
                    LastSweptChallengeBlock::<Test>::get(),
                    Some(expected_cursor)
                );
            }
            assert!(Challenges::<Test>::get(101, 0).is_some());
            assert_eq!(
                Providers::<Test>::get(2).unwrap().stats.challenges_failed,
                0
            );

            // Fourth sweep covers 98..=129 and slashes.
            <StorageProvider as Hooks<u64>>::on_initialize(500);
            assert_eq!(LastSweptChallengeBlock::<Test>::get(), Some(129));
            assert!(Challenges::<Test>::get(101, 0).is_none());
            assert_eq!(
                Providers::<Test>::get(2).unwrap().stats.challenges_failed,
                1
            );
        });
    }

    /// The sweep never slashes more than `MaxChallengesPerDeadline` (5 in
    /// this mock) challenges per block even when a gap matured several
    /// populated deadlines at once; on exhaustion it parks the cursor below
    /// the partially drained key and finishes on the next block.
    #[test]
    fn challenge_timeout_sweep_budget_carries_over_mid_key() {
        new_test_ext().execute_with(|| {
            System::set_block_number(1);
            let (mmr_root, _, _) = single_chunk_proof(b"chunk-0");
            setup_primary_with_snapshot(mmr_root, 0, 1);

            let challenge = || {
                assert_ok!(StorageProvider::challenge_checkpoint(
                    RuntimeOrigin::signed(3),
                    0,
                    2,
                    ChunkLocation {
                        leaf_index: 0,
                        chunk_index: 0,
                    },
                ));
            };

            // Load two consecutive deadlines with 3 challenges each: 199
            // (created at block 99) and 200 (created at block 100).
            run_to_block(99);
            for _ in 0..3 {
                challenge();
            }
            run_to_block(100);
            for _ in 0..3 {
                challenge();
            }
            assert_eq!(PendingChallenges::<Test>::get(2), 6);

            // Jump far past both deadlines. The span cap (32 keys/sweep)
            // takes three sweeps to walk the empty range up to key 195.
            System::set_block_number(300);
            for _ in 0..3 {
                <StorageProvider as Hooks<u64>>::on_initialize(300);
            }
            assert_eq!(LastSweptChallengeBlock::<Test>::get(), Some(195));
            assert_eq!(
                Providers::<Test>::get(2).unwrap().stats.challenges_failed,
                0
            );

            // Fourth sweep reaches the loaded keys: drains all 3 at 199,
            // then only 2 of 3 at 200 before the budget (5) is exhausted —
            // cursor parks below the partially drained key, whose index
            // allocator survives for the carry-over.
            <StorageProvider as Hooks<u64>>::on_initialize(300);
            assert_eq!(
                Providers::<Test>::get(2).unwrap().stats.challenges_failed,
                5
            );
            assert_eq!(LastSweptChallengeBlock::<Test>::get(), Some(199));
            assert_eq!(Challenges::<Test>::iter_prefix(200).count(), 1);
            assert_eq!(NextChallengeIndex::<Test>::get(200), 3);
            assert_eq!(PendingChallenges::<Test>::get(2), 1);

            // Fifth sweep finishes the key and cleans up its allocator.
            <StorageProvider as Hooks<u64>>::on_initialize(300);
            assert_eq!(
                Providers::<Test>::get(2).unwrap().stats.challenges_failed,
                6
            );
            assert_eq!(Challenges::<Test>::iter_prefix(200).count(), 0);
            assert_eq!(NextChallengeIndex::<Test>::get(200), 0);
            assert_eq!(PendingChallenges::<Test>::get(2), 0);
            assert_eq!(LastSweptChallengeBlock::<Test>::get(), Some(231));
        });
    }

    /// Re-running the sweep at an unchanged block number (several parachain
    /// blocks can share one relay parent) is a no-op: no double slash, no
    /// counter underflow.
    #[test]
    fn challenge_timeout_sweep_idempotent_at_same_block() {
        new_test_ext().execute_with(|| {
            System::set_block_number(1);
            let (mmr_root, _, _) = single_chunk_proof(b"chunk-0");
            setup_primary_with_snapshot(mmr_root, 0, 1);
            assert_ok!(StorageProvider::challenge_checkpoint(
                RuntimeOrigin::signed(3),
                0,
                2,
                ChunkLocation {
                    leaf_index: 0,
                    chunk_index: 0,
                },
            ));

            run_to_block(102);
            let provider = Providers::<Test>::get(2).unwrap();
            assert_eq!(provider.stats.challenges_failed, 1);
            assert_eq!(PendingChallenges::<Test>::get(2), 0);
            let cursor = LastSweptChallengeBlock::<Test>::get();

            <StorageProvider as Hooks<u64>>::on_initialize(102);

            let provider = Providers::<Test>::get(2).unwrap();
            assert_eq!(provider.stats.challenges_failed, 1);
            assert_eq!(PendingChallenges::<Test>::get(2), 0);
            assert_eq!(LastSweptChallengeBlock::<Test>::get(), cursor);
        });
    }

    /// The very first sweep anchors the cursor at the current clock instead
    /// of scanning up from zero (live relay numbers start in the millions).
    #[test]
    fn challenge_timeout_sweep_anchors_cursor_on_first_run() {
        new_test_ext().execute_with(|| {
            System::set_block_number(1_000_000);
            assert_eq!(LastSweptChallengeBlock::<Test>::get(), None);

            <StorageProvider as Hooks<u64>>::on_initialize(1_000_000);

            assert_eq!(LastSweptChallengeBlock::<Test>::get(), Some(999_999));
        });
    }

    // ─────────────────────────────────────────────────────────────────────────
    // challenge_replica
    // ─────────────────────────────────────────────────────────────────────────

    #[test]
    fn challenge_replica_uses_last_sync() {
        new_test_ext().execute_with(|| {
            System::set_block_number(1);
            // Two providers — primary (2) and replica (4).
            let primary_addr = b"/ip4/127.0.0.1/tcp/3000".to_vec();
            assert_ok!(StorageProvider::register_provider(
                RuntimeOrigin::signed(2),
                primary_addr.try_into().unwrap(),
                test_public_key(),
                200
            ));
            let replica_addr = b"/ip4/127.0.0.1/tcp/3001".to_vec();
            assert_ok!(StorageProvider::register_provider(
                RuntimeOrigin::signed(4),
                replica_addr.try_into().unwrap(),
                test_public_key(),
                200
            ));
            // Enable replica acceptance via settings update.
            let settings = ProviderSettings {
                min_duration: 0u64,
                max_duration: 1000u64,
                price_per_byte: 0u64,
                accepting_primary: false,
                replica_sync_price: Some(1u64),
                accepting_extensions: true,
                max_capacity: 0,
            };
            assert_ok!(StorageProvider::update_provider_settings(
                RuntimeOrigin::signed(4),
                settings
            ));
            // Create bucket 0 together with the primary (provider 2) agreement
            // by redeeming provider-signed terms — the merged pallet's way to
            // establish a primary, replacing the old request/accept pair.
            let bucket_id = setup_agreement(2, 1, 100, 100);
            debug_assert_eq!(bucket_id, 0);

            // Inject a replica agreement directly with a `last_sync` set. The
            // request_replica_agreement / accept flow has a lot of moving parts
            // unrelated to what's under test here.
            let last_root = H256::repeat_byte(0x42);
            let replica_agreement = StorageAgreement::<Test> {
                owner: 1,
                max_bytes: 100,
                payment_locked: 0,
                price_per_byte: 0,
                expires_at: 1000,
                extensions_blocked: false,
                role: ProviderRole::Replica {
                    sync_balance: 100,
                    sync_price: 1,
                    min_sync_interval: 0,
                    last_sync: Some(ReplicaSyncRecord {
                        // commit 4: replica's challenge_replica now uses start_seq
                        commitment: Commitment {
                            mmr_root: last_root,
                            start_seq: 7,
                            leaf_count: 3,
                        },
                        block: 1,
                    }),
                },
                started_at: 1,
            };
            StorageAgreements::<Test>::insert(0u64, 4u64, replica_agreement);
            // Provider 4 stats need challenges_received bump infra — bump
            // committed_bytes so the slash path doesn't underflow:
            Providers::<Test>::mutate(4u64, |p| {
                if let Some(p) = p.as_mut() {
                    p.committed_bytes = 100;
                }
            });

            assert_ok!(StorageProvider::challenge_replica(
                RuntimeOrigin::signed(3),
                0,
                4,
                ChunkLocation {
                    leaf_index: 0,
                    chunk_index: 0,
                },
            ));

            let challenge = Challenges::<Test>::get(101, 0).expect("created");
            assert_eq!(challenge.mmr_root, last_root);
            // start_seq now reflects the value captured at sync time
            // (previously hardcoded 0u64 placeholder).
            assert_eq!(challenge.start_seq, 7);
        });
    }

    #[test]
    fn challenge_replica_fails_for_primary_provider() {
        new_test_ext().execute_with(|| {
            System::set_block_number(1);
            let (mmr_root, _, _) = single_chunk_proof(b"chunk-0");
            setup_primary_with_snapshot(mmr_root, 0, 1);
            assert_noop!(
                StorageProvider::challenge_replica(
                    RuntimeOrigin::signed(3),
                    0,
                    2,
                    ChunkLocation {
                        leaf_index: 0,
                        chunk_index: 0,
                    },
                ),
                Error::<Test>::NotReplica
            );
        });
    }

    // ─────────────────────────────────────────────────────────────────────────
    // Replay protection — `CommitmentPayload::nonce` recency
    // ─────────────────────────────────────────────────────────────────────────

    /// Pre-replay-protection, a provider's signature was valid forever. With
    /// `T::MaxNonceAge` enforcement, a nonce older than that window is
    /// rejected before signature verification fires — so the test can use a
    /// dummy signature and still observe the rejection.
    #[test]
    fn challenge_offchain_rejects_old_nonce() {
        new_test_ext().execute_with(|| {
            // Mock `MaxNonceAge = 200`. Place us far enough ahead that nonce=1
            // is outside the window.
            System::set_block_number(500);
            let (mmr_root, _, _) = single_chunk_proof(b"chunk-0");
            setup_primary_with_snapshot(mmr_root, 0, 1);

            let dummy_sig =
                sp_runtime::MultiSignature::Sr25519(sp_core::sr25519::Signature::from([0u8; 64]));
            assert_noop!(
                StorageProvider::challenge_offchain(
                    RuntimeOrigin::signed(3),
                    0,
                    2,
                    Commitment {
                        mmr_root,
                        start_seq: 0,
                        leaf_count: 1,
                    },
                    ChunkLocation {
                        leaf_index: 0,
                        chunk_index: 0,
                    },
                    1,
                    dummy_sig,
                ),
                Error::<Test>::CommitmentNonceTooOld
            );
        });
    }

    /// A future-dated nonce is nonsensical (the signer can't know future
    /// blocks at sign-time) and is rejected the same way.
    #[test]
    fn challenge_offchain_rejects_future_nonce() {
        new_test_ext().execute_with(|| {
            System::set_block_number(10);
            let (mmr_root, _, _) = single_chunk_proof(b"chunk-0");
            setup_primary_with_snapshot(mmr_root, 0, 1);

            let dummy_sig =
                sp_runtime::MultiSignature::Sr25519(sp_core::sr25519::Signature::from([0u8; 64]));
            assert_noop!(
                StorageProvider::challenge_offchain(
                    RuntimeOrigin::signed(3),
                    0,
                    2,
                    Commitment {
                        mmr_root,
                        start_seq: 0,
                        leaf_count: 1,
                    },
                    ChunkLocation {
                        leaf_index: 0,
                        chunk_index: 0,
                    },
                    9999,
                    dummy_sig,
                ),
                Error::<Test>::CommitmentNonceTooOld
            );
        });
    }

    /// The same recency gate fires on the multi-signature `checkpoint`
    /// extrinsic, before any per-signature work happens. Same dummy-signature
    /// trick works here.
    #[test]
    fn checkpoint_rejects_old_nonce() {
        new_test_ext().execute_with(|| {
            System::set_block_number(500);
            // No snapshot needed — the recency check runs before bucket
            // resolution.
            let dummy_sig =
                sp_runtime::MultiSignature::Sr25519(sp_core::sr25519::Signature::from([0u8; 64]));
            let sigs: frame_support::BoundedVec<
                (u64, sp_runtime::MultiSignature),
                <Test as Config>::MaxPrimaryProviders,
            > = vec![(2, dummy_sig)].try_into().unwrap();

            assert_noop!(
                StorageProvider::checkpoint(
                    RuntimeOrigin::signed(1),
                    0,
                    Commitment {
                        mmr_root: H256::zero(),
                        start_seq: 0,
                        leaf_count: 0,
                    },
                    1,
                    sigs,
                ),
                Error::<Test>::CommitmentNonceTooOld
            );
        });
    }

    #[test]
    fn challenge_replica_fails_without_last_sync() {
        new_test_ext().execute_with(|| {
            System::set_block_number(1);
            // Bare bucket 0 (no primary agreement needed for this path).
            let bucket_id = create_bucket(1, 1);
            debug_assert_eq!(bucket_id, 0);
            let replica_addr = b"/ip4/127.0.0.1/tcp/3001".to_vec();
            assert_ok!(StorageProvider::register_provider(
                RuntimeOrigin::signed(4),
                replica_addr.try_into().unwrap(),
                test_public_key(),
                200
            ));
            // Replica agreement without a confirmed sync.
            let replica_agreement = StorageAgreement::<Test> {
                owner: 1,
                max_bytes: 100,
                payment_locked: 0,
                price_per_byte: 0,
                expires_at: 1000,
                extensions_blocked: false,
                role: ProviderRole::Replica {
                    sync_balance: 100,
                    sync_price: 1,
                    min_sync_interval: 0,
                    last_sync: None::<ReplicaSyncRecord<u64>>,
                },
                started_at: 1,
            };
            StorageAgreements::<Test>::insert(bucket_id, 4u64, replica_agreement);

            assert_noop!(
                StorageProvider::challenge_replica(
                    RuntimeOrigin::signed(3),
                    bucket_id,
                    4,
                    ChunkLocation {
                        leaf_index: 0,
                        chunk_index: 0
                    }
                ),
                Error::<Test>::InvalidSyncRoot
            );
        });
    }

    // ─────────────────────────────────────────────────────────────────────────
    // ChallengerStats — per-challenger aggregates
    // ─────────────────────────────────────────────────────────────────────────

    #[test]
    fn challenger_stats_total_bumps_on_create_challenge() {
        new_test_ext().execute_with(|| {
            System::set_block_number(1);
            let (mmr_root, _, _) = single_chunk_proof(b"chunk-0");
            setup_primary_with_snapshot(mmr_root, 0, 1);

            assert_eq!(
                ChallengerStats::<Test>::get(3),
                ChallengerStatRecord::default()
            );
            assert_ok!(StorageProvider::challenge_checkpoint(
                RuntimeOrigin::signed(3),
                0,
                2,
                ChunkLocation {
                    leaf_index: 0,
                    chunk_index: 0,
                },
            ));
            let stats = ChallengerStats::<Test>::get(3);
            assert_eq!(stats.total_challenges, 1);
            assert_eq!(stats.successful_challenges, 0);
            assert_eq!(stats.failed_challenges, 0);
        });
    }

    #[test]
    fn challenger_stats_failed_bumps_when_provider_defends() {
        new_test_ext().execute_with(|| {
            System::set_block_number(1);
            let chunk_data = b"chunk-0".to_vec();
            let (mmr_root, mmr_proof, chunk_proof) = single_chunk_proof(&chunk_data);
            setup_primary_with_snapshot(mmr_root, 0, 1);
            assert_ok!(StorageProvider::challenge_checkpoint(
                RuntimeOrigin::signed(3),
                0,
                2,
                ChunkLocation {
                    leaf_index: 0,
                    chunk_index: 0,
                },
            ));
            assert_ok!(StorageProvider::respond_to_challenge(
                RuntimeOrigin::signed(2),
                ChallengeId {
                    deadline: 101u64,
                    index: 0u16,
                },
                ChallengeResponse::Proof {
                    chunk_data: make_chunk_bv(&chunk_data),
                    mmr_proof,
                    chunk_proof,
                },
            ));
            let stats = ChallengerStats::<Test>::get(3);
            assert_eq!(stats.total_challenges, 1);
            assert_eq!(stats.failed_challenges, 1);
            assert_eq!(stats.successful_challenges, 0);
        });
    }

    #[test]
    fn challenger_stats_successful_bumps_on_timeout_slash() {
        new_test_ext().execute_with(|| {
            System::set_block_number(1);
            let (mmr_root, _, _) = single_chunk_proof(b"chunk-0");
            setup_primary_with_snapshot(mmr_root, 0, 1);
            assert_ok!(StorageProvider::challenge_checkpoint(
                RuntimeOrigin::signed(3),
                0,
                2,
                ChunkLocation {
                    leaf_index: 0,
                    chunk_index: 0,
                },
            ));
            run_to_block(102);
            let stats = ChallengerStats::<Test>::get(3);
            assert_eq!(stats.total_challenges, 1);
            assert_eq!(stats.successful_challenges, 1);
            assert_eq!(stats.failed_challenges, 0);
        });
    }

    #[test]
    fn challenger_stats_successful_bumps_on_invalid_proof_slash() {
        new_test_ext().execute_with(|| {
            System::set_block_number(1);
            let chunk_data = b"chunk-0".to_vec();
            let (mmr_root, mmr_proof, _) = single_chunk_proof(&chunk_data);
            setup_primary_with_snapshot(mmr_root, 0, 1);
            assert_ok!(StorageProvider::challenge_checkpoint(
                RuntimeOrigin::signed(3),
                0,
                2,
                ChunkLocation {
                    leaf_index: 0,
                    chunk_index: 0,
                },
            ));
            let bad_proof = MerkleProof {
                siblings: vec![H256::repeat_byte(0xab)],
                path: vec![true],
            };
            assert_ok!(StorageProvider::respond_to_challenge(
                RuntimeOrigin::signed(2),
                ChallengeId {
                    deadline: 101u64,
                    index: 0u16,
                },
                ChallengeResponse::Proof {
                    chunk_data: make_chunk_bv(&chunk_data),
                    mmr_proof,
                    chunk_proof: bad_proof,
                },
            ));
            let stats = ChallengerStats::<Test>::get(3);
            assert_eq!(stats.successful_challenges, 1);
        });
    }

    // ─────────────────────────────────────────────────────────────────────────
    // Fix 2 — lifecycle gating + pending-challenge counters
    // ─────────────────────────────────────────────────────────────────────────

    /// Sign a `CommitmentPayload` the way a provider would when quoting an
    /// off-chain commitment, so `challenge_offchain` accepts it.
    fn signed_offchain_commitment(
        provider: u64,
        bucket_id: u64,
        mmr_root: H256,
        start_seq: u64,
        leaf_count: u64,
        nonce: u64,
    ) -> sp_runtime::MultiSignature {
        let pair = provider_signer(provider);
        let payload = storage_primitives::CommitmentPayload::new(
            bucket_id,
            Commitment {
                mmr_root,
                start_seq,
                leaf_count,
            },
            nonce,
        );
        // `verify_signature` checks the raw encoded payload (sr25519 hashes
        // internally), so sign the encoding directly — no extra blake2 round.
        sp_runtime::MultiSignature::Sr25519(pair.sign(&payload.encode()))
    }

    // Counter lifecycle ───────────────────────────────────────────────────────

    /// `create_challenge` bumps both pending counters to 1, and a defended
    /// `respond_to_challenge` (valid proof) returns both to 0.
    #[test]
    fn pending_counters_zero_after_defended_response() {
        new_test_ext().execute_with(|| {
            System::set_block_number(1);
            let chunk_data = b"chunk-0".to_vec();
            let (mmr_root, mmr_proof, chunk_proof) = single_chunk_proof(&chunk_data);
            setup_primary_with_snapshot(mmr_root, 0, 1);

            assert_ok!(StorageProvider::challenge_checkpoint(
                RuntimeOrigin::signed(3),
                0,
                2,
                ChunkLocation {
                    leaf_index: 0,
                    chunk_index: 0,
                },
            ));
            assert_eq!(PendingChallenges::<Test>::get(2), 1);
            assert_eq!(PendingChallengesByBucket::<Test>::get(0, 2), 1);

            assert_ok!(StorageProvider::respond_to_challenge(
                RuntimeOrigin::signed(2),
                ChallengeId {
                    deadline: 101u64,
                    index: 0u16,
                },
                ChallengeResponse::Proof {
                    chunk_data: make_chunk_bv(&chunk_data),
                    mmr_proof,
                    chunk_proof,
                },
            ));
            assert_eq!(PendingChallenges::<Test>::get(2), 0);
            assert_eq!(PendingChallengesByBucket::<Test>::get(0, 2), 0);
        });
    }

    /// An invalid-response slash also resolves the challenge, so both counters
    /// return to 0.
    #[test]
    fn pending_counters_zero_after_invalid_response_slash() {
        new_test_ext().execute_with(|| {
            System::set_block_number(1);
            let chunk_data = b"chunk-0".to_vec();
            let (mmr_root, mmr_proof, _) = single_chunk_proof(&chunk_data);
            setup_primary_with_snapshot(mmr_root, 0, 1);

            assert_ok!(StorageProvider::challenge_checkpoint(
                RuntimeOrigin::signed(3),
                0,
                2,
                ChunkLocation {
                    leaf_index: 0,
                    chunk_index: 0,
                },
            ));
            assert_eq!(PendingChallenges::<Test>::get(2), 1);
            assert_eq!(PendingChallengesByBucket::<Test>::get(0, 2), 1);

            // A bogus chunk proof is a demonstrable lie → immediate slash.
            let bad_proof = MerkleProof {
                siblings: vec![H256::repeat_byte(0xab)],
                path: vec![true],
            };
            assert_ok!(StorageProvider::respond_to_challenge(
                RuntimeOrigin::signed(2),
                ChallengeId {
                    deadline: 101u64,
                    index: 0u16,
                },
                ChallengeResponse::Proof {
                    chunk_data: make_chunk_bv(&chunk_data),
                    mmr_proof,
                    chunk_proof: bad_proof,
                },
            ));
            assert_eq!(Providers::<Test>::get(2).unwrap().stake, 0);
            assert_eq!(PendingChallenges::<Test>::get(2), 0);
            assert_eq!(PendingChallengesByBucket::<Test>::get(0, 2), 0);
        });
    }

    /// A timeout slash (deadline passes → the sweep drains the challenge)
    /// returns both counters to 0.
    #[test]
    fn pending_counters_zero_after_timeout_slash() {
        new_test_ext().execute_with(|| {
            System::set_block_number(1);
            let (mmr_root, _, _) = single_chunk_proof(b"chunk-0");
            setup_primary_with_snapshot(mmr_root, 0, 1);

            assert_ok!(StorageProvider::challenge_checkpoint(
                RuntimeOrigin::signed(3),
                0,
                2,
                ChunkLocation {
                    leaf_index: 0,
                    chunk_index: 0,
                },
            ));
            assert_eq!(PendingChallenges::<Test>::get(2), 1);
            assert_eq!(PendingChallengesByBucket::<Test>::get(0, 2), 1);

            // Deadline = 1 + ChallengeTimeout(100) = 101; advance past it.
            run_to_block(102);
            assert_eq!(
                Providers::<Test>::get(2).unwrap().stats.challenges_failed,
                1
            );
            assert_eq!(PendingChallenges::<Test>::get(2), 0);
            assert_eq!(PendingChallengesByBucket::<Test>::get(0, 2), 0);
        });
    }

    // Case B — challenge entry gated on a live agreement ───────────────────────

    /// `challenge_offchain` succeeds while the agreement is live and fails with
    /// `AgreementExpired` once the block reaches `expires_at`.
    #[test]
    fn challenge_offchain_rejects_expired_agreement() {
        new_test_ext().execute_with(|| {
            System::set_block_number(1);
            let (mmr_root, _, _) = single_chunk_proof(b"chunk-0");
            // setup_agreement uses duration 100 → expires_at = 1 + 100 = 101.
            setup_primary_with_snapshot(mmr_root, 0, 1);

            // While active (block 5 < expires_at 101): a valid signature opens
            // the challenge.
            System::set_block_number(5);
            let sig = signed_offchain_commitment(2, 0, mmr_root, 0, 1, 5);
            assert_ok!(StorageProvider::challenge_offchain(
                RuntimeOrigin::signed(3),
                0,
                2,
                Commitment {
                    mmr_root,
                    start_seq: 0,
                    leaf_count: 1,
                },
                ChunkLocation {
                    leaf_index: 0,
                    chunk_index: 0,
                },
                5,
                sig,
            ));

            // At/after expiry the entry is rejected before signature checks, so
            // a dummy signature still surfaces `AgreementExpired`.
            System::set_block_number(101);
            let dummy =
                sp_runtime::MultiSignature::Sr25519(sp_core::sr25519::Signature::from([0u8; 64]));
            assert_noop!(
                StorageProvider::challenge_offchain(
                    RuntimeOrigin::signed(3),
                    0,
                    2,
                    Commitment {
                        mmr_root,
                        start_seq: 0,
                        leaf_count: 1,
                    },
                    ChunkLocation {
                        leaf_index: 0,
                        chunk_index: 0,
                    },
                    101,
                    dummy,
                ),
                Error::<Test>::AgreementExpired
            );
        });
    }

    /// `challenge_replica` succeeds while the agreement is live and fails with
    /// `AgreementExpired` once the block reaches `expires_at`.
    #[test]
    fn challenge_replica_rejects_expired_agreement() {
        new_test_ext().execute_with(|| {
            System::set_block_number(1);
            let bucket_id = create_bucket(1, 1);
            let replica_addr = b"/ip4/127.0.0.1/tcp/3001".to_vec();
            assert_ok!(StorageProvider::register_provider(
                RuntimeOrigin::signed(4),
                replica_addr.try_into().unwrap(),
                test_public_key(),
                200
            ));
            // Replica agreement expiring at block 50, with a confirmed sync so
            // the role carries a usable `last_sync` root.
            let replica_agreement = StorageAgreement::<Test> {
                owner: 1,
                max_bytes: 100,
                payment_locked: 0,
                price_per_byte: 0,
                expires_at: 50,
                extensions_blocked: false,
                role: ProviderRole::Replica {
                    sync_balance: 100,
                    sync_price: 1,
                    min_sync_interval: 0,
                    last_sync: Some(ReplicaSyncRecord {
                        commitment: Commitment {
                            mmr_root: H256::repeat_byte(0xAB),
                            start_seq: 0,
                            leaf_count: 10,
                        },
                        block: 1u64,
                    }),
                },
                started_at: 1,
            };
            StorageAgreements::<Test>::insert(bucket_id, 4u64, replica_agreement);

            // Active (block 10 < expires_at 50): challenge opens.
            System::set_block_number(10);
            assert_ok!(StorageProvider::challenge_replica(
                RuntimeOrigin::signed(3),
                bucket_id,
                4,
                ChunkLocation {
                    leaf_index: 0,
                    chunk_index: 0,
                },
            ));

            // At expiry (block 50 == expires_at): rejected.
            System::set_block_number(50);
            assert_noop!(
                StorageProvider::challenge_replica(
                    RuntimeOrigin::signed(3),
                    bucket_id,
                    4,
                    ChunkLocation {
                        leaf_index: 0,
                        chunk_index: 0
                    }
                ),
                Error::<Test>::AgreementExpired
            );
        });
    }

    // Case A — teardown blocked while a challenge is pending ───────────────────

    /// With a pending challenge, `end_agreement` is rejected; after the
    /// challenge is defended, teardown succeeds.
    #[test]
    fn end_agreement_blocked_while_challenge_pending() {
        new_test_ext().execute_with(|| {
            System::set_block_number(1);
            let chunk_data = b"chunk-0".to_vec();
            let (mmr_root, mmr_proof, chunk_proof) = single_chunk_proof(&chunk_data);
            // Bucket 0 owned by 1, provider 2, expires_at = 1 + 100 = 101.
            setup_primary_with_snapshot(mmr_root, 0, 1);

            assert_ok!(StorageProvider::challenge_checkpoint(
                RuntimeOrigin::signed(3),
                0,
                2,
                ChunkLocation {
                    leaf_index: 0,
                    chunk_index: 0,
                },
            ));

            // Early termination by admin while the challenge is pending: blocked.
            assert_noop!(
                StorageProvider::end_agreement(RuntimeOrigin::signed(1), 0, 2, EndAction::Pay),
                Error::<Test>::AgreementHasPendingChallenge
            );

            // Defend the challenge → counter clears.
            assert_ok!(StorageProvider::respond_to_challenge(
                RuntimeOrigin::signed(2),
                ChallengeId {
                    deadline: 101u64,
                    index: 0u16,
                },
                ChallengeResponse::Proof {
                    chunk_data: make_chunk_bv(&chunk_data),
                    mmr_proof,
                    chunk_proof,
                },
            ));
            assert_eq!(PendingChallengesByBucket::<Test>::get(0, 2), 0);

            // Now early termination succeeds.
            assert_ok!(StorageProvider::end_agreement(
                RuntimeOrigin::signed(1),
                0,
                2,
                EndAction::Pay
            ));
        });
    }

    /// With a pending challenge, `claim_expired_agreement` is rejected; after a
    /// timeout slash resolves the challenge, the claim path is no longer gated
    /// by the pending counter (it still requires expiry + settlement window,
    /// which a slashed provider can satisfy).
    #[test]
    fn claim_expired_agreement_blocked_while_challenge_pending() {
        new_test_ext().execute_with(|| {
            System::set_block_number(1);
            let (mmr_root, _, _) = single_chunk_proof(b"chunk-0");
            // expires_at = 1 + 100 = 101.
            setup_primary_with_snapshot(mmr_root, 0, 1);

            assert_ok!(StorageProvider::challenge_checkpoint(
                RuntimeOrigin::signed(3),
                0,
                2,
                ChunkLocation {
                    leaf_index: 0,
                    chunk_index: 0,
                },
            ));

            // Move past expiry (101) + SettlementTimeout (50) = 151 so the
            // claim's own gates pass. Crossing block 101 fires the challenge
            // timeout via the on_initialize sweep, clearing the pending counter.
            run_to_block(152);
            // The timeout slash at block 101 cleared the pending counter.
            assert_eq!(PendingChallengesByBucket::<Test>::get(0, 2), 0);

            // Provider 2 was slashed but the agreement row remains; the claim
            // path is unblocked by the (now-zero) pending counter.
            assert_ok!(StorageProvider::claim_expired_agreement(
                RuntimeOrigin::signed(2),
                0
            ));
        });
    }

    /// `claim_expired_agreement` surfaces `AgreementHasPendingChallenge` while
    /// a challenge is live — the pending gate is checked before the
    /// expiry/settlement gates, so it fires at any block while the counter is
    /// non-zero.
    #[test]
    fn claim_expired_agreement_rejects_pending_challenge_error() {
        new_test_ext().execute_with(|| {
            System::set_block_number(1);
            let (mmr_root, _, _) = single_chunk_proof(b"chunk-0");
            setup_primary_with_snapshot(mmr_root, 0, 1);

            assert_ok!(StorageProvider::challenge_checkpoint(
                RuntimeOrigin::signed(3),
                0,
                2,
                ChunkLocation {
                    leaf_index: 0,
                    chunk_index: 0,
                },
            ));
            assert_eq!(PendingChallengesByBucket::<Test>::get(0, 2), 1);

            // Plain set_block_number (no hooks, so no sweep) keeps the challenge
            // pending. The pending gate precedes the expiry check, so the
            // rejection is `AgreementHasPendingChallenge`.
            System::set_block_number(60);
            assert_noop!(
                StorageProvider::claim_expired_agreement(RuntimeOrigin::signed(2), 0),
                Error::<Test>::AgreementHasPendingChallenge
            );
        });
    }

    // Deregister race ──────────────────────────────────────────────────────────

    /// With a pending challenge, `complete_deregister` is rejected with
    /// `ProviderHasPendingChallenges`; after a timeout slash resolves it,
    /// completion succeeds.
    ///
    /// `deregister_provider` (announce) itself requires `committed_bytes == 0`,
    /// and Case A blocks ending the agreement while the challenge is pending —
    /// so a realistic path can't even reach `complete_deregister` with a live
    /// challenge. We therefore set up the announce preconditions by zeroing
    /// `committed_bytes` and stamping `deregister_at` directly, isolating the
    /// `complete_deregister` pending-challenge gate. The challenge itself is
    /// real (so the counter is genuinely maintained by the pallet).
    #[test]
    fn complete_deregister_blocked_while_challenge_pending() {
        new_test_ext().execute_with(|| {
            System::set_block_number(1);
            let (mmr_root, _, _) = single_chunk_proof(b"chunk-0");
            // Provider 2, bucket 0 owned by 1; challenge deadline = 101.
            setup_primary_with_snapshot(mmr_root, 0, 1);

            assert_ok!(StorageProvider::challenge_checkpoint(
                RuntimeOrigin::signed(3),
                0,
                2,
                ChunkLocation {
                    leaf_index: 0,
                    chunk_index: 0,
                },
            ));
            assert_eq!(PendingChallenges::<Test>::get(2), 1);

            // Stamp the announce preconditions directly (see doc comment).
            Providers::<Test>::mutate(2, |p| {
                let p = p.as_mut().unwrap();
                p.committed_bytes = 0;
                p.deregister_at = Some(101); // period elapsed at block 101
            });

            // At block 102 the on_initialize sweep has passed deadline 101 —
            // clearing the pending counter and slashing provider 2. Block 102
            // is also >= deregister_at (101).
            run_to_block(102);
            assert_eq!(PendingChallenges::<Test>::get(2), 0);

            // With the challenge resolved and committed_bytes zero, completion
            // succeeds.
            assert_ok!(StorageProvider::complete_deregister(RuntimeOrigin::signed(
                2
            )));
            assert!(Providers::<Test>::get(2).is_none());
        });
    }

    /// Directly assert the `ProviderHasPendingChallenges` rejection on
    /// `complete_deregister` while a real challenge is live (announce
    /// preconditions stamped directly, window elapsed, but the challenge still
    /// pending because the sweep has not run).
    #[test]
    fn complete_deregister_rejects_pending_challenge_error() {
        new_test_ext().execute_with(|| {
            System::set_block_number(1);
            let (mmr_root, _, _) = single_chunk_proof(b"chunk-0");
            setup_primary_with_snapshot(mmr_root, 0, 1);

            assert_ok!(StorageProvider::challenge_checkpoint(
                RuntimeOrigin::signed(3),
                0,
                2,
                ChunkLocation {
                    leaf_index: 0,
                    chunk_index: 0,
                },
            ));
            assert_eq!(PendingChallenges::<Test>::get(2), 1);

            // Stamp announce preconditions so the only remaining gate is the
            // pending-challenge counter.
            Providers::<Test>::mutate(2, |p| {
                let p = p.as_mut().unwrap();
                p.committed_bytes = 0;
                p.deregister_at = Some(101);
            });

            // Plain set_block_number (no hooks, so no sweep) keeps the challenge
            // pending. Period elapsed (101 >= 101), committed_bytes zero, so the
            // rejection is `ProviderHasPendingChallenges`.
            System::set_block_number(101);
            assert_noop!(
                StorageProvider::complete_deregister(RuntimeOrigin::signed(2)),
                Error::<Test>::ProviderHasPendingChallenges
            );
        });
    }
}
