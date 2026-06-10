use super::*;
use codec::Encode;
use sp_core::{sr25519, Pair, H256};
use storage_primitives::BucketSnapshot;

/// Register a provider using a real Sr25519 keypair so signatures can be verified on-chain.
/// Returns the keypair for later use in signing.
fn register_provider_with_keypair(who: u64, stake: u64) -> sr25519::Pair {
    use frame_support::assert_ok;
    let (pair, _) = sr25519::Pair::generate();
    let public_key: frame_support::BoundedVec<u8, frame_support::traits::ConstU32<64>> =
        pair.public().0.to_vec().try_into().unwrap();
    let multiaddr = b"/ip4/127.0.0.1/tcp/3000".to_vec();
    assert_ok!(StorageProvider::register_provider(
        RuntimeOrigin::signed(who),
        multiaddr.try_into().unwrap(),
        public_key,
        stake,
    ));
    pair
}

/// Create a signed checkpoint proposal for use in provider_checkpoint calls.
fn sign_checkpoint_proposal(
    pair: &sr25519::Pair,
    signer_account: u64,
    bucket_id: u64,
    mmr_root: H256,
    start_seq: u64,
    leaf_count: u64,
    window: u64,
) -> (u64, sp_runtime::MultiSignature) {
    let proposal = storage_primitives::CheckpointProposal::new(
        bucket_id, mmr_root, start_seq, leaf_count, window,
    );
    let encoded = proposal.encode();
    let signature = pair.sign(&encoded);
    (
        signer_account,
        sp_runtime::MultiSignature::Sr25519(signature),
    )
}

/// Setup agreement with a keypair-registered provider. Returns (bucket_id, keypair).
fn setup_agreement_with_keypair(
    provider: u64,
    client: u64,
    max_bytes: u64,
    duration: u64,
) -> (u64, sr25519::Pair) {
    let pair = register_provider_with_keypair(provider, 200);
    let bucket_id = {
        use frame_support::assert_ok;
        assert_ok!(StorageProvider::create_bucket(
            RuntimeOrigin::signed(client),
            1
        ));
        let bucket_id = crate::NextBucketId::<Test>::get() - 1;
        assert_ok!(StorageProvider::request_primary_agreement(
            RuntimeOrigin::signed(client),
            bucket_id,
            provider,
            max_bytes,
            duration,
            max_bytes * duration,
        ));
        assert_ok!(StorageProvider::accept_agreement(
            RuntimeOrigin::signed(provider),
            bucket_id,
        ));
        bucket_id
    };
    (bucket_id, pair)
}

/// Insert a snapshot directly into storage for testing checkpoint-related extrinsics.
#[allow(dead_code)]
fn insert_snapshot(bucket_id: u64, providers: &[u64]) {
    Buckets::<Test>::mutate(bucket_id, |maybe_bucket| {
        if let Some(bucket) = maybe_bucket {
            let num_bytes = providers.len().div_ceil(8);
            let mut signers = vec![0u8; num_bytes];
            for (i, _) in providers.iter().enumerate() {
                signers[i / 8] |= 1 << (i % 8);
            }
            bucket.snapshot = Some(BucketSnapshot {
                mmr_root: H256::repeat_byte(0xAB),
                start_seq: 0,
                leaf_count: 10,
                checkpoint_block: 1,
                primary_signers: signers,
            });
        }
    });
}

#[test]
fn configure_checkpoint_window_works() {
    new_test_ext().execute_with(|| {
        frame_system::Pallet::<Test>::set_block_number(1);
        create_bucket(1, 1);

        assert_ok!(StorageProvider::configure_checkpoint_window(
            RuntimeOrigin::signed(1),
            0,
            20, // interval
            10, // grace_period
            true
        ));

        let config = CheckpointConfigs::<Test>::get(0).unwrap();
        assert_eq!(config.interval, 20);
        assert_eq!(config.grace_period, 10);
        assert!(config.enabled);
    });
}

