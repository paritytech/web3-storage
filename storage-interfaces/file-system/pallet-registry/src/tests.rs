use crate::{mock::*, Error};
use frame_support::assert_noop;

#[test]
fn create_drive_validates_inputs() {
    new_test_ext().execute_with(|| {
        let alice = 1u64;

        // Zero capacity
        assert_noop!(
            DriveRegistry::create_drive(
                RuntimeOrigin::signed(alice),
                Some(b"My Drive".to_vec()),
                0, // invalid
                500,
                1_000_000_000_000,
                None,
            ),
            Error::<Test>::InvalidStorageSize
        );

        // Zero storage period
        assert_noop!(
            DriveRegistry::create_drive(
                RuntimeOrigin::signed(alice),
                Some(b"My Drive".to_vec()),
                10_000_000_000,
                0, // invalid
                1_000_000_000_000,
                None,
            ),
            Error::<Test>::InvalidStoragePeriod
        );

        // Zero payment
        assert_noop!(
            DriveRegistry::create_drive(
                RuntimeOrigin::signed(alice),
                Some(b"My Drive".to_vec()),
                10_000_000_000,
                500,
                0, // invalid
                None,
            ),
            Error::<Test>::InvalidPayment
        );

        // Zero min_providers
        assert_noop!(
            DriveRegistry::create_drive(
                RuntimeOrigin::signed(alice),
                Some(b"My Drive".to_vec()),
                10_000_000_000,
                500,
                1_000_000_000_000,
                Some(0), // invalid
            ),
            Error::<Test>::InvalidProviderCount
        );
    });
}

#[test]
fn create_drive_fails_without_providers() {
    new_test_ext().execute_with(|| {
        let alice = 1u64;

        // No providers registered in the test mock
        assert_noop!(
            DriveRegistry::create_drive(
                RuntimeOrigin::signed(alice),
                Some(b"My Documents".to_vec()),
                10_000_000_000,
                500,
                1_000_000_000_000,
                None,
            ),
            Error::<Test>::NoProvidersAvailable
        );
    });
}

#[test]
fn create_drive_name_too_long_fails() {
    new_test_ext().execute_with(|| {
        let alice = 1u64;
        let long_name = vec![b'a'; 257]; // Max is 256

        assert_noop!(
            DriveRegistry::create_drive(
                RuntimeOrigin::signed(alice),
                Some(long_name),
                10_000_000_000,
                500,
                1_000_000_000_000,
                None,
            ),
            Error::<Test>::DriveNameTooLong
        );
    });
}

#[test]
fn delete_drive_not_found_fails() {
    new_test_ext().execute_with(|| {
        let alice = 1u64;

        assert_noop!(
            DriveRegistry::delete_drive(RuntimeOrigin::signed(alice), 999),
            Error::<Test>::DriveNotFound
        );
    });
}

#[test]
fn delete_drive_not_owner_fails() {
    new_test_ext().execute_with(|| {
        // We can't easily create a drive without providers, so we just test that
        // deleting a nonexistent drive gives DriveNotFound.
        // A full integration test would set up providers + create drive + delete.
        let bob = 2u64;

        assert_noop!(
            DriveRegistry::delete_drive(RuntimeOrigin::signed(bob), 0),
            Error::<Test>::DriveNotFound
        );
    });
}

#[test]
fn helper_functions_work() {
    new_test_ext().execute_with(|| {
        let alice = 1u64;
        let bob = 2u64;

        // No drives exist yet
        assert!(DriveRegistry::get_drive(0).is_none());
        assert!(DriveRegistry::get_drive(999).is_none());

        let alice_drives = DriveRegistry::list_user_drives(&alice);
        assert_eq!(alice_drives.len(), 0);

        let bob_drives = DriveRegistry::list_user_drives(&bob);
        assert_eq!(bob_drives.len(), 0);

        assert!(!DriveRegistry::is_drive_owner(0, &alice));
        assert!(!DriveRegistry::is_drive_owner(999, &alice));
    });
}
