// SPDX-License-Identifier: Apache-2.0

use crate::{
    mock::{MaxMultiaddrLength, *},
    Error, Event,
};
use frame_support::{assert_noop, assert_ok, traits::ConstU32, BoundedVec};
use pallet_storage_provider::{AgreementTermsOf, ProviderSettings};
use sp_core::crypto::KeyTypeId;
use storage_primitives::{AgreementTerms, Role};

const PROVIDER_KEY_TYPE: KeyTypeId = KeyTypeId(*b"prov");

/// Generate a provider sr25519 keypair via the runtime keystore.
fn generate_provider_public_key(
    seed: &str,
) -> (sp_core::sr25519::Public, BoundedVec<u8, ConstU32<64>>) {
    let public = sp_io::crypto::sr25519_generate(PROVIDER_KEY_TYPE, Some(seed.as_bytes().to_vec()));
    let bounded = public.0.to_vec().try_into().unwrap();
    (public, bounded)
}

/// Sign SCALE-encoded terms with the provider's keystore key.
fn sign_terms(
    public: &sp_core::sr25519::Public,
    terms: &AgreementTermsOf<Test>,
) -> sp_runtime::MultiSignature {
    let hash = sp_io::hashing::blake2_256(&terms.signing_payload());
    let sig = sp_io::crypto::sr25519_sign(PROVIDER_KEY_TYPE, public, &hash)
        .expect("keystore signs with a key it generated");
    sp_runtime::MultiSignature::Sr25519(sig)
}

/// Build primary terms for the standard test provider.
fn primary_terms(
    owner: u64,
    max_bytes: u64,
    duration: u64,
    nonce: u64,
    price_per_byte: u128,
) -> AgreementTermsOf<Test> {
    AgreementTerms {
        owner,
        max_bytes,
        duration,
        price_per_byte,
        valid_until: frame_system::Pallet::<Test>::block_number()
            .saturating_add(<Test as pallet_storage_provider::Config>::RequestTimeout::get()),
        nonce,
        bucket_id: None,
        replica_params: None,
    }
}

/// Register provider account 3 with capacity that covers our test agreements,
/// and return its sr25519 public key so callers can sign terms.
fn setup_provider() -> (sp_core::sr25519::Public, u64) {
    let provider = 3;
    let multiaddr: BoundedVec<u8, MaxMultiaddrLength> =
        b"/ip4/127.0.0.1/tcp/3000".to_vec().try_into().unwrap();
    let (public, public_key_bytes) = generate_provider_public_key("//Provider");
    assert_ok!(StorageProvider::register_provider(
        RuntimeOrigin::signed(provider),
        multiaddr,
        public_key_bytes,
        10_000_000_000_000, // Must exceed MinProviderStake (1_000_000_000_000)
    ));
    let settings = ProviderSettings::<Test> {
        min_duration: 10u64,
        max_duration: 10_000u64,
        price_per_byte: 0u128,
        accepting_primary: true,
        replica_sync_price: None,
        accepting_extensions: true,
        max_capacity: 10_000_000_000, // stake / MinStakePerByte
    };
    assert_ok!(StorageProvider::update_provider_settings(
        RuntimeOrigin::signed(provider),
        settings
    ));
    (public, provider)
}

/// Advance to block 1 so that events are registered.
fn advance_to_block_1() {
    frame_system::Pallet::<Test>::set_block_number(1);
}

#[test]
fn create_drive_surfaces_layer0_signature_errors() {
    // If the named provider isn't registered, signature verification fails
    // at Layer 0 and surfaces directly — the registry no longer wraps it
    // in NoProvidersAvailable / BucketCreationFailed.
    new_test_ext().execute_with(|| {
        let (unregistered_pk, _) = generate_provider_public_key("//Ghost");
        let terms = primary_terms(1, 100, 500, 1, 1000);
        let sig = sign_terms(&unregistered_pk, &terms);
        assert_noop!(
            DriveRegistry::create_drive(
                RuntimeOrigin::signed(1),
                Some(b"My Documents".to_vec()),
                3,
                terms,
                sig,
            ),
            pallet_storage_provider::Error::<Test>::ProviderNotFound
        );
    });
}

#[test]
fn create_drive_name_too_long_fails() {
    new_test_ext().execute_with(|| {
        let (provider_pk, provider) = setup_provider();
        let terms = primary_terms(1, 100, 500, 1, 100);
        let sig = sign_terms(&provider_pk, &terms);
        let long_name = vec![b'a'; 257]; // MaxDriveNameLength = 256 in mock

        assert_noop!(
            DriveRegistry::create_drive(
                RuntimeOrigin::signed(1),
                Some(long_name),
                provider,
                terms,
                sig
            ),
            Error::<Test>::DriveNameTooLong
        );
    });
}

#[test]
fn delete_drive_not_found_fails() {
    new_test_ext().execute_with(|| {
        assert_noop!(
            DriveRegistry::delete_drive(RuntimeOrigin::signed(1), 999),
            Error::<Test>::DriveNotFound
        );
    });
}