#[test]
fn configure_checkpoint_window_fails_not_admin() {
    new_test_ext().execute_with(|| {
        create_bucket(1, 1);

        assert_noop!(
            StorageProvider::configure_checkpoint_window(RuntimeOrigin::signed(3), 0, 20, 10, true),
            Error::<Test>::NotBucketAdmin
        );
    });
}

#[test]
fn configure_checkpoint_window_fails_no_bucket() {
    new_test_ext().execute_with(|| {
        assert_noop!(
            StorageProvider::configure_checkpoint_window(
                RuntimeOrigin::signed(1),
                999,
                20,
                10,
                true
            ),
            Error::<Test>::BucketNotFound
        );
    });
}

#[test]
fn fund_checkpoint_pool_works() {
    new_test_ext().execute_with(|| {
        create_bucket(1, 1);

        let free_before = Balances::free_balance(1);

        assert_ok!(StorageProvider::fund_checkpoint_pool(
            RuntimeOrigin::signed(1),
            0,
            500
        ));

        assert_eq!(CheckpointPool::<Test>::get(0), 500);
        assert_eq!(Balances::free_balance(1), free_before - 500);
    });
}

#[test]
fn fund_checkpoint_pool_fails_no_bucket() {
    new_test_ext().execute_with(|| {
        assert_noop!(
            StorageProvider::fund_checkpoint_pool(RuntimeOrigin::signed(1), 999, 500),
            Error::<Test>::BucketNotFound
        );
    });
}

#[test]
fn claim_checkpoint_rewards_works() {
    new_test_ext().execute_with(|| {
        // Seed rewards directly via storage
        CheckpointRewards::<Test>::insert(1u64, 0u64, 200u64);

        let free_before = Balances::free_balance(1);

        assert_ok!(StorageProvider::claim_checkpoint_rewards(
            RuntimeOrigin::signed(1),
            0
        ));

        assert_eq!(Balances::free_balance(1), free_before + 200);
        assert_eq!(CheckpointRewards::<Test>::get(1u64, 0u64), 0);
    });
}

#[test]
fn claim_checkpoint_rewards_fails_no_rewards() {
    new_test_ext().execute_with(|| {
        assert_noop!(
            StorageProvider::claim_checkpoint_rewards(RuntimeOrigin::signed(1), 0),
            Error::<Test>::NoRewardsToClaim
        );
    });
}

#[test]
fn checkpoint_fails_not_writer() {
    new_test_ext().execute_with(|| {
        create_bucket(1, 0);

        // Account 3 is not a member
        assert_noop!(
            StorageProvider::checkpoint(
                RuntimeOrigin::signed(3),
                0,
                H256::repeat_byte(0xAA),
                0,
                10,
                Default::default(),
            ),
            Error::<Test>::NotBucketWriter
        );
    });
}

#[test]
fn checkpoint_fails_no_bucket() {
    new_test_ext().execute_with(|| {
        assert_noop!(
            StorageProvider::checkpoint(
                RuntimeOrigin::signed(1),
                999,
                H256::repeat_byte(0xAA),
                0,
                10,
                Default::default(),
            ),
            Error::<Test>::BucketNotFound
        );
    });
}

#[test]
fn checkpoint_works_with_zero_min_providers() {
    new_test_ext().execute_with(|| {
        // With min_providers = 0, no signatures needed
        create_bucket(1, 0);

        assert_ok!(StorageProvider::checkpoint(
            RuntimeOrigin::signed(1),
            0,
            H256::repeat_byte(0xAA),
            0,
            10,
            Default::default(), // empty signatures
        ));

        let bucket = Buckets::<Test>::get(0).unwrap();
        let snapshot = bucket.snapshot.unwrap();
        assert_eq!(snapshot.mmr_root, H256::repeat_byte(0xAA));
        assert_eq!(snapshot.leaf_count, 10);
    });
}

#[test]
fn extend_checkpoint_fails_no_snapshot() {
    new_test_ext().execute_with(|| {
        create_bucket(1, 0);

        assert_noop!(
            StorageProvider::extend_checkpoint(RuntimeOrigin::signed(1), 0, Default::default(),),
            Error::<Test>::NoSnapshot
        );
    });
}

