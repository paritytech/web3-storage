use super::*;
use sp_core::H256;
use storage_primitives::BucketSnapshot;

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
        assert_ok!(StorageProvider::create_bucket(RuntimeOrigin::signed(1), 1));

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
        assert_ok!(StorageProvider::create_bucket(RuntimeOrigin::signed(1), 1));

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
        assert_ok!(StorageProvider::create_bucket(RuntimeOrigin::signed(1), 1));

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
        assert_ok!(StorageProvider::create_bucket(RuntimeOrigin::signed(1), 0));

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
        assert_ok!(StorageProvider::create_bucket(RuntimeOrigin::signed(1), 0));

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
        assert_ok!(StorageProvider::create_bucket(RuntimeOrigin::signed(1), 0));

        assert_noop!(
            StorageProvider::extend_checkpoint(RuntimeOrigin::signed(1), 0, Default::default(),),
            Error::<Test>::NoSnapshot
        );
    });
}

#[test]
fn extend_checkpoint_works_after_initial_checkpoint() {
    new_test_ext().execute_with(|| {
        assert_ok!(StorageProvider::create_bucket(RuntimeOrigin::signed(1), 0));

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
