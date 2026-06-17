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
fn advance_snapshot_root(bucket_id: u64) {
    Buckets::<Test>::mutate(bucket_id, |maybe_bucket| {
        let bucket = maybe_bucket.as_mut().expect("bucket exists");
        bucket.snapshot = Some(BucketSnapshot {
            mmr_root: H256::repeat_byte(0xCD),
            start_seq: 0,
            leaf_count: 10,
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
                    commitment_nonce: 0,
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
                0, // nonce
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

        // Advance the canonical snapshot to a NEW root so the challenged
        // commitment (root 0xAB) is genuinely superseded. The challenged seq 0
        // still sits inside the new canonical range 0..10, so `Superseded` is a
        // valid defense.
        Buckets::<Test>::mutate(bucket_id, |maybe_bucket| {
            let bucket = maybe_bucket.as_mut().unwrap();
            bucket.snapshot = Some(BucketSnapshot {
                mmr_root: H256::repeat_byte(0xCD),
                start_seq: 0,
                leaf_count: 10,
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

        advance_snapshot_root(bucket_id);

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

        advance_snapshot_root(bucket_id);

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

        advance_snapshot_root(bucket_id);

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

        advance_snapshot_root(bucket_id);

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
                    commitment_nonce: 0,
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
            0,
            0,
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
                mmr_root: H256::repeat_byte(0xCD),
                start_seq: 0,
                leaf_count: 10,
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
    use sp_core::H256;
    use storage_primitives::{
        blake2_256, BucketSnapshot, ChallengeId, ChallengerStatRecord, MerkleProof, MmrLeaf,
        MmrProof, ProviderRole, ReplicaSyncRecord,
    };

    // ─────────────────────────────────────────────────────────────────────────
    // Helpers
    // ─────────────────────────────────────────────────────────────────────────

    /// Run blocks forward, invoking both system and pallet hooks each block so
    /// `on_finalize` for the storage provider fires (the default `run_to_block`
    /// in `mock.rs` only invokes system hooks).
    fn advance_to(n: u64) {
        while System::block_number() < n {
            <StorageProvider as Hooks<u64>>::on_finalize(System::block_number());
            <System as Hooks<u64>>::on_finalize(System::block_number());
            System::set_block_number(System::block_number() + 1);
            <System as Hooks<u64>>::on_initialize(System::block_number());
            <StorageProvider as Hooks<u64>>::on_initialize(System::block_number());
        }
    }

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
            mmr_root,
            start_seq,
            leaf_count,
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
                0, // bucket_id
                2, // provider
                0, // leaf_index
                0, // chunk_index
            ));

            // Challenge stored at deadline = block(1) + ChallengeTimeout(100) = 101.
            let challenges = Challenges::<Test>::get(101).expect("challenge present");
            assert_eq!(challenges.len(), 1);
            let challenge = &challenges[0];
            assert_eq!(challenge.provider, 2);
            assert_eq!(challenge.challenger, 3);
            assert_eq!(challenge.mmr_root, mmr_root);
            // Currently a fixed `100u32`; commit 4 will change this.
            assert_eq!(challenge.deposit, 100);

            // Provider stats reflect received challenge.
            let provider = Providers::<Test>::get(2).unwrap();
            assert_eq!(provider.stats.challenges_received, 1);
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
                StorageProvider::challenge_checkpoint(RuntimeOrigin::signed(3), bucket_id, 2, 0, 0),
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
                StorageProvider::challenge_checkpoint(RuntimeOrigin::signed(3), 0, 5, 0, 0),
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
                0,
                0,
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
            assert!(Challenges::<Test>::get(101).is_none());
            // Defended path slashes a fraction of the deposit from the
            // provider's stake based on response time. At block 1 (challenge
            // also created at block 1) the response is within "block 1" → 10%
            // of the 100-deposit = 10 deducted, stake drops 200 → 190.
            let provider = Providers::<Test>::get(2).unwrap();
            assert_eq!(provider.stake, 190);
            assert_eq!(provider.stats.challenges_failed, 0);
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
                0,
                0,
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
                0,
                0,
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
            assert!(Challenges::<Test>::get(101).is_none());
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
                0,
                0,
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
                5,
                0,
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
                0,
                0,
            ));

            // Bump snapshot to cover seq 0..10 — the challenged seq 0 is now
            // within the canonical window, so `Superseded` is valid.
            Buckets::<Test>::mutate(0u64, |bucket| {
                let bucket = bucket.as_mut().unwrap();
                bucket.snapshot = Some(BucketSnapshot {
                    mmr_root: H256::repeat_byte(0x77),
                    start_seq: 0,
                    leaf_count: 10,
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
            assert!(Challenges::<Test>::get(101).is_none());
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
                0,
                0,
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
                0,
                0,
            ));

            // Advance the snapshot: new root (so (a) passes) but start_seq
            // rolled past the challenged seq 0, so the canonical range is
            // 5..15 and seq 0 is below the front.
            Buckets::<Test>::mutate(0u64, |bucket| {
                let bucket = bucket.as_mut().unwrap();
                bucket.snapshot = Some(BucketSnapshot {
                    mmr_root: H256::repeat_byte(0x77),
                    start_seq: 5,
                    leaf_count: 10,
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
                0,
                0,
            ));

            // Walk forward one block past the deadline. (Don't invoke pallet
            // hooks here — we want to observe the rejection branch in
            // `respond_to_challenge`, not the timeout slashing in `on_finalize`.)
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
    // Timeout slashing via on_finalize
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
                0,
                0,
            ));
            assert_eq!(Balances::reserved_balance(3), 100);

            // Provider has stake of 200 reserved at registration.
            assert_eq!(Balances::reserved_balance(2), 200);

            // Advance one block past the deadline; `on_finalize(101)` slashes.
            advance_to(102);

            // Provider stake is zero post-slash.
            let provider = Providers::<Test>::get(2).unwrap();
            assert_eq!(provider.stake, 0);
            assert_eq!(provider.stats.challenges_failed, 1);

            // Challenger deposit is unreserved and they receive a 10% reward of
            // the 200-stake slash = 20.
            assert_eq!(Balances::reserved_balance(3), 0);
            assert_eq!(Balances::free_balance(3), challenger_before + 20);

            // Challenge cleared from storage.
            assert!(Challenges::<Test>::get(101).is_none());
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
                        mmr_root: last_root,
                        start_seq: 7, // commit 4: replica's challenge_replica now uses this
                        leaf_count: 3,
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
                0, // bucket
                4, // replica provider
                0, // leaf
                0, // chunk
            ));

            let challenges = Challenges::<Test>::get(101).expect("created");
            assert_eq!(challenges[0].mmr_root, last_root);
            // start_seq now reflects the value captured at sync time
            // (previously hardcoded 0u64 placeholder).
            assert_eq!(challenges[0].start_seq, 7);
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
                    2, // primary
                    0,
                    0,
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
                    0, // bucket_id
                    2, // provider
                    mmr_root,
                    0, // start_seq
                    1, // leaf_count
                    0, // leaf_index
                    0, // chunk_index
                    1, // nonce — block 1 is 499 blocks behind, > MaxNonceAge=200
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
                    mmr_root,
                    0,
                    1,
                    0,
                    0,
                    9999, // nonce in the future
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
                    0, // bucket
                    H256::zero(),
                    0, // start_seq
                    0, // leaf_count
                    1, // stale nonce
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
                StorageProvider::challenge_replica(RuntimeOrigin::signed(3), bucket_id, 4, 0, 0,),
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
                0,
                0,
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
                0,
                0,
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
            assert_eq!(stats.total_earnings, 0);
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
                0,
                0,
            ));
            advance_to(102);
            let stats = ChallengerStats::<Test>::get(3);
            assert_eq!(stats.total_challenges, 1);
            assert_eq!(stats.successful_challenges, 1);
            assert_eq!(stats.failed_challenges, 0);
            // 10% of provider stake (200) = 20 reward.
            assert_eq!(stats.total_earnings, 20);
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
                0,
                0,
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
            assert_eq!(stats.total_earnings, 20);
        });
    }
}
