// SPDX-License-Identifier: Apache-2.0

//! `try_state` invariant checks exercised against real post-extrinsic state:
//! each implemented invariant holds on state the pallet's own extrinsics
//! produce, and a deliberate corruption of each is detected.

use super::*;

/// Every implemented invariant (P0 timing, P1.1 committed_bytes,
/// P1.3 primary_providers and the snapshot signer bitfield, P1.4 MemberBuckets)
/// holds on state produced by the pallet's own extrinsics.
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

/// P1.3: a snapshot signer bitfield shorter than `primary_providers` is caught.
#[test]
fn try_state_detects_signer_bitfield_length_mismatch() {
    new_test_ext().execute_with(|| {
        let bucket = super::challenge::setup_with_snapshot(2, 1);
        assert_ok!(StorageProvider::do_try_state());

        Buckets::<Test>::mutate(bucket, |b| {
            b.as_mut()
                .unwrap()
                .snapshot
                .as_mut()
                .unwrap()
                .primary_signers = Default::default();
        });
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

/// Storage-deposit holds equal the recorded tickets; a hold released behind
/// the pallet's back is caught.
#[test]
fn try_state_detects_storage_deposit_hold_mismatch() {
    use frame_support::traits::{fungible::MutateHold, tokens::Precision};

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

        assert_ok!(Balances::release(
            &HoldReason::StorageDeposit.into(),
            &1,
            10,
            Precision::Exact
        ));
        assert!(StorageProvider::do_try_state().is_err());
    });
}

/// A `StorageDeposit` hold on an account with no record behind it is a
/// ticket that was dropped from state without being released; it is caught
/// even though no record points at the account.
#[test]
fn try_state_detects_a_storage_deposit_hold_without_a_record() {
    use frame_support::traits::fungible::MutateHold;

    new_test_ext().execute_with(|| {
        create_bucket(1, 1);
        assert_ok!(StorageProvider::do_try_state());

        assert_ok!(Balances::hold(&HoldReason::StorageDeposit.into(), &5, 10));
        assert!(StorageProvider::do_try_state().is_err());
    });
}

/// A pending-challenge counter or a reverse-index entry left at zero is
/// caught.
#[test]
fn try_state_detects_leftover_empty_entries() {
    new_test_ext().execute_with(|| {
        create_bucket(1, 1);
        assert_ok!(StorageProvider::do_try_state());

        MemberBuckets::<Test>::insert(5, frame_support::BoundedVec::default());
        assert!(StorageProvider::do_try_state().is_err());
    });
}
/// An `AgreementNonces` entry is legitimate while its owner exists, and is
/// gone once the owner is reaped. An entry left behind for a missing account
/// (a runtime without the `OnKilledAccount` hook) is caught.
#[test]
fn try_state_detects_agreement_nonce_of_missing_account() {
    new_test_ext().execute_with(|| {
        register_provider(2, 200);
        let bucket_id = setup_agreement(2, 1, 100, 100);
        assert_eq!(AgreementNonces::<Test>::get(1), 1);
        assert_ok!(StorageProvider::do_try_state());

        // Reaping the owner removes the entry, so the invariant still holds.
        release_owner_records(1, bucket_id, 2);
        reap(1);
        assert!(!AgreementNonces::<Test>::contains_key(1));
        assert_ok!(StorageProvider::do_try_state());

        // An entry for an account that does not exist is caught.
        AgreementNonces::<Test>::insert(1, 1);
        assert!(StorageProvider::do_try_state().is_err());
    });
}

/// Open a challenge against provider 2 on a bucket with a snapshot. Opened at
/// block 1 with `ChallengeTimeout` 100, so its id is `(deadline 101, index 0)`.
fn bucket_with_an_open_challenge() -> u64 {
    use storage_primitives::ChunkLocation;
    frame_system::Pallet::<Test>::set_block_number(1);
    let bucket_id = super::challenge::setup_with_snapshot(2, 1);
    assert_ok!(StorageProvider::challenge_checkpoint(
        RuntimeOrigin::signed(3),
        bucket_id,
        2,
        ChunkLocation {
            leaf_index: 0,
            chunk_index: 0,
        },
    ));
    bucket_id
}

/// The pending-challenge counters match the open challenges on state the
/// pallet's own extrinsics produce, with a challenge open and after it is
/// cancelled.
#[test]
fn try_state_holds_with_an_open_and_a_cancelled_challenge() {
    new_test_ext().execute_with(|| {
        bucket_with_an_open_challenge();
        assert_ok!(StorageProvider::do_try_state());

        assert_ok!(StorageProvider::cancel_challenge(
            RuntimeOrigin::signed(3),
            ChallengeId {
                deadline: 101,
                index: 0,
            }
        ));
        assert_ok!(StorageProvider::do_try_state());
    });
}

/// A pending-challenge counter that does not equal the number of open
/// challenges is caught: too high, missing, zero, or present for a provider
/// or bucket with no challenge.
#[test]
fn try_state_detects_pending_challenge_counter_mismatch() {
    new_test_ext().execute_with(|| {
        let bucket_id = bucket_with_an_open_challenge();
        assert_ok!(StorageProvider::do_try_state());

        PendingChallenges::<Test>::insert(2, 2);
        assert!(StorageProvider::do_try_state().is_err());
        PendingChallenges::<Test>::insert(2, 1);
        assert_ok!(StorageProvider::do_try_state());

        PendingChallengesByBucket::<Test>::remove(bucket_id, 2);
        assert!(StorageProvider::do_try_state().is_err());
        PendingChallengesByBucket::<Test>::insert(bucket_id, 2, 1);
        assert_ok!(StorageProvider::do_try_state());

        PendingChallenges::<Test>::insert(7, 0);
        assert!(StorageProvider::do_try_state().is_err());
        PendingChallenges::<Test>::insert(7, 1);
        assert!(StorageProvider::do_try_state().is_err());
        PendingChallenges::<Test>::remove(7);
        assert_ok!(StorageProvider::do_try_state());

        PendingChallengesByBucket::<Test>::insert(bucket_id, 7, 1);
        assert!(StorageProvider::do_try_state().is_err());
    });
}
