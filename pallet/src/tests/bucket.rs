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
        assert!(bucket.snapshot.is_none());
        assert!(bucket.frozen_start_seq.is_none());

        // Check bucket ID incremented
        assert_eq!(NextBucketId::<Test>::get(), 1);
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
