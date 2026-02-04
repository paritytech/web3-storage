use crate::{mock::*, Error, Event};
use file_system_primitives::compute_cid;
use frame_support::{assert_noop, assert_ok};
use sp_core::H256;

#[test]
fn create_drive_works() {
    new_test_ext().execute_with(|| {
        System::set_block_number(1);

        let alice = 1u64;
        let bucket_id = 42u64;
        let root_cid = H256::zero();
        let name = Some(b"My Drive".to_vec());

        // Create drive
        assert_ok!(DriveRegistry::create_drive(
            RuntimeOrigin::signed(alice),
            bucket_id,
            root_cid,
            name.clone()
        ));

        // Check storage
        let drive = DriveRegistry::drives(0).unwrap();
        assert_eq!(drive.owner, alice);
        assert_eq!(drive.bucket_id, bucket_id);
        assert_eq!(drive.root_cid, root_cid);
        let expected_name = name.map(|n| sp_runtime::BoundedVec::try_from(n).unwrap());
        assert_eq!(drive.name, expected_name);

        // Check user drives
        let user_drives = DriveRegistry::user_drives(alice);
        assert_eq!(user_drives.len(), 1);
        assert_eq!(user_drives[0], 0);

        // Check next drive ID
        assert_eq!(DriveRegistry::next_drive_id(), 1);

        // Check event
        System::assert_last_event(
            Event::DriveCreated {
                drive_id: 0,
                owner: alice,
                bucket_id,
                root_cid,
            }
            .into(),
        );
    });
}

#[test]
fn create_multiple_drives_works() {
    new_test_ext().execute_with(|| {
        let alice = 1u64;

        // Create first drive
        assert_ok!(DriveRegistry::create_drive(
            RuntimeOrigin::signed(alice),
            1,
            H256::zero(),
            Some(b"Drive 1".to_vec())
        ));

        // Create second drive
        assert_ok!(DriveRegistry::create_drive(
            RuntimeOrigin::signed(alice),
            2,
            H256::zero(),
            Some(b"Drive 2".to_vec())
        ));

        // Check user has 2 drives
        let user_drives = DriveRegistry::user_drives(alice);
        assert_eq!(user_drives.len(), 2);
        assert_eq!(user_drives[0], 0);
        assert_eq!(user_drives[1], 1);

        // Check next ID
        assert_eq!(DriveRegistry::next_drive_id(), 2);
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
                1,
                H256::zero(),
                Some(long_name)
            ),
            Error::<Test>::DriveNameTooLong
        );
    });
}

#[test]
fn update_root_cid_works() {
    new_test_ext().execute_with(|| {
        System::set_block_number(1);
        let alice = 1u64;
        let bucket_id = 1u64;
        let initial_cid = H256::zero();

        // Create drive
        assert_ok!(DriveRegistry::create_drive(
            RuntimeOrigin::signed(alice),
            bucket_id,
            initial_cid,
            None
        ));

        // Update root CID
        let new_cid = compute_cid(b"new root");
        assert_ok!(DriveRegistry::update_root_cid(
            RuntimeOrigin::signed(alice),
            0,
            new_cid
        ));

        // Check updated
        let drive = DriveRegistry::drives(0).unwrap();
        assert_eq!(drive.root_cid, new_cid);

        // Check event
        System::assert_last_event(
            Event::RootCIDUpdated {
                drive_id: 0,
                old_root_cid: initial_cid,
                new_root_cid: new_cid,
            }
            .into(),
        );
    });
}

#[test]
fn update_root_cid_not_owner_fails() {
    new_test_ext().execute_with(|| {
        let alice = 1u64;
        let bob = 2u64;

        // Alice creates drive
        assert_ok!(DriveRegistry::create_drive(
            RuntimeOrigin::signed(alice),
            1,
            H256::zero(),
            None
        ));

        // Bob tries to update
        let new_cid = compute_cid(b"bob's root");
        assert_noop!(
            DriveRegistry::update_root_cid(RuntimeOrigin::signed(bob), 0, new_cid),
            Error::<Test>::NotDriveOwner
        );
    });
}

#[test]
fn update_root_cid_drive_not_found_fails() {
    new_test_ext().execute_with(|| {
        let alice = 1u64;
        let new_cid = compute_cid(b"new root");

        assert_noop!(
            DriveRegistry::update_root_cid(RuntimeOrigin::signed(alice), 999, new_cid),
            Error::<Test>::DriveNotFound
        );
    });
}