#[test]
fn extend_checkpoint_works_after_initial_checkpoint() {
    new_test_ext().execute_with(|| {
        create_bucket(1, 0);

        // First, create a snapshot with zero sigs (min_providers = 0)
        assert_ok!(StorageProvider::checkpoint(
            RuntimeOrigin::signed(1),
            0,
            H256::repeat_byte(0xAA),
            0,
            10,
            Default::default(),
        ));

        // extend_checkpoint with no additional sigs is valid (just no-ops)
        assert_ok!(StorageProvider::extend_checkpoint(
            RuntimeOrigin::signed(1),
            0,
            Default::default(),
        ));
    });
}

#[test]
fn report_missed_checkpoint_fails_within_grace() {
    new_test_ext().execute_with(|| {
        register_provider(2, 200);
        let bucket_id = setup_agreement(2, 1, 50, 500);

        // Default interval = 10, default grace = 5
        // Window 0 starts at block 0, ends at block 10
        // At block 5, we're in window 0 still, can't report window 0 as missed
        run_to_block(5);

        assert_noop!(
            StorageProvider::report_missed_checkpoint(RuntimeOrigin::signed(3), bucket_id, 0),
            Error::<Test>::InvalidCheckpointWindow
        );
    });
}

#[test]
fn report_missed_checkpoint_works() {
    new_test_ext().execute_with(|| {
        register_provider(2, 200);
        let bucket_id = setup_agreement(2, 1, 50, 500);

        // Default interval = 10, grace = 5
        // Window 0 = blocks [0, 10), window 1 = blocks [10, 20)
        // To report window 0 as missed, we need to be past window 1 start (block 10)
        // and we need current_block > window_end for window 0
        // window_end = window_start_block(0+1, 10) = 10
        // So we need current_block > 10
        run_to_block(11);

        let provider_stake_before = Providers::<Test>::get(2).unwrap().stake;
        let reporter_balance_before = Balances::free_balance(3);

        assert_ok!(StorageProvider::report_missed_checkpoint(
            RuntimeOrigin::signed(3),
            bucket_id,
            0
        ));

        // Provider stake should decrease by penalty (50)
        let provider_stake_after = Providers::<Test>::get(2).unwrap().stake;
        assert!(provider_stake_after < provider_stake_before);

        // Reporter gets 10% of penalty
        let actual_penalty = provider_stake_before - provider_stake_after;
        let reporter_reward = actual_penalty / 10;
        assert_eq!(
            Balances::free_balance(3),
            reporter_balance_before + reporter_reward
        );
    });
}

#[test]
fn provider_checkpoint_fails_disabled() {
    new_test_ext().execute_with(|| {
        register_provider(2, 200);
        let bucket_id = setup_agreement(2, 1, 50, 500);

        // Disable provider checkpoints
        assert_ok!(StorageProvider::configure_checkpoint_window(
            RuntimeOrigin::signed(1),
            bucket_id,
            10,
            5,
            false // disabled
        ));

        assert_noop!(
            StorageProvider::provider_checkpoint(
                RuntimeOrigin::signed(2),
                bucket_id,
                H256::repeat_byte(0xAA),
                0,
                10,
                0,
                Default::default(),
            ),
            Error::<Test>::ProviderCheckpointsDisabled
        );
    });
}

#[test]
fn provider_checkpoint_fails_wrong_window() {
    new_test_ext().execute_with(|| {
        register_provider(2, 200);
        let bucket_id = setup_agreement(2, 1, 50, 500);

        // At block 0, current window = 0/10 = 0
        // Trying to submit for window 5 should fail
        assert_noop!(
            StorageProvider::provider_checkpoint(
                RuntimeOrigin::signed(2),
                bucket_id,
                H256::repeat_byte(0xAA),
                0,
                10,
                5, // wrong window
                Default::default(),
            ),
            Error::<Test>::InvalidCheckpointWindow
        );
    });
}

