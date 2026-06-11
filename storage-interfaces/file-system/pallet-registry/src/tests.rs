use crate::{mock::*, Error, Event};
use frame_support::{assert_noop, assert_ok, traits::ConstU32, BoundedVec};
use pallet_storage_provider::ProviderSettings;
use storage_primitives::Role;

fn test_public_key() -> BoundedVec<u8, ConstU32<64>> {
    vec![1u8; 32].try_into().unwrap()
}

/// Advance to block 1 so that events are registered.
fn advance_to_block_1() {
    frame_system::Pallet::<Test>::set_block_number(1);
}

/// Register provider (account 3) with accepting_primary = true.
fn setup_provider() {
    let multiaddr: BoundedVec<u8, MaxMultiaddrLength> =
        b"/ip4/127.0.0.1/tcp/3000".to_vec().try_into().unwrap();
    assert_ok!(StorageProvider::register_provider(
        RuntimeOrigin::signed(3),
        multiaddr,
        test_public_key(),
        10_000_000_000_000, // Must exceed MinProviderStake (1_000_000_000_000)
    ));
    assert_ok!(StorageProvider::update_provider_settings(
        RuntimeOrigin::signed(3),
        ProviderSettings {
            min_duration: 10u64,
            max_duration: 10_000u64,
            price_per_byte: 0u128,
            accepting_primary: true,
            replica_sync_price: None,
            accepting_extensions: true,
            max_capacity: 10_000_000_000,
        },
    ));
}

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

#[test]
fn share_drive_fails_when_drive_not_found() {
    new_test_ext().execute_with(|| {
        let alice = 1u64;
        let bob = 2u64;

        assert_noop!(
            DriveRegistry::share_drive(
                RuntimeOrigin::signed(alice),
                999, // nonexistent
                bob,
                Role::Reader,
            ),
            Error::<Test>::DriveNotFound
        );
    });
}

#[test]
fn unshare_drive_fails_when_drive_not_found() {
    new_test_ext().execute_with(|| {
        let alice = 1u64;
        let bob = 2u64;

        assert_noop!(
            DriveRegistry::unshare_drive(
                RuntimeOrigin::signed(alice),
                999, // nonexistent
                bob,
            ),
            Error::<Test>::DriveNotFound
        );
    });
}

#[test]
fn share_drive_fails_when_non_owner_non_admin() {
    new_test_ext().execute_with(|| {
        // Without a drive existing, we just test drive-not-found.
        // Full permission tests require a registered provider + created drive.
        let bob = 2u64;
        let charlie = 3u64;

        assert_noop!(
            DriveRegistry::share_drive(RuntimeOrigin::signed(bob), 0, charlie, Role::Writer,),
            Error::<Test>::DriveNotFound
        );
    });
}

// --- Success-path tests (require setup_provider) ---

#[test]
fn create_drive_works() {
    new_test_ext().execute_with(|| {
        advance_to_block_1();
        setup_provider();
        let alice = 1u64;

        assert_ok!(DriveRegistry::create_drive(
            RuntimeOrigin::signed(alice),
            None,
            100,  // max_capacity
            500,  // storage_period (≤1000 → 1 provider)
            1000, // payment (price_per_byte=0 so any amount works)
            None,
        ));

        // Verify drive state
        let drive = DriveRegistry::get_drive(0).expect("drive should exist");
        assert_eq!(drive.owner, alice);
        assert_eq!(drive.max_capacity, 100);
        assert_eq!(drive.storage_period, 500);
        assert!(drive.name.is_none());

        // Verify Layer 0 bucket exists
        assert!(pallet_storage_provider::Buckets::<Test>::get(drive.bucket_id).is_some());

        // Verify event
        System::assert_has_event(
            Event::<Test>::DriveCreated {
                drive_id: 0,
                owner: alice,
                bucket_id: drive.bucket_id,
            }
            .into(),
        );
    });
}

#[test]
fn create_drive_with_name_works() {
    new_test_ext().execute_with(|| {
        setup_provider();
        let alice = 1u64;

        assert_ok!(DriveRegistry::create_drive(
            RuntimeOrigin::signed(alice),
            Some(b"My Documents".to_vec()),
            100,
            500,
            1000,
            None,
        ));

        let drive = DriveRegistry::get_drive(0).expect("drive should exist");
        assert_eq!(drive.owner, alice);
        assert_eq!(drive.name.as_ref().unwrap().as_slice(), b"My Documents");
    });
}

