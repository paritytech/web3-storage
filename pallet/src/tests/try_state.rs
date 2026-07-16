// SPDX-License-Identifier: Apache-2.0

//! `try_state` invariant checks exercised against real post-extrinsic state:
//! each implemented invariant holds on state the pallet's own extrinsics
//! produce, and a deliberate corruption of each is detected.

use super::*;

/// Every implemented invariant (P0 timing, P1.1 committed_bytes,
/// P1.3 primary_providers, P1.4 MemberBuckets) holds on state produced by the
/// pallet's own extrinsics.
#[test]
fn try_state_holds_on_real_state() {
    new_test_ext().execute_with(|| {
        register_provider(2, 200);
        let bucket = setup_agreement(2, 1, 100, 100);
        assert_ok!(StorageProvider::set_member(
            RuntimeOrigin::signed(1),
            bucket,
            3,
            Role::Reader
        ));

        assert_ok!(StorageProvider::do_try_state());
    });
}

/// P1.1: a `committed_bytes` that no longer matches the sum of agreement
/// `max_bytes` is caught.
#[test]
fn try_state_detects_committed_bytes_mismatch() {
    new_test_ext().execute_with(|| {
        register_provider(2, 200);
        setup_agreement(2, 1, 100, 100);
        assert_ok!(StorageProvider::do_try_state());

        Providers::<Test>::mutate(2, |p| {
            p.as_mut().unwrap().committed_bytes += 1;
        });
        assert!(StorageProvider::do_try_state().is_err());
    });
}

/// P1.3: a `primary_providers` entry without a matching Primary agreement is caught.
#[test]
fn try_state_detects_primary_providers_mismatch() {
    new_test_ext().execute_with(|| {
        register_provider(2, 200);
        let bucket = setup_agreement(2, 1, 100, 100);
        assert_ok!(StorageProvider::do_try_state());

        // Drop the agreement while leaving provider 2 in `primary_providers`.
        StorageAgreements::<Test>::remove(bucket, 2);
        assert!(StorageProvider::do_try_state().is_err());
    });
}

/// P1.4: a bucket member missing from the `MemberBuckets` reverse index is caught.
#[test]
fn try_state_detects_member_index_gap() {
    new_test_ext().execute_with(|| {
        create_bucket(1, 1);
        assert_ok!(StorageProvider::do_try_state());

        // Admin 1 is still a member of bucket 0 but no longer indexed.
        MemberBuckets::<Test>::remove(1);
        assert!(StorageProvider::do_try_state().is_err());
    });
}
