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
                0, // start_seq
                0, // leaf_count
                0, // leaf_index
                0, // chunk_index
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
fn respond_to_challenge_proof_binds_leaf_index() {
    use codec::Encode;
    use storage_primitives::{blake2_256, hash_children, MerkleProof, MmrLeaf, MmrProof};

    new_test_ext().execute_with(|| {
        frame_system::Pallet::<Test>::set_block_number(1);

        // Build a real 2-leaf MMR by hand. Each "file" is a single chunk, so
        // data_root == blake2_256(chunk) and the chunk proof is empty.
        let chunk0 = b"file0".to_vec();
        let chunk1 = b"file1".to_vec();
        let leaf0 = MmrLeaf {
            data_root: blake2_256(&chunk0),
            data_size: 5,
            total_size: 5,
        };
        let leaf1 = MmrLeaf {
            data_root: blake2_256(&chunk1),
            data_size: 5,
            total_size: 10,
        };
        let l0_hash = blake2_256(&leaf0.encode());
        let l1_hash = blake2_256(&leaf1.encode());
        let root = hash_children(l0_hash, l1_hash); // single peak over 2 leaves

        // Register provider 2, an agreement, and a snapshot committing to `root`.
        register_provider(2, 200);
        let bucket_id = setup_agreement(2, 1, 50, 200);
        Buckets::<Test>::mutate(bucket_id, |maybe_bucket| {
            if let Some(bucket) = maybe_bucket {
                bucket.snapshot = Some(BucketSnapshot {
                    mmr_root: root,
                    start_seq: 0,
                    leaf_count: 2,
                    checkpoint_block: 1,
                    primary_signers: vec![0x01],
                });
            }
        });

        // Challenge leaf_index = 1.
        assert_ok!(StorageProvider::challenge_checkpoint(
            RuntimeOrigin::signed(3),
            bucket_id,
            2,
            1, // leaf_index
            0, // chunk_index
        ));
        let challenge_id = ChallengeId {
            deadline: 101,
            index: 0,
        };
        // The snapshot's leaf_count is bound onto the stored challenge (mirrors the
        // replica test's assertion), so the checkpoint→leaf_count wiring is pinned
        // directly, not only via the proof outcome.
        assert_eq!(Challenges::<Test>::get(101).unwrap()[0].leaf_count, 2);
        let empty_chunk_proof = MerkleProof {
            siblings: vec![],
            path: vec![],
        };

        // SUBSTITUTION: answer the leaf-1 challenge with leaf 0's proof + data.
        // Rejected — the proof's path does not match leaf_index 1.
        let substituted = crate::ChallengeResponse::Proof {
            chunk_data: chunk0.clone().try_into().unwrap(),
            mmr_proof: MmrProof {
                peaks: vec![root],
                leaf: leaf0.clone(),
                leaf_proof: MerkleProof {
                    siblings: vec![l1_hash],
                    path: vec![false],
                },
            },
            chunk_proof: empty_chunk_proof.clone(),
        };
        assert_noop!(
            StorageProvider::respond_to_challenge(
                RuntimeOrigin::signed(2),
                challenge_id,
                substituted
            ),
            Error::<Test>::InvalidChallengeProof
        );
        // The challenge is still pending after the rejected response.
        assert_eq!(Challenges::<Test>::get(101).unwrap().len(), 1);

        // HONEST: answer with leaf 1's own proof + data. Defends successfully.
        let honest = crate::ChallengeResponse::Proof {
            chunk_data: chunk1.clone().try_into().unwrap(),
            mmr_proof: MmrProof {
                peaks: vec![root],
                leaf: leaf1.clone(),
                leaf_proof: MerkleProof {
                    siblings: vec![l0_hash],
                    path: vec![true],
                },
            },
            chunk_proof: empty_chunk_proof,
        };
        assert_ok!(StorageProvider::respond_to_challenge(
            RuntimeOrigin::signed(2),
            challenge_id,
            honest,
        ));
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
    use codec::Encode;
    use sp_core::Pair;
    use storage_primitives::CommitmentPayload;

    new_test_ext().execute_with(|| {
        frame_system::Pallet::<Test>::set_block_number(1);
        let bucket_id = setup_with_snapshot(2, 1); // current snapshot: start_seq=0, leaf_count=10
        let pair = provider_signer(2);

        // Off-chain challenge over a commitment claiming leaf_count = 20, at leaf 15.
        // The signed leaf_count (20) makes leaf 15 exist for the creation guard, but
        // the challenge is against a commitment beyond the *current* canonical range.
        // (challenge_checkpoint can no longer reach this: its leaf_index is bound by
        // the snapshot's own leaf_count via the create_challenge guard.)
        let root = H256::repeat_byte(0xEE);
        let payload = CommitmentPayload::new(bucket_id, root, 0, 20);
        let sig = sp_runtime::MultiSignature::Sr25519(pair.sign(&payload.encode()));
        assert_ok!(StorageProvider::challenge_offchain(
            RuntimeOrigin::signed(3),
            bucket_id,
            2,
            root,
            0,  // start_seq
            20, // leaf_count (signed)
            15, // leaf_index
            0,  // chunk_index
            sig,
        ));

        let challenge_id = ChallengeId {
            deadline: 101,
            index: 0,
        };

        // Superseded is checked against the CURRENT snapshot: canonical_end = 0 + 10 = 10,
        // challenged_seq = 0 + 15 = 15, and 15 < 10 is false → LeafBeyondCanonical.
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

#[test]
fn challenge_rejects_out_of_range_leaf_index() {
    new_test_ext().execute_with(|| {
        frame_system::Pallet::<Test>::set_block_number(1);
        let bucket_id = setup_with_snapshot(2, 1); // snapshot leaf_count = 10

        // leaf_index == leaf_count (and beyond) is out of range: no valid proof
        // could ever defend it, so it is rejected at creation rather than resolved
        // by slashing the provider on timeout.
        assert_noop!(
            StorageProvider::challenge_checkpoint(RuntimeOrigin::signed(3), bucket_id, 2, 10, 0),
            Error::<Test>::LeafIndexOutOfRange
        );
        assert_noop!(
            StorageProvider::challenge_checkpoint(RuntimeOrigin::signed(3), bucket_id, 2, 99, 0),
            Error::<Test>::LeafIndexOutOfRange
        );
        // The last in-range leaf (leaf_count - 1) is accepted.
        assert_ok!(StorageProvider::challenge_checkpoint(
            RuntimeOrigin::signed(3),
            bucket_id,
            2,
            9,
            0,
        ));
    });
}

#[test]
fn challenge_offchain_binds_signed_leaf_count() {
    use codec::Encode;
    use sp_core::Pair;
    use storage_primitives::{
        blake2_256, hash_children, CommitmentPayload, MerkleProof, MmrLeaf, MmrProof,
    };

    new_test_ext().execute_with(|| {
        frame_system::Pallet::<Test>::set_block_number(1);
        // Provider 2 with a real keypair and an agreement on the bucket.
        register_provider(2, 200);
        let bucket_id = setup_agreement(2, 1, 50, 200);
        let pair = provider_signer(2);

        // A real 2-leaf MMR (single-chunk leaves).
        let chunk0 = b"file0".to_vec();
        let chunk1 = b"file1".to_vec();
        let leaf0 = MmrLeaf {
            data_root: blake2_256(&chunk0),
            data_size: 5,
            total_size: 5,
        };
        let leaf1 = MmrLeaf {
            data_root: blake2_256(&chunk1),
            data_size: 5,
            total_size: 10,
        };
        let l0 = blake2_256(&leaf0.encode());
        let l1 = blake2_256(&leaf1.encode());
        let root = hash_children(l0, l1);
        let leaf_count = 2u64;

        // Provider signs the commitment over the REAL leaf_count.
        let payload = CommitmentPayload::new(bucket_id, root, 0, leaf_count);
        let sig = sp_runtime::MultiSignature::Sr25519(pair.sign(&payload.encode()));

        // Challenge leaf 1 off-chain; the signed leaf_count is bound onto the challenge.
        assert_ok!(StorageProvider::challenge_offchain(
            RuntimeOrigin::signed(3),
            bucket_id,
            2,
            root,
            0,          // start_seq
            leaf_count, // leaf_count (must match the signed payload)
            1,          // leaf_index
            0,          // chunk_index
            sig,
        ));
        let challenge_id = ChallengeId {
            deadline: 101,
            index: 0,
        };
        assert_eq!(Challenges::<Test>::get(101).unwrap()[0].leaf_count, 2);

        let empty = MerkleProof {
            siblings: vec![],
            path: vec![],
        };

        // Substituted leaf-0 proof is rejected for the leaf-1 challenge.
        assert_noop!(
            StorageProvider::respond_to_challenge(
                RuntimeOrigin::signed(2),
                challenge_id,
                crate::ChallengeResponse::Proof {
                    chunk_data: chunk0.clone().try_into().unwrap(),
                    mmr_proof: MmrProof {
                        peaks: vec![root],
                        leaf: leaf0.clone(),
                        leaf_proof: MerkleProof {
                            siblings: vec![l1],
                            path: vec![false],
                        },
                    },
                    chunk_proof: empty.clone(),
                },
            ),
            Error::<Test>::InvalidChallengeProof
        );

        // Honest leaf-1 proof defends.
        assert_ok!(StorageProvider::respond_to_challenge(
            RuntimeOrigin::signed(2),
            challenge_id,
            crate::ChallengeResponse::Proof {
                chunk_data: chunk1.clone().try_into().unwrap(),
                mmr_proof: MmrProof {
                    peaks: vec![root],
                    leaf: leaf1.clone(),
                    leaf_proof: MerkleProof {
                        siblings: vec![l0],
                        path: vec![true],
                    },
                },
                chunk_proof: empty,
            },
        ));
        assert!(Challenges::<Test>::get(101).is_none());
    });
}

#[test]
fn challenge_offchain_rejects_leaf_count_not_signed() {
    use codec::Encode;
    use sp_core::Pair;
    use storage_primitives::CommitmentPayload;

    new_test_ext().execute_with(|| {
        frame_system::Pallet::<Test>::set_block_number(1);
        register_provider(2, 200);
        let bucket_id = setup_agreement(2, 1, 50, 200);
        let pair = provider_signer(2);

        // Provider signs leaf_count = 2 ...
        let root = H256::repeat_byte(0xAB);
        let payload = CommitmentPayload::new(bucket_id, root, 0, 2);
        let sig = sp_runtime::MultiSignature::Sr25519(pair.sign(&payload.encode()));

        // ... but the challenger supplies leaf_count = 3, so the signature is over a
        // different payload and the challenge is rejected (leaf_count can't be forged).
        assert_noop!(
            StorageProvider::challenge_offchain(
                RuntimeOrigin::signed(3),
                bucket_id,
                2,
                root,
                0, // start_seq
                3, // leaf_count (NOT what was signed)
                1, // leaf_index
                0, // chunk_index
                sig,
            ),
            Error::<Test>::InvalidSignature
        );
    });
}
