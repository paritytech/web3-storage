// SPDX-License-Identifier: Apache-2.0

use super::*;

#[test]
fn create_bucket_works() {
    new_test_ext().execute_with(|| {
        create_bucket(1, 2);

        let bucket = Buckets::<Test>::get(0).unwrap();
        assert_eq!(bucket.min_providers, 2);
        assert_eq!(bucket.members.len(), 1);
        assert_eq!(bucket.members[0].account, 1);
        assert_eq!(bucket.members[0].role, Role::Admin);
        assert!(bucket.primary_providers.is_empty());
        assert_eq!(bucket.visibility, storage_primitives::Visibility::Public);
        assert!(bucket.snapshot.is_none());
        assert!(bucket.frozen_start_seq.is_none());
        assert_eq!(bucket.creator, 1);

        // Check bucket ID incremented
        assert_eq!(NextBucketId::<Test>::get(), 1);
    });
}

/// The bucket record and the creator's own member entry stay in state until
/// someone removes them, so the creator pays a deposit for each; without one,
/// buckets would be free to spam.
#[test]
fn create_bucket_holds_a_storage_deposit_on_the_creator() {
    new_test_ext().execute_with(|| {
        let free_before = Balances::free_balance(1);

        create_bucket(1, 0);

        assert_eq!(held(HoldReason::StorageDeposit, 1), 20);
        assert_eq!(Balances::free_balance(1), free_before - 20);
    });
}

/// A member entry is a record the adding admin chose to create, so the admin
/// pays for it: members must not be able to be charged by someone else's
/// action, and a role change creates no new record.
#[test]
fn set_member_holds_the_deposit_on_the_adding_admin_not_the_member() {
    new_test_ext().execute_with(|| {
        let bucket_id = create_bucket(1, 0);
        let admin_held = held(HoldReason::StorageDeposit, 1);

        assert_ok!(StorageProvider::set_member(
            RuntimeOrigin::signed(1),
            bucket_id,
            2,
            Role::Writer
        ));
        assert_eq!(held(HoldReason::StorageDeposit, 1), admin_held + 10);
        assert_eq!(held(HoldReason::StorageDeposit, 2), 0);

        assert_ok!(StorageProvider::set_member(
            RuntimeOrigin::signed(1),
            bucket_id,
            2,
            Role::Reader
        ));
        assert_eq!(held(HoldReason::StorageDeposit, 1), admin_held + 10);

        let member = Buckets::<Test>::get(bucket_id).unwrap().members[1].clone();
        assert_eq!(member.account, 2);
        assert_eq!(member.depositor, 1);
    });
}

/// An admin that cannot fund the deposit cannot create the entry; the hold
/// fails before any state is written.
#[test]
fn set_member_fails_without_funds_for_the_deposit() {
    new_test_ext().execute_with(|| {
        let bucket_id = create_bucket(1, 0);
        assert_ok!(StorageProvider::set_member(
            RuntimeOrigin::signed(1),
            bucket_id,
            9,
            Role::Admin
        ));
        assert_ok!(Balances::force_set_balance(RuntimeOrigin::root(), 9, 5));

        assert_noop!(
            StorageProvider::set_member(RuntimeOrigin::signed(9), bucket_id, 7, Role::Reader),
            sp_runtime::TokenError::FundsUnavailable
        );
    });
}

/// The refund goes to whoever paid, whatever admin performs the removal and
/// whether the removed member is the depositor itself.
#[test]
fn removing_a_member_refunds_the_admin_who_added_them() {
    new_test_ext().execute_with(|| {
        let bucket_id = create_bucket(1, 0);
        assert_ok!(StorageProvider::set_member(
            RuntimeOrigin::signed(1),
            bucket_id,
            2,
            Role::Writer
        ));
        assert_ok!(StorageProvider::set_member(
            RuntimeOrigin::signed(1),
            bucket_id,
            3,
            Role::Admin
        ));
        let depositor_held = held(HoldReason::StorageDeposit, 1);
        let other_admin_free = Balances::free_balance(3);

        assert_ok!(StorageProvider::remove_member(
            RuntimeOrigin::signed(3),
            bucket_id,
            2
        ));
        assert_eq!(held(HoldReason::StorageDeposit, 1), depositor_held - 10);
        assert_eq!(Balances::free_balance(3), other_admin_free);

        assert_ok!(StorageProvider::remove_member(
            RuntimeOrigin::signed(1),
            bucket_id,
            1
        ));
        assert_eq!(held(HoldReason::StorageDeposit, 1), depositor_held - 20);
    });
}

