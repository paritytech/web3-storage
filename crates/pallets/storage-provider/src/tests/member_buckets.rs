// SPDX-License-Identifier: Apache-2.0

use super::*;

#[test]
fn member_buckets_index_on_create() {
    new_test_ext().execute_with(|| {
        create_bucket(1, 1);
        create_bucket(1, 1);

        let member_buckets = pallet::MemberBuckets::<Test>::get(1);
        assert_eq!(member_buckets.to_vec(), vec![0, 1]);
    });
}

#[test]
fn member_buckets_index_on_set_member() {
    new_test_ext().execute_with(|| {
        create_bucket(1, 1);

        // Add account 2 as writer
        assert_ok!(StorageProvider::set_member(
            RuntimeOrigin::signed(1),
            0,
            2,
            Role::Writer
        ));

        let member_buckets = pallet::MemberBuckets::<Test>::get(2);
        assert_eq!(member_buckets.to_vec(), vec![0]);

        // Updating role (not a new member) should not duplicate entry
        assert_ok!(StorageProvider::set_member(
            RuntimeOrigin::signed(1),
            0,
            2,
            Role::Reader
        ));

        let member_buckets = pallet::MemberBuckets::<Test>::get(2);
        assert_eq!(member_buckets.to_vec(), vec![0]);
    });
}

#[test]
fn member_buckets_index_on_remove_member() {
    new_test_ext().execute_with(|| {
        create_bucket(1, 1);
        assert_ok!(StorageProvider::set_member(
            RuntimeOrigin::signed(1),
            0,
            2,
            Role::Writer
        ));

        // Remove account 2
        assert_ok!(StorageProvider::remove_member(
            RuntimeOrigin::signed(1),
            0,
            2
        ));

        // The member deposit paid for this entry; once released, nothing
        // pays for an empty vector, so the key must be gone.
        assert!(!pallet::MemberBuckets::<Test>::contains_key(2));
    });
}

#[test]
fn member_buckets_index_on_bucket_delete() {
    new_test_ext().execute_with(|| {
        create_bucket(1, 1);
        assert_ok!(StorageProvider::set_member(
            RuntimeOrigin::signed(1),
            0,
            2,
            Role::Writer
        ));
        assert_ok!(StorageProvider::set_member(
            RuntimeOrigin::signed(1),
            0,
            3,
            Role::Reader
        ));

        assert_ok!(StorageProvider::delete_bucket(RuntimeOrigin::signed(1), 0));

        // All members should have the bucket removed from their index, and
        // nobody pays for an empty entry, so the keys must be gone.
        assert!(!pallet::MemberBuckets::<Test>::contains_key(1));
        assert!(!pallet::MemberBuckets::<Test>::contains_key(2));
        assert!(!pallet::MemberBuckets::<Test>::contains_key(3));
    });
}

#[test]
fn member_buckets_multi_membership() {
    new_test_ext().execute_with(|| {
        // Create 3 buckets owned by different accounts
        create_bucket(1, 1);
        create_bucket(2, 1);
        create_bucket(3, 1);

        // Add account 4 to all 3 buckets
        assert_ok!(StorageProvider::set_member(
            RuntimeOrigin::signed(1),
            0,
            4,
            Role::Writer
        ));
        assert_ok!(StorageProvider::set_member(
            RuntimeOrigin::signed(2),
            1,
            4,
            Role::Reader
        ));
        assert_ok!(StorageProvider::set_member(
            RuntimeOrigin::signed(3),
            2,
            4,
            Role::Admin
        ));

        let member_buckets = pallet::MemberBuckets::<Test>::get(4);
        assert_eq!(member_buckets.to_vec(), vec![0, 1, 2]);

        // Remove from bucket 1 only
        assert_ok!(StorageProvider::remove_member(
            RuntimeOrigin::signed(2),
            1,
            4
        ));

        let member_buckets = pallet::MemberBuckets::<Test>::get(4);
        assert_eq!(member_buckets.to_vec(), vec![0, 2]);
    });
}