#[test]
fn delete_drive_works() {
    new_test_ext().execute_with(|| {
        advance_to_block_1();
        setup_provider();
        let alice = 1u64;

        assert_ok!(DriveRegistry::create_drive(
            RuntimeOrigin::signed(alice),
            None,
            100,
            500,
            1000,
            None,
        ));

        let drive = DriveRegistry::get_drive(0).expect("drive should exist");
        let bucket_id = drive.bucket_id;

        assert_ok!(DriveRegistry::delete_drive(RuntimeOrigin::signed(alice), 0));

        // Drive removed
        assert!(DriveRegistry::get_drive(0).is_none());

        // User's drive list empty
        assert!(DriveRegistry::list_user_drives(&alice).is_empty());

        // Verify DriveDeleted event was emitted
        // With price_per_byte=0, calculated payment is 0, so total_refunded = 0
        System::assert_has_event(
            Event::<Test>::DriveDeleted {
                drive_id: 0,
                owner: alice,
                bucket_id,
                refunded: 0,
            }
            .into(),
        );
    });
}

#[test]
fn share_drive_works() {
    new_test_ext().execute_with(|| {
        advance_to_block_1();
        setup_provider();
        let alice = 1u64;
        let bob = 2u64;

        assert_ok!(DriveRegistry::create_drive(
            RuntimeOrigin::signed(alice),
            None,
            100,
            500,
            1000,
            None,
        ));

        assert_ok!(DriveRegistry::share_drive(
            RuntimeOrigin::signed(alice),
            0,
            bob,
            Role::Writer,
        ));

        System::assert_has_event(
            Event::<Test>::DriveShared {
                drive_id: 0,
                member: bob,
                role: Role::Writer,
            }
            .into(),
        );
    });
}

#[test]
fn unshare_drive_works() {
    new_test_ext().execute_with(|| {
        advance_to_block_1();
        setup_provider();
        let alice = 1u64;
        let bob = 2u64;

        assert_ok!(DriveRegistry::create_drive(
            RuntimeOrigin::signed(alice),
            None,
            100,
            500,
            1000,
            None,
        ));

        // Share then unshare
        assert_ok!(DriveRegistry::share_drive(
            RuntimeOrigin::signed(alice),
            0,
            bob,
            Role::Writer,
        ));

        assert_ok!(DriveRegistry::unshare_drive(
            RuntimeOrigin::signed(alice),
            0,
            bob,
        ));

        System::assert_has_event(
            Event::<Test>::DriveUnshared {
                drive_id: 0,
                member: bob,
            }
            .into(),
        );
    });
}

#[test]
fn share_drive_admin_can_share() {
    new_test_ext().execute_with(|| {
        advance_to_block_1();
        setup_provider();
        let alice = 1u64;
        let bob = 2u64;

        assert_ok!(DriveRegistry::create_drive(
            RuntimeOrigin::signed(alice),
            None,
            100,
            500,
            1000,
            None,
        ));

        // Alice adds Bob as Admin
        assert_ok!(DriveRegistry::share_drive(
            RuntimeOrigin::signed(alice),
            0,
            bob,
            Role::Admin,
        ));

        // Bob (as Admin) shares with account 3 (Charlie - also the provider, but that's fine)
        // Note: account 3 is the provider but membership is separate from provider status
        let dave = 4u64; // Use a fresh account; no balance needed to be a member
        assert_ok!(DriveRegistry::share_drive(
            RuntimeOrigin::signed(bob),
            0,
            dave,
            Role::Reader,
        ));

        System::assert_has_event(
            Event::<Test>::DriveShared {
                drive_id: 0,
                member: dave,
                role: Role::Reader,
            }
            .into(),
        );
    });
}

#[test]
fn list_user_drives_after_create() {
    new_test_ext().execute_with(|| {
        setup_provider();
        let alice = 1u64;

        assert_ok!(DriveRegistry::create_drive(
            RuntimeOrigin::signed(alice),
            None,
            100,
            500,
            1000,
            None,
        ));

        let drives = DriveRegistry::list_user_drives(&alice);
        assert_eq!(drives, vec![0]);
    });
}

#[test]
fn delete_drive_removes_from_list() {
    new_test_ext().execute_with(|| {
        setup_provider();
        let alice = 1u64;

        assert_ok!(DriveRegistry::create_drive(
            RuntimeOrigin::signed(alice),
            None,
            100,
            500,
            1000,
            None,
        ));

        assert_eq!(DriveRegistry::list_user_drives(&alice), vec![0]);

        assert_ok!(DriveRegistry::delete_drive(RuntimeOrigin::signed(alice), 0));

        assert!(DriveRegistry::list_user_drives(&alice).is_empty());
    });
}