/// The deposit is what makes a bucket cost something, so an account that
/// cannot fund it must not get the record.
#[test]
fn create_bucket_fails_without_funds_for_the_deposit() {
    new_test_ext().execute_with(|| {
        assert_ok!(Balances::force_set_balance(RuntimeOrigin::root(), 9, 5));

        assert_noop!(
            StorageProvider::create_bucket(
                RuntimeOrigin::signed(9),
                0,
                storage_primitives::Visibility::Public
            ),
            sp_runtime::TokenError::FundsUnavailable
        );
    });
}

/// The deposit belongs to whoever created the bucket, not to whoever removes
/// it: a later admin must not be able to pocket the creator's funds.
#[test]
fn removing_a_bucket_refunds_the_creator_not_the_admin_who_removes_it() {
    new_test_ext().execute_with(|| {
        let creator_free = Balances::free_balance(1);
        let admin_free = Balances::free_balance(3);
        let bucket_id = create_bucket(1, 0);
        assert_ok!(StorageProvider::set_member(
            RuntimeOrigin::signed(1),
            bucket_id,
            3,
            Role::Admin
        ));
        assert_ok!(StorageProvider::set_member(
            RuntimeOrigin::signed(1),
            bucket_id,
            1,
            Role::Reader
        ));
        assert_ok!(StorageProvider::remove_member(
            RuntimeOrigin::signed(3),
            bucket_id,
            1
        ));

        assert_ok!(StorageProvider::cleanup_bucket_internal(bucket_id, &3));

        assert_eq!(held(HoldReason::StorageDeposit, 1), 0);
        assert_eq!(held(HoldReason::StorageDeposit, 3), 0);
        assert_eq!(Balances::free_balance(1), creator_free);
        assert_eq!(Balances::free_balance(3), admin_free);
        assert!(Buckets::<Test>::get(bucket_id).is_none());
    });
}

#[test]
fn create_multiple_buckets_increments_id() {
    new_test_ext().execute_with(|| {
        create_bucket(1, 1);
        create_bucket(2, 2);
        create_bucket(1, 3);

        assert_eq!(NextBucketId::<Test>::get(), 3);
        assert!(Buckets::<Test>::get(0).is_some());
        assert!(Buckets::<Test>::get(1).is_some());
        assert!(Buckets::<Test>::get(2).is_some());
    });
}

#[test]
fn set_member_works() {
    new_test_ext().execute_with(|| {
        create_bucket(1, 1);

        // Add writer
        assert_ok!(StorageProvider::set_member(
            RuntimeOrigin::signed(1),
            0,
            2,
            Role::Writer
        ));

        let bucket = Buckets::<Test>::get(0).unwrap();
        assert_eq!(bucket.members.len(), 2);

        let writer = bucket.members.iter().find(|m| m.account == 2).unwrap();
        assert_eq!(writer.role, Role::Writer);
    });
}

#[test]
fn set_member_updates_existing_role() {
    new_test_ext().execute_with(|| {
        create_bucket(1, 1);

        // Add as writer
        assert_ok!(StorageProvider::set_member(
            RuntimeOrigin::signed(1),
            0,
            2,
            Role::Writer
        ));

        // Promote to admin
        assert_ok!(StorageProvider::set_member(
            RuntimeOrigin::signed(1),
            0,
            2,
            Role::Admin
        ));

        let bucket = Buckets::<Test>::get(0).unwrap();
        let member = bucket.members.iter().find(|m| m.account == 2).unwrap();
        assert_eq!(member.role, Role::Admin);
    });
}