#[test]
fn provider_checkpoint_leader_within_grace_period() {
    new_test_ext().execute_with(|| {
        let (bucket_id, pair) = setup_agreement_with_keypair(2, 1, 50, 500);

        // With only 1 provider, leader_idx = hash % 1 = 0, so provider 2 is always leader
        // Default interval=10, grace=5. Window 0 starts at block 0.
        // At block 3 we're within grace period (block <= 0 + 5).
        run_to_block(3);

        let sig = sign_checkpoint_proposal(&pair, 2, bucket_id, H256::repeat_byte(0xAA), 0, 10, 0);

        assert_ok!(StorageProvider::provider_checkpoint(
            RuntimeOrigin::signed(2),
            bucket_id,
            H256::repeat_byte(0xAA),
            0,
            10,
            0, // window 0
            vec![sig].try_into().unwrap(),
        ));

        let bucket = Buckets::<Test>::get(bucket_id).unwrap();
        let snapshot = bucket.snapshot.unwrap();
        assert_eq!(snapshot.mmr_root, H256::repeat_byte(0xAA));
        assert_eq!(snapshot.leaf_count, 10);
        assert_eq!(LastCheckpointWindow::<Test>::get(bucket_id), Some(0));
    });
}

#[test]
fn provider_checkpoint_non_leader_rejected_during_grace() {
    new_test_ext().execute_with(|| {
        let (bucket_id, pair2) = setup_agreement_with_keypair(2, 1, 50, 500);
        let pair3 = register_provider_with_keypair(3, 200);

        // Add second provider
        assert_ok!(StorageProvider::request_primary_agreement(
            RuntimeOrigin::signed(1),
            bucket_id,
            3,
            50,
            500,
            25000,
        ));
        assert_ok!(StorageProvider::accept_agreement(
            RuntimeOrigin::signed(3),
            bucket_id,
        ));

        // Within grace period, only leader can submit.
        // Determine the leader deterministically and only try the non-leader.
        run_to_block(3);

        let bucket = Buckets::<Test>::get(bucket_id).unwrap();
        let num_providers = bucket.primary_providers.len() as u32;
        // Replicate deterministic leader election: blake2_256(bucket_id || window) % num_providers
        let mut data = [0u8; 16];
        data[..8].copy_from_slice(&bucket_id.to_le_bytes());
        data[8..].copy_from_slice(&0u64.to_le_bytes()); // window = 0
        let hash = sp_io::hashing::blake2_256(&data);
        let seed = u32::from_le_bytes([hash[0], hash[1], hash[2], hash[3]]);
        let leader_idx = (seed % num_providers) as usize;
        let leader_account = bucket.primary_providers[leader_idx];

        // Try submitting as the non-leader — should fail with NotCheckpointLeader
        let (non_leader_account, non_leader_pair) = if leader_account == 2 {
            (3u64, &pair3)
        } else {
            (2u64, &pair2)
        };

        let sig = sign_checkpoint_proposal(
            non_leader_pair,
            non_leader_account,
            bucket_id,
            H256::repeat_byte(0xAA),
            0,
            10,
            0,
        );

        assert_noop!(
            StorageProvider::provider_checkpoint(
                RuntimeOrigin::signed(non_leader_account),
                bucket_id,
                H256::repeat_byte(0xAA),
                0,
                10,
                0,
                vec![sig].try_into().unwrap(),
            ),
            Error::<Test>::NotCheckpointLeader
        );
    });
}