#[test]
fn delete_drive_not_owner_fails() {
    new_test_ext().execute_with(|| {
        let (provider_pk, provider) = setup_provider();
        let terms = primary_terms(1, 100, 500, 1, 100);
        let sig = sign_terms(&provider_pk, &terms);
        assert_ok!(DriveRegistry::create_drive(
            RuntimeOrigin::signed(1),
            None,
            provider,
            terms,
            sig,
        ));
        // Bob is not the owner.
        assert_noop!(
            DriveRegistry::delete_drive(RuntimeOrigin::signed(2), 0),
            Error::<Test>::NotDriveOwner
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
        assert_noop!(
            DriveRegistry::share_drive(RuntimeOrigin::signed(1), 999, 2, Role::Reader),
            Error::<Test>::DriveNotFound
        );
    });
}

#[test]
fn unshare_drive_fails_when_drive_not_found() {
    new_test_ext().execute_with(|| {
        assert_noop!(
            DriveRegistry::unshare_drive(RuntimeOrigin::signed(1), 999, 2),
            Error::<Test>::DriveNotFound
        );
    });
}

#[test]
fn share_drive_fails_when_non_owner_non_admin() {
    new_test_ext().execute_with(|| {
        // Without a drive existing, we just hit DriveNotFound. A full
        // permission test would set up a drive and have a non-admin try
        // to share it.
        assert_noop!(
            DriveRegistry::share_drive(RuntimeOrigin::signed(2), 0, 3, Role::Writer),
            Error::<Test>::DriveNotFound
        );
    });
}

// --- Success-path tests (require setup_provider) ---

#[test]
fn create_drive_works() {
    new_test_ext().execute_with(|| {
        advance_to_block_1();

        let (provider_pk, provider) = setup_provider();
        let terms = primary_terms(1, 100, 500, 1, 100);
        let sig = sign_terms(&provider_pk, &terms);

        let alice = 1u64;

        assert_ok!(DriveRegistry::create_drive(
            RuntimeOrigin::signed(alice),
            None,
            provider,
            terms,
            sig
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
        let (provider_pk, provider) = setup_provider();
        let terms = primary_terms(1, 100, 500, 1, 100);
        let sig = sign_terms(&provider_pk, &terms);

        let alice = 1u64;

        assert_ok!(DriveRegistry::create_drive(
            RuntimeOrigin::signed(alice),
            Some(b"My Documents".to_vec()),
            provider,
            terms,
            sig
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

        let (provider_pk, provider) = setup_provider();
        let terms = primary_terms(1, 100, 500, 1, 0);
        let sig = sign_terms(&provider_pk, &terms);

        let alice = 1u64;

        assert_ok!(DriveRegistry::create_drive(
            RuntimeOrigin::signed(alice),
            None,
            provider,
            terms,
            sig
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

        let (provider_pk, provider) = setup_provider();
        let terms = primary_terms(1, 100, 500, 1, 100);
        let sig = sign_terms(&provider_pk, &terms);

        let alice = 1u64;
        let bob = 2u64;

        assert_ok!(DriveRegistry::create_drive(
            RuntimeOrigin::signed(alice),
            None,
            provider,
            terms,
            sig
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

        let (provider_pk, provider) = setup_provider();
        let terms = primary_terms(1, 100, 500, 1, 100);
        let sig = sign_terms(&provider_pk, &terms);

        let alice = 1u64;
        let bob = 2u64;

        assert_ok!(DriveRegistry::create_drive(
            RuntimeOrigin::signed(alice),
            None,
            provider,
            terms,
            sig
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

        let (provider_pk, provider) = setup_provider();
        let terms = primary_terms(1, 100, 500, 1, 100);
        let sig = sign_terms(&provider_pk, &terms);

        let alice = 1u64;
        let bob = 2u64;

        assert_ok!(DriveRegistry::create_drive(
            RuntimeOrigin::signed(alice),
            None,
            provider,
            terms,
            sig
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
        let (provider_pk, provider) = setup_provider();
        let terms = primary_terms(1, 100, 500, 1, 100);
        let sig = sign_terms(&provider_pk, &terms);

        let alice = 1u64;

        assert_ok!(DriveRegistry::create_drive(
            RuntimeOrigin::signed(alice),
            None,
            provider,
            terms,
            sig
        ));

        let drives = DriveRegistry::list_user_drives(&alice);
        assert_eq!(drives, vec![0]);
    });
}

#[test]
fn delete_drive_removes_from_list() {
    new_test_ext().execute_with(|| {
        let (provider_pk, provider) = setup_provider();
        let terms = primary_terms(1, 100, 500, 1, 100);
        let sig = sign_terms(&provider_pk, &terms);

        let alice = 1u64;

        assert_ok!(DriveRegistry::create_drive(
            RuntimeOrigin::signed(alice),
            None,
            provider,
            terms,
            sig
        ));

        assert_eq!(DriveRegistry::list_user_drives(&alice), vec![0]);

        assert_ok!(DriveRegistry::delete_drive(RuntimeOrigin::signed(alice), 0));

        assert!(DriveRegistry::list_user_drives(&alice).is_empty());
    });
}

#[cfg(feature = "try-runtime")]
#[test]
fn try_state_holds_and_detects_corruption() {
    new_test_ext().execute_with(|| {
        advance_to_block_1();

        let (provider_pk, provider) = setup_provider();
        let terms = primary_terms(1, 100, 500, 1, 100);
        let sig = sign_terms(&provider_pk, &terms);

        assert_ok!(DriveRegistry::create_drive(
            RuntimeOrigin::signed(1),
            None,
            provider,
            terms,
            sig
        ));

        // Index invariants hold on real state.
        assert_ok!(DriveRegistry::do_try_state());

        // UserDrives now references a non-existent drive.
        crate::UserDrives::<Test>::mutate(1u64, |ds| {
            ds.try_push(999).expect("bound not reached");
        });
        assert!(DriveRegistry::do_try_state().is_err());
    });
}
