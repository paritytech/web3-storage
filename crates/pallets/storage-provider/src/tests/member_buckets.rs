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

        let member_buckets = pallet::MemberBuckets::<Test>::get(2);
        assert!(member_buckets.is_empty());
    });
}

#[test]
fn member_buckets_index_on_bucket_delete() {
    new_test_ext().execute_with(|| {
        create_bucket(1, 0);
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

        // Delete the bucket via internal function
        assert_ok!(StorageProvider::cleanup_bucket_internal(0, &1));

        // All members should have the bucket removed from their index
        assert!(pallet::MemberBuckets::<Test>::get(1).is_empty());
        assert!(pallet::MemberBuckets::<Test>::get(2).is_empty());
        assert!(pallet::MemberBuckets::<Test>::get(3).is_empty());
    });
}

#[test]
fn cleanup_bucket_internal_pays_prorated_lifetime_revenue() {
    new_test_ext().execute_with(|| {
        register_provider_with_settings(
            2,
            200,
            ProviderSettings {
                price_per_byte: 1,
                accepting_primary: true,
                ..Default::default()
            },
        );
        let bucket_id = setup_agreement(2, 1, 10, 100); // payment = 1 * 10 * 100 = 1000

        let agreement = StorageAgreements::<Test>::get(bucket_id, 2).unwrap();
        let total_duration = agreement.expires_at - agreement.started_at;

        // Delete the bucket partway through the term (40% elapsed), so the
        // prorated split actually differs from a simple full-payment case.
        let elapsed = total_duration * 4 / 10;
        run_to_block(agreement.started_at + elapsed);

        assert_ok!(StorageProvider::cleanup_bucket_internal(bucket_id, &1));

        let remaining = total_duration - elapsed;
        let refund_to_owner = agreement.payment_locked * remaining / total_duration;
        let payment_to_provider = agreement.payment_locked - refund_to_owner;

        assert_eq!(
            Providers::<Test>::get(2).unwrap().stats.lifetime_revenue,
            payment_to_provider
        );
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