#[test]
fn provider_checkpoint_fallback_after_grace() {
    new_test_ext().execute_with(|| {
        let (bucket_id, pair2) = setup_agreement_with_keypair(2, 1, 50, 500);
        let _pair3 = register_provider_with_keypair(3, 200);

        // Add second provider
        assert_ok!(StorageProvider::request_primary_agreement(
            RuntimeOrigin::signed(1),
            bucket_id,
            3,
            50,
            500,
            25000,
        ));
        assert_ok!(StorageProvider::accept_agreement(
            RuntimeOrigin::signed(3),
            bucket_id,
        ));

        // After grace period (block > window_start + grace = 0 + 5 = 5)
        // Any primary provider can submit (fallback)
        run_to_block(6);

        let sig2 =
            sign_checkpoint_proposal(&pair2, 2, bucket_id, H256::repeat_byte(0xAA), 0, 10, 0);

        // After grace, any primary provider can submit — provider 2 must succeed
        assert_ok!(StorageProvider::provider_checkpoint(
            RuntimeOrigin::signed(2),
            bucket_id,
            H256::repeat_byte(0xAA),
            0,
            10,
            0,
            vec![sig2].try_into().unwrap(),
        ));
        let bucket = Buckets::<Test>::get(bucket_id).unwrap();
        assert_eq!(bucket.snapshot.unwrap().mmr_root, H256::repeat_byte(0xAA));
    });
}

#[test]
fn provider_checkpoint_already_submitted() {
    new_test_ext().execute_with(|| {
        let (bucket_id, pair) = setup_agreement_with_keypair(2, 1, 50, 500);

        let sig = sign_checkpoint_proposal(&pair, 2, bucket_id, H256::repeat_byte(0xAA), 0, 10, 0);

        // Submit checkpoint for window 0
        assert_ok!(StorageProvider::provider_checkpoint(
            RuntimeOrigin::signed(2),
            bucket_id,
            H256::repeat_byte(0xAA),
            0,
            10,
            0,
            vec![sig].try_into().unwrap(),
        ));

        let sig2 = sign_checkpoint_proposal(&pair, 2, bucket_id, H256::repeat_byte(0xBB), 0, 20, 0);

        // Try to submit again for window 0
        assert_noop!(
            StorageProvider::provider_checkpoint(
                RuntimeOrigin::signed(2),
                bucket_id,
                H256::repeat_byte(0xBB),
                0,
                20,
                0,
                vec![sig2].try_into().unwrap(),
            ),
            Error::<Test>::CheckpointAlreadySubmitted
        );
    });
}

#[test]
fn provider_checkpoint_frozen_constraint() {
    new_test_ext().execute_with(|| {
        let (bucket_id, pair) = setup_agreement_with_keypair(2, 1, 50, 500);

        // Set frozen_start_seq on the bucket
        Buckets::<Test>::mutate(bucket_id, |maybe_bucket| {
            if let Some(bucket) = maybe_bucket {
                bucket.frozen_start_seq = Some(5);
            }
        });

        let sig = sign_checkpoint_proposal(&pair, 2, bucket_id, H256::repeat_byte(0xAA), 3, 10, 0);

        // Try to submit with start_seq < frozen_start_seq
        assert_noop!(
            StorageProvider::provider_checkpoint(
                RuntimeOrigin::signed(2),
                bucket_id,
                H256::repeat_byte(0xAA),
                3, // start_seq < frozen_start_seq(5)
                10,
                0,
                vec![sig].try_into().unwrap(),
            ),
            Error::<Test>::SnapshotViolatesFrozen
        );
    });
}

#[test]
fn provider_checkpoint_reward_from_pool() {
    new_test_ext().execute_with(|| {
        let (bucket_id, pair) = setup_agreement_with_keypair(2, 1, 50, 500);

        // Fund the checkpoint pool
        assert_ok!(StorageProvider::fund_checkpoint_pool(
            RuntimeOrigin::signed(1),
            bucket_id,
            500,
        ));

        let pool_before = CheckpointPool::<Test>::get(bucket_id);
        assert_eq!(pool_before, 500);

        let sig = sign_checkpoint_proposal(&pair, 2, bucket_id, H256::repeat_byte(0xAA), 0, 10, 0);

        // Submit checkpoint
        assert_ok!(StorageProvider::provider_checkpoint(
            RuntimeOrigin::signed(2),
            bucket_id,
            H256::repeat_byte(0xAA),
            0,
            10,
            0,
            vec![sig].try_into().unwrap(),
        ));

        // Pool should decrease by CheckpointReward (10)
        assert_eq!(CheckpointPool::<Test>::get(bucket_id), 500 - 10);

        // Reward should be stored in CheckpointRewards
        assert_eq!(CheckpointRewards::<Test>::get(2u64, bucket_id), 10);
    });
}