#[test]
fn set_member_fails_for_non_admin() {
    new_test_ext().execute_with(|| {
        create_bucket(1, 1);

        // Non-admin tries to add member
        assert_noop!(
            StorageProvider::set_member(RuntimeOrigin::signed(2), 0, 3, Role::Writer),
            Error::<Test>::NotBucketAdmin
        );
    });
}

#[test]
fn cannot_demote_other_admin() {
    new_test_ext().execute_with(|| {
        create_bucket(1, 1);

        // Add second admin
        assert_ok!(StorageProvider::set_member(
            RuntimeOrigin::signed(1),
            0,
            2,
            Role::Admin
        ));

        // Admin 1 tries to demote admin 2
        assert_noop!(
            StorageProvider::set_member(RuntimeOrigin::signed(1), 0, 2, Role::Writer),
            Error::<Test>::CannotDemoteAdmin
        );
    });
}

#[test]
fn last_admin_cannot_self_demote() {
    new_test_ext().execute_with(|| {
        create_bucket(1, 1);

        // Admin 1 is the sole admin and cannot demote themselves.
        assert_noop!(
            StorageProvider::set_member(RuntimeOrigin::signed(1), 0, 1, Role::Writer),
            Error::<Test>::LastAdminCannotBeRemoved
        );
    });
}

#[test]
fn last_admin_cannot_be_removed() {
    new_test_ext().execute_with(|| {
        create_bucket(1, 1);

        assert_noop!(
            StorageProvider::remove_member(RuntimeOrigin::signed(1), 0, 1),
            Error::<Test>::LastAdminCannotBeRemoved
        );
    });
}

#[test]
fn admin_can_demote_self() {
    new_test_ext().execute_with(|| {
        create_bucket(1, 1);

        // Add second admin
        assert_ok!(StorageProvider::set_member(
            RuntimeOrigin::signed(1),
            0,
            2,
            Role::Admin
        ));

        // Admin 1 demotes self
        assert_ok!(StorageProvider::set_member(
            RuntimeOrigin::signed(1),
            0,
            1,
            Role::Writer
        ));

        let bucket = Buckets::<Test>::get(0).unwrap();
        let member = bucket.members.iter().find(|m| m.account == 1).unwrap();
        assert_eq!(member.role, Role::Writer);
    });
}

#[test]
fn remove_member_works() {
    new_test_ext().execute_with(|| {
        create_bucket(1, 1);
        assert_ok!(StorageProvider::set_member(
            RuntimeOrigin::signed(1),
            0,
            2,
            Role::Writer
        ));

        assert_ok!(StorageProvider::remove_member(
            RuntimeOrigin::signed(1),
            0,
            2
        ));

        let bucket = Buckets::<Test>::get(0).unwrap();
        assert_eq!(bucket.members.len(), 1);
        assert!(!bucket.members.iter().any(|m| m.account == 2));
    });
}

#[test]
fn remove_member_fails_for_non_existent() {
    new_test_ext().execute_with(|| {
        create_bucket(1, 1);

        assert_noop!(
            StorageProvider::remove_member(RuntimeOrigin::signed(1), 0, 99),
            Error::<Test>::MemberNotFound
        );
    });
}

#[test]
fn set_min_providers_works() {
    new_test_ext().execute_with(|| {
        create_bucket(1, 2);

        // Can set to 0 (no minimum)
        assert_ok!(StorageProvider::set_min_providers(
            RuntimeOrigin::signed(1),
            0,
            0
        ));

        let bucket = Buckets::<Test>::get(0).unwrap();
        assert_eq!(bucket.min_providers, 0);
    });
}

#[test]
fn freeze_bucket_requires_snapshot() {
    new_test_ext().execute_with(|| {
        create_bucket(1, 1);

        assert_noop!(
            StorageProvider::freeze_bucket(RuntimeOrigin::signed(1), 0),
            Error::<Test>::NoSnapshot
        );
    });
}
