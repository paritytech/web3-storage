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
