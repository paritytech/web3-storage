use crate::{mock::*, Error, Event};
use file_system_primitives::{compute_cid, CommitStrategy};
use frame_support::{assert_noop, assert_ok};
use sp_core::H256;

#[test]
fn create_drive_with_bucket_works() {
    new_test_ext().execute_with(|| {
        System::set_block_number(1);

        let alice = 1u64;
        let bucket_id = 42u64;
        let root_cid = H256::zero();
        let name = Some(b"My Drive".to_vec());

        // Create drive with existing bucket (legacy API)
        #[allow(deprecated)]
        assert_ok!(DriveRegistry::create_drive_with_bucket(
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
        assert_ok!(DriveRegistry::create_drive_with_bucket(
            RuntimeOrigin::signed(alice),
            1,
            H256::zero(),
            Some(b"Drive 1".to_vec())
        ));

        // Create second drive
        assert_ok!(DriveRegistry::create_drive_with_bucket(
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
            DriveRegistry::create_drive_with_bucket(
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
        assert_ok!(DriveRegistry::create_drive_with_bucket(
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
        assert_ok!(DriveRegistry::create_drive_with_bucket(
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
fn delete_drive_requires_layer0_bucket() {
    new_test_ext().execute_with(|| {
        System::set_block_number(1);
        let alice = 1u64;

        // Create drive using deprecated API (doesn't create Layer 0 bucket)
        #[allow(deprecated)]
        assert_ok!(DriveRegistry::create_drive_with_bucket(
            RuntimeOrigin::signed(alice),
            1,
            H256::zero(),
            None
        ));

        // Verify drive exists
        assert!(DriveRegistry::drives(0).is_some());
        assert_eq!(DriveRegistry::user_drives(alice).len(), 1);

        // Delete drive fails because bucket doesn't exist in Layer 0
        // The new delete_drive implementation requires proper Layer 0 cleanup
        assert_noop!(
            DriveRegistry::delete_drive(RuntimeOrigin::signed(alice), 0),
            Error::<Test>::BucketCleanupFailed
        );

        // Drive still exists after failed deletion
        assert!(DriveRegistry::drives(0).is_some());
    });
}

// NOTE: Full delete_drive integration test with Layer 0 bucket and agreements
// would require setting up providers, creating bucket properly, etc.
// For now, we test error handling with the deprecated API.

#[test]
fn delete_drive_not_owner_fails() {
    new_test_ext().execute_with(|| {
        let alice = 1u64;
        let bob = 2u64;

        // Alice creates drive
        assert_ok!(DriveRegistry::create_drive_with_bucket(
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
        assert_ok!(DriveRegistry::create_drive_with_bucket(
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
        assert_ok!(DriveRegistry::create_drive_with_bucket(
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
        assert_ok!(DriveRegistry::create_drive_with_bucket(
            RuntimeOrigin::signed(alice),
            1,
            H256::zero(),
            None
        ));
        assert_ok!(DriveRegistry::create_drive_with_bucket(
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

// ============================================================
// Bucket-Based Model Tests
// ============================================================

#[test]
fn create_drive_on_bucket_works() {
    new_test_ext().execute_with(|| {
        System::set_block_number(1);

        let alice = 1u64;
        let bucket_id = 42u64;
        let root_cid = H256::zero();
        let name = Some(b"My Drive".to_vec());

        // Create drive on bucket (simplified flow)
        // NOTE: In production, this would validate:
        // - Bucket exists in Layer 0
        // - User has Reader+Writer permissions
        // For now, we just test the basic functionality
        assert_ok!(DriveRegistry::create_drive_on_bucket(
            RuntimeOrigin::signed(alice),
            bucket_id,
            root_cid,
            name.clone()
        ));

        // Check drive was created
        let drive = DriveRegistry::drives(0).unwrap();
        assert_eq!(drive.owner, alice);
        assert_eq!(drive.bucket_id, bucket_id);
        assert_eq!(drive.root_cid, root_cid);

        // Check BucketToDrive mapping was established (1-to-1)
        assert_eq!(DriveRegistry::bucket_to_drive(bucket_id), Some(0));

        // Check user drives
        let user_drives = DriveRegistry::user_drives(alice);
        assert_eq!(user_drives.len(), 1);
        assert_eq!(user_drives[0], 0);

        // Check event
        System::assert_last_event(
            Event::DriveCreatedOnBucket {
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
fn create_drive_on_bucket_already_used_fails() {
    new_test_ext().execute_with(|| {
        let alice = 1u64;
        let bob = 2u64;
        let bucket_id = 42u64;

        // Alice creates drive on bucket 42
        assert_ok!(DriveRegistry::create_drive_on_bucket(
            RuntimeOrigin::signed(alice),
            bucket_id,
            H256::zero(),
            Some(b"Alice's Drive".to_vec())
        ));

        // Bob tries to create drive on same bucket - should fail
        assert_noop!(
            DriveRegistry::create_drive_on_bucket(
                RuntimeOrigin::signed(bob),
                bucket_id,
                H256::zero(),
                Some(b"Bob's Drive".to_vec())
            ),
            Error::<Test>::BucketAlreadyUsed
        );

        // Verify Alice's drive exists
        assert_eq!(DriveRegistry::bucket_to_drive(bucket_id), Some(0));
        let drive = DriveRegistry::drives(0).unwrap();
        assert_eq!(drive.owner, alice);

        // Verify Bob has no drives
        assert_eq!(DriveRegistry::user_drives(bob).len(), 0);
    });
}

#[test]
fn multiple_users_can_use_different_buckets() {
    new_test_ext().execute_with(|| {
        let alice = 1u64;
        let bob = 2u64;

        // Alice creates drive on bucket 1
        assert_ok!(DriveRegistry::create_drive_on_bucket(
            RuntimeOrigin::signed(alice),
            1,
            H256::zero(),
            Some(b"Alice's Drive".to_vec())
        ));

        // Bob creates drive on bucket 2 (different bucket)
        assert_ok!(DriveRegistry::create_drive_on_bucket(
            RuntimeOrigin::signed(bob),
            2,
            H256::zero(),
            Some(b"Bob's Drive".to_vec())
        ));

        // Verify both drives exist
        assert_eq!(DriveRegistry::bucket_to_drive(1), Some(0));
        assert_eq!(DriveRegistry::bucket_to_drive(2), Some(1));

        // Verify ownership
        let alice_drive = DriveRegistry::drives(0).unwrap();
        assert_eq!(alice_drive.owner, alice);
        assert_eq!(alice_drive.bucket_id, 1);

        let bob_drive = DriveRegistry::drives(1).unwrap();
        assert_eq!(bob_drive.owner, bob);
        assert_eq!(bob_drive.bucket_id, 2);
    });
}

#[test]
fn bucket_to_drive_mapping_persists() {
    new_test_ext().execute_with(|| {
        let alice = 1u64;
        let bucket_id = 42u64;

        // Create drive
        assert_ok!(DriveRegistry::create_drive_on_bucket(
            RuntimeOrigin::signed(alice),
            bucket_id,
            H256::zero(),
            None
        ));

        // Verify mapping exists
        assert_eq!(DriveRegistry::bucket_to_drive(bucket_id), Some(0));

        // Query the drive
        let drive = DriveRegistry::drives(0).unwrap();
        assert_eq!(drive.bucket_id, bucket_id);

        // Verify we can find the drive via bucket
        assert!(DriveRegistry::bucket_to_drive(bucket_id).is_some());

        // Verify other buckets have no mapping
        assert!(DriveRegistry::bucket_to_drive(999).is_none());
    });
}

// ============================================================
// Simplified User API Tests
// ============================================================

#[test]
fn create_drive_simplified_api_fails_without_providers() {
    new_test_ext().execute_with(|| {
        let alice = 1u64;
        let name = Some(b"My Documents".to_vec());
        let max_capacity = 10_000_000_000u64; // 10 GB
        let storage_period = 500u64; // 500 blocks
        let payment = 1_000_000_000_000u128; // 1 token (12 decimals)

        // Attempt to create drive with simplified API using defaults
        // Bucket creation will succeed, but it will fail when trying to find
        // available providers since none are registered in the test
        assert_noop!(
            DriveRegistry::create_drive(
                RuntimeOrigin::signed(alice),
                name,
                max_capacity,
                storage_period,
                payment,
                None, // Use default providers
                CommitStrategy::Batched { interval: 100 }, // Default strategy
            ),
            Error::<Test>::NoProvidersAvailable
        );
    });
}

#[test]
fn create_drive_validates_inputs() {
    new_test_ext().execute_with(|| {
        let alice = 1u64;

        // Test invalid storage size (zero)
        assert_noop!(
            DriveRegistry::create_drive(
                RuntimeOrigin::signed(alice),
                Some(b"My Drive".to_vec()),
                0, // Invalid: zero capacity
                500,
                1_000_000_000_000,
                None,
                CommitStrategy::Batched { interval: 100 },
            ),
            Error::<Test>::InvalidStorageSize
        );

        // Test invalid storage period (zero)
        assert_noop!(
            DriveRegistry::create_drive(
                RuntimeOrigin::signed(alice),
                Some(b"My Drive".to_vec()),
                10_000_000_000,
                0, // Invalid: zero period
                1_000_000_000_000,
                None,
                CommitStrategy::Batched { interval: 100 },
            ),
            Error::<Test>::InvalidStoragePeriod
        );

        // Test invalid payment (zero)
        assert_noop!(
            DriveRegistry::create_drive(
                RuntimeOrigin::signed(alice),
                Some(b"My Drive".to_vec()),
                10_000_000_000,
                500,
                0, // Invalid: zero payment
                None,
                CommitStrategy::Batched { interval: 100 },
            ),
            Error::<Test>::InvalidPayment
        );

        // Test invalid min_providers (zero)
        assert_noop!(
            DriveRegistry::create_drive(
                RuntimeOrigin::signed(alice),
                Some(b"My Drive".to_vec()),
                10_000_000_000,
                500,
                1_000_000_000_000,
                Some(0), // Invalid: zero providers
                CommitStrategy::Batched { interval: 100 }, // Default strategy
            ),
            Error::<Test>::InvalidProviderCount
        );
    });
}

// ============================================================
// Drive Cleanup Tests
// ============================================================

#[test]
fn clear_drive_works() {
    new_test_ext().execute_with(|| {
        System::set_block_number(1);
        let alice = 1u64;
        let bucket_id = 1u64;

        // Create drive
        assert_ok!(DriveRegistry::create_drive_with_bucket(
            RuntimeOrigin::signed(alice),
            bucket_id,
            H256::zero(),
            Some(b"My Drive".to_vec())
        ));

        // Update root CID to simulate data
        let data_cid = compute_cid(b"some data");
        assert_ok!(DriveRegistry::update_root_cid(
            RuntimeOrigin::signed(alice),
            0,
            data_cid
        ));

        // Verify drive has data
        let drive = DriveRegistry::drives(0).unwrap();
        assert_eq!(drive.root_cid, data_cid);

        // Clear drive
        assert_ok!(DriveRegistry::clear_drive(RuntimeOrigin::signed(alice), 0));

        // Verify drive is cleared but still exists
        let drive = DriveRegistry::drives(0).unwrap();
        assert_eq!(drive.root_cid, H256::zero());
        assert_eq!(drive.pending_root_cid, None);
        assert_eq!(drive.owner, alice);
        assert_eq!(drive.bucket_id, bucket_id);

        // Verify user still owns the drive
        let user_drives = DriveRegistry::user_drives(alice);
        assert_eq!(user_drives.len(), 1);
        assert_eq!(user_drives[0], 0);

        // Check event
        System::assert_last_event(
            Event::DriveCleared {
                drive_id: 0,
                owner: alice,
                old_root_cid: data_cid,
            }
            .into(),
        );
    });
}

#[test]
fn clear_drive_not_owner_fails() {
    new_test_ext().execute_with(|| {
        let alice = 1u64;
        let bob = 2u64;

        // Alice creates drive
        assert_ok!(DriveRegistry::create_drive_with_bucket(
            RuntimeOrigin::signed(alice),
            1,
            H256::zero(),
            None
        ));

        // Bob tries to clear Alice's drive
        assert_noop!(
            DriveRegistry::clear_drive(RuntimeOrigin::signed(bob), 0),
            Error::<Test>::NotDriveOwner
        );
    });
}

#[test]
fn clear_drive_multiple_times_works() {
    new_test_ext().execute_with(|| {
        let alice = 1u64;

        // Create drive
        assert_ok!(DriveRegistry::create_drive_with_bucket(
            RuntimeOrigin::signed(alice),
            1,
            H256::zero(),
            None
        ));

        // Add data
        let cid1 = compute_cid(b"data 1");
        assert_ok!(DriveRegistry::update_root_cid(
            RuntimeOrigin::signed(alice),
            0,
            cid1
        ));

        // Clear drive
        assert_ok!(DriveRegistry::clear_drive(RuntimeOrigin::signed(alice), 0));

        // Add new data
        let cid2 = compute_cid(b"data 2");
        assert_ok!(DriveRegistry::update_root_cid(
            RuntimeOrigin::signed(alice),
            0,
            cid2
        ));

        // Clear again
        assert_ok!(DriveRegistry::clear_drive(RuntimeOrigin::signed(alice), 0));

        // Verify drive is empty
        let drive = DriveRegistry::drives(0).unwrap();
        assert_eq!(drive.root_cid, H256::zero());
    });
}

#[test]
fn clear_drive_not_found_fails() {
    new_test_ext().execute_with(|| {
        let alice = 1u64;

        assert_noop!(
            DriveRegistry::clear_drive(RuntimeOrigin::signed(alice), 999),
            Error::<Test>::DriveNotFound
        );
    });
}
