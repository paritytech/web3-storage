// SPDX-License-Identifier: Apache-2.0

use super::*;
use sp_core::H256;
use storage_primitives::Commitment;

#[test]
fn checkpoint_fails_not_writer() {
    new_test_ext().execute_with(|| {
        create_bucket(1, 0);

        // Account 3 is not a member
        assert_noop!(
            StorageProvider::checkpoint(
                RuntimeOrigin::signed(3),
                0,
                Commitment {
                    mmr_root: H256::repeat_byte(0xAA),
                    start_seq: 0,
                    leaf_count: 10,
                },
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
                Commitment {
                    mmr_root: H256::repeat_byte(0xAA),
                    start_seq: 0,
                    leaf_count: 10,
                },
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
            Commitment {
                mmr_root: H256::repeat_byte(0xAA),
                start_seq: 0,
                leaf_count: 10,
            },
            Default::default(), // empty signatures
        ));

        let bucket = Buckets::<Test>::get(0).unwrap();
        let snapshot = bucket.snapshot.unwrap();
        assert_eq!(snapshot.commitment.mmr_root, H256::repeat_byte(0xAA));
        assert_eq!(snapshot.commitment.leaf_count, 10);
    });
}

#[test]
fn checkpoint_records_the_commitment_in_historical_roots() {
    new_test_ext().execute_with(|| {
        create_bucket(1, 0);
        // Anchor block 7 crosses the prime-3 and prime-7 boundaries only.
        System::set_block_number(7);
        let commitment = Commitment {
            mmr_root: H256::repeat_byte(0xAA),
            start_seq: 4,
            leaf_count: 10,
        };
        assert_ok!(StorageProvider::checkpoint(
            RuntimeOrigin::signed(1),
            0,
            commitment,
            Default::default(),
        ));

        let roots = Buckets::<Test>::get(0).unwrap().historical_roots;
        // The whole commitment is stored, so a replica syncing to this root
        // later records a range challenge_replica can bind to.
        assert_eq!(roots[0], (2, commitment));
        assert_eq!(roots[1], (1, commitment));
        assert_eq!(roots[2], (0, Commitment::default()));
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
            Commitment {
                mmr_root: H256::repeat_byte(0xAA),
                start_seq: 0,
                leaf_count: 10,
            },
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

/// Submit a checkpoint for bucket 0 as account 1 (admin), with no signatures.
/// The test buckets use `min_providers = 0`, so no signature is needed.
fn checkpoint_range(root: u8, start_seq: u64, leaf_count: u64) -> sp_runtime::DispatchResult {
    StorageProvider::checkpoint(
        RuntimeOrigin::signed(1),
        0,
        Commitment {
            mmr_root: H256::repeat_byte(root),
            start_seq,
            leaf_count,
        },
        Default::default(),
    )
}

#[test]
fn checkpoint_rejects_end_moving_back() {
    new_test_ext().execute_with(|| {
        create_bucket(1, 0);
        assert_ok!(checkpoint_range(0xAA, 0, 20));

        // 0..20 then 0..10: the end moves back from 20 to 10.
        assert_noop!(
            checkpoint_range(0xBB, 0, 10),
            Error::<Test>::SnapshotNotForward
        );
    });
}

#[test]
fn checkpoint_rejects_start_moving_back() {
    new_test_ext().execute_with(|| {
        create_bucket(1, 0);
        assert_ok!(checkpoint_range(0xAA, 4, 16));

        // 4..20 then 1..25: the end advances, but the start moves back from 4 to 1.
        assert_noop!(
            checkpoint_range(0xBB, 1, 24),
            Error::<Test>::SnapshotNotForward
        );
    });
}

#[test]
fn checkpoint_on_frozen_bucket_rejects_start_moving_forward() {
    new_test_ext().execute_with(|| {
        create_bucket(1, 0);
        assert_ok!(checkpoint_range(0xAA, 4, 16));
        assert_ok!(StorageProvider::freeze_bucket(RuntimeOrigin::signed(1), 0));

        // Frozen at start 4. 6..25 deletes leaves 4 and 5 from a frozen bucket.
        assert_noop!(
            checkpoint_range(0xBB, 6, 19),
            Error::<Test>::SnapshotViolatesFrozen
        );
    });
}

#[test]
fn checkpoint_rejects_same_range() {
    new_test_ext().execute_with(|| {
        create_bucket(1, 0);
        assert_ok!(checkpoint_range(0xAA, 0, 20));

        // 0..20 again with a different root: no new leaves, rewritten content.
        assert_noop!(
            checkpoint_range(0xBB, 0, 20),
            Error::<Test>::SnapshotNotForward
        );
        // The same commitment again adds nothing either.
        assert_noop!(
            checkpoint_range(0xAA, 0, 20),
            Error::<Test>::SnapshotNotForward
        );
    });
}

#[test]
fn checkpoint_accepts_append() {
    new_test_ext().execute_with(|| {
        create_bucket(1, 0);
        assert_ok!(checkpoint_range(0xAA, 0, 20));
        assert_ok!(checkpoint_range(0xBB, 0, 25));

        let commitment = Buckets::<Test>::get(0)
            .unwrap()
            .snapshot
            .unwrap()
            .commitment;
        assert_eq!((commitment.start_seq, commitment.leaf_count), (0, 25));
    });
}

#[test]
fn checkpoint_accepts_deletion_that_keeps_the_end() {
    new_test_ext().execute_with(|| {
        create_bucket(1, 0);
        assert_ok!(checkpoint_range(0xAA, 0, 15));

        // Deleting leaves 0..10 moves the start forward and keeps the end at 15.
        assert_ok!(checkpoint_range(0xBB, 10, 5));
        // After the deletion the end still must not move back.
        assert_noop!(
            checkpoint_range(0xCC, 12, 2),
            Error::<Test>::SnapshotNotForward
        );
        // Deleting every leaf leaves an empty range that ends at 15.
        assert_ok!(checkpoint_range(0xDD, 15, 0));
    });
}

#[test]
fn checkpoint_accepts_deletion_with_append() {
    new_test_ext().execute_with(|| {
        create_bucket(1, 0);
        assert_ok!(checkpoint_range(0xAA, 0, 15));

        // 0..15 then 10..20: leaves 0..10 deleted and five new leaves appended.
        assert_ok!(checkpoint_range(0xBB, 10, 10));
    });
}

#[test]
fn checkpoint_on_frozen_bucket_accepts_append_only() {
    new_test_ext().execute_with(|| {
        create_bucket(1, 0);
        assert_ok!(checkpoint_range(0xAA, 4, 16));
        assert_ok!(StorageProvider::freeze_bucket(RuntimeOrigin::signed(1), 0));

        assert_ok!(checkpoint_range(0xBB, 4, 21));
        // The frozen start is kept, but the end does not advance.
        assert_noop!(
            checkpoint_range(0xCC, 4, 21),
            Error::<Test>::SnapshotNotForward
        );
    });
}