#[test]
fn delete_drive_works() {
    new_test_ext().execute_with(|| {
        System::set_block_number(1);
        let alice = 1u64;

        // Create drive
        assert_ok!(DriveRegistry::create_drive(
            RuntimeOrigin::signed(alice),
            1,
            H256::zero(),
            None
        ));

        // Verify it exists
        assert!(DriveRegistry::drives(0).is_some());
        assert_eq!(DriveRegistry::user_drives(alice).len(), 1);

        // Delete drive
        assert_ok!(DriveRegistry::delete_drive(RuntimeOrigin::signed(alice), 0));

        // Verify it's gone
        assert!(DriveRegistry::drives(0).is_none());
        assert_eq!(DriveRegistry::user_drives(alice).len(), 0);

        // Check event
        System::assert_last_event(
            Event::DriveDeleted {
                drive_id: 0,
                owner: alice,
            }
            .into(),
        );
    });
}

#[test]
fn delete_drive_not_owner_fails() {
    new_test_ext().execute_with(|| {
        let alice = 1u64;
        let bob = 2u64;

        // Alice creates drive
        assert_ok!(DriveRegistry::create_drive(
            RuntimeOrigin::signed(alice),
            1,
            H256::zero(),
            None
        ));

        // Bob tries to delete
        assert_noop!(
            DriveRegistry::delete_drive(RuntimeOrigin::signed(bob), 0),
            Error::<Test>::NotDriveOwner
        );
    });
}

#[test]
fn update_drive_name_works() {
    new_test_ext().execute_with(|| {
        System::set_block_number(1);
        let alice = 1u64;

        // Create drive
        assert_ok!(DriveRegistry::create_drive(
            RuntimeOrigin::signed(alice),
            1,
            H256::zero(),
            Some(b"Original Name".to_vec())
        ));

        // Update name
        let new_name = Some(b"New Name".to_vec());
        assert_ok!(DriveRegistry::update_drive_name(
            RuntimeOrigin::signed(alice),
            0,
            new_name.clone()
        ));

        // Check updated
        let drive = DriveRegistry::drives(0).unwrap();
        let expected_name = new_name
            .clone()
            .map(|n| sp_runtime::BoundedVec::try_from(n).unwrap());
        assert_eq!(drive.name, expected_name);

        // Check event
        System::assert_last_event(
            Event::DriveNameUpdated {
                drive_id: 0,
                name: new_name,
            }
            .into(),
        );
    });
}

#[test]
fn update_drive_name_clear_works() {
    new_test_ext().execute_with(|| {
        let alice = 1u64;

        // Create drive with name
        assert_ok!(DriveRegistry::create_drive(
            RuntimeOrigin::signed(alice),
            1,
            H256::zero(),
            Some(b"Name".to_vec())
        ));

        // Clear name
        assert_ok!(DriveRegistry::update_drive_name(
            RuntimeOrigin::signed(alice),
            0,
            None
        ));

        // Check cleared
        let drive = DriveRegistry::drives(0).unwrap();
        assert_eq!(drive.name, None);
    });
}

#[test]
fn helper_functions_work() {
    new_test_ext().execute_with(|| {
        let alice = 1u64;
        let bob = 2u64;

        // Create drives for Alice
        assert_ok!(DriveRegistry::create_drive(
            RuntimeOrigin::signed(alice),
            1,
            H256::zero(),
            None
        ));
        assert_ok!(DriveRegistry::create_drive(
            RuntimeOrigin::signed(alice),
            2,
            H256::zero(),
            None
        ));

        // Test get_drive
        assert!(DriveRegistry::get_drive(0).is_some());
        assert!(DriveRegistry::get_drive(999).is_none());

        // Test list_user_drives
        let alice_drives = DriveRegistry::list_user_drives(&alice);
        assert_eq!(alice_drives.len(), 2);
        assert_eq!(alice_drives, vec![0, 1]);

        let bob_drives = DriveRegistry::list_user_drives(&bob);
        assert_eq!(bob_drives.len(), 0);

        // Test is_drive_owner
        assert!(DriveRegistry::is_drive_owner(0, &alice));
        assert!(!DriveRegistry::is_drive_owner(0, &bob));
        assert!(!DriveRegistry::is_drive_owner(999, &alice));
    });
}