#[test]
fn provider_checkpoint_no_reward_empty_pool() {
    new_test_ext().execute_with(|| {
        let (bucket_id, pair) = setup_agreement_with_keypair(2, 1, 50, 500);

        let sig = sign_checkpoint_proposal(&pair, 2, bucket_id, H256::repeat_byte(0xAA), 0, 10, 0);

        // Don't fund pool — should still succeed but reward = 0
        assert_ok!(StorageProvider::provider_checkpoint(
            RuntimeOrigin::signed(2),
            bucket_id,
            H256::repeat_byte(0xAA),
            0,
            10,
            0,
            vec![sig].try_into().unwrap(),
        ));

        // No reward accumulated
        assert_eq!(CheckpointRewards::<Test>::get(2u64, bucket_id), 0);
    });
}

#[test]
fn provider_checkpoint_emits_event() {
    new_test_ext().execute_with(|| {
        frame_system::Pallet::<Test>::set_block_number(1);
        let (bucket_id, pair) = setup_agreement_with_keypair(2, 1, 50, 500);

        // Fund pool so we get a reward in the event
        assert_ok!(StorageProvider::fund_checkpoint_pool(
            RuntimeOrigin::signed(1),
            bucket_id,
            500,
        ));

        let sig = sign_checkpoint_proposal(&pair, 2, bucket_id, H256::repeat_byte(0xAA), 0, 10, 0);

        assert_ok!(StorageProvider::provider_checkpoint(
            RuntimeOrigin::signed(2),
            bucket_id,
            H256::repeat_byte(0xAA),
            0,
            10,
            0,
            vec![sig].try_into().unwrap(),
        ));

        let expected = RuntimeEvent::StorageProvider(crate::Event::ProviderCheckpointSubmitted {
            bucket_id,
            mmr_root: H256::repeat_byte(0xAA),
            window: 0,
            leader: 2,
            signers: vec![2],
            reward: 10,
        });
        assert!(frame_system::Pallet::<Test>::events()
            .iter()
            .any(|r| r.event == expected));
    });
}

#[test]
fn report_missed_checkpoint_emits_event() {
    new_test_ext().execute_with(|| {
        frame_system::Pallet::<Test>::set_block_number(1);
        register_provider(2, 200);
        let bucket_id = setup_agreement(2, 1, 50, 500);

        // Advance past window 0 + grace
        run_to_block(11);

        assert_ok!(StorageProvider::report_missed_checkpoint(
            RuntimeOrigin::signed(3),
            bucket_id,
            0,
        ));

        // Penalty = CheckpointMissPenalty(50) or provider's stake if less
        let events = frame_system::Pallet::<Test>::events();
        let found = events.iter().any(|r| {
            matches!(
                &r.event,
                RuntimeEvent::StorageProvider(crate::Event::CheckpointMissPenalized {
                    bucket_id: bid,
                    provider: 2,
                    window: 0,
                    ..
                }) if *bid == bucket_id
            )
        });
        assert!(found);
    });
}

#[test]
fn report_missed_checkpoint_fails_already_submitted() {
    new_test_ext().execute_with(|| {
        let (bucket_id, pair) = setup_agreement_with_keypair(2, 1, 50, 500);

        let sig = sign_checkpoint_proposal(&pair, 2, bucket_id, H256::repeat_byte(0xAA), 0, 10, 0);

        // Submit checkpoint for window 0
        assert_ok!(StorageProvider::provider_checkpoint(
            RuntimeOrigin::signed(2),
            bucket_id,
            H256::repeat_byte(0xAA),
            0,
            10,
            0,
            vec![sig].try_into().unwrap(),
        ));

        // Advance past window 0
        run_to_block(11);

        // Try to report window 0 as missed — but it was submitted
        assert_noop!(
            StorageProvider::report_missed_checkpoint(RuntimeOrigin::signed(3), bucket_id, 0),
            Error::<Test>::CheckpointAlreadySubmitted
        );
    });
}
