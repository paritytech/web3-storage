// SPDX-License-Identifier: Apache-2.0

//! Ownership transfer: the escrow follows the owner, and every owner-only
//! call follows with it.

use super::*;

const PROVIDER: u64 = 2;
const OWNER: u64 = 1;
const NEW_OWNER: u64 = 4;

fn transfer(
    who: u64,
    bucket_id: u64,
    provider: u64,
    new_owner: u64,
) -> frame_support::pallet_prelude::DispatchResult {
    StorageProvider::transfer_agreement_ownership(
        RuntimeOrigin::signed(who),
        bucket_id,
        provider,
        new_owner,
    )
}

#[test]
fn transfer_moves_owner_and_escrow_together() {
    new_test_ext().execute_with(|| {
        // Events are only recorded from block 1 on.
        System::set_block_number(1);
        priced_provider(PROVIDER, 200);
        let bucket_id = setup_agreement(PROVIDER, OWNER, 50, 100);
        let escrow = held(HoldReason::AgreementPayment, OWNER);
        assert!(escrow > 0, "test needs a non-zero escrow to be meaningful");

        assert_ok!(transfer(OWNER, bucket_id, PROVIDER, NEW_OWNER));

        let agreement = StorageAgreements::<Test>::get(bucket_id, PROVIDER).unwrap();
        assert_eq!(agreement.owner, NEW_OWNER);
        assert_eq!(held(HoldReason::AgreementPayment, OWNER), 0);
        assert_eq!(held(HoldReason::AgreementPayment, NEW_OWNER), escrow);
        System::assert_has_event(
            Event::<Test>::AgreementOwnershipTransferred {
                bucket_id,
                provider: PROVIDER,
                old_owner: OWNER,
                new_owner: NEW_OWNER,
            }
            .into(),
        );
        assert_ok!(StorageProvider::do_try_state());
    });
}

#[test]
fn only_the_owner_can_transfer_and_only_to_someone_else() {
    new_test_ext().execute_with(|| {
        priced_provider(PROVIDER, 200);
        let bucket_id = setup_agreement(PROVIDER, OWNER, 50, 100);

        assert_noop!(
            transfer(3, bucket_id, PROVIDER, NEW_OWNER),
            Error::<Test>::NotAgreementOwner
        );
        assert_noop!(
            transfer(OWNER, bucket_id, PROVIDER, OWNER),
            Error::<Test>::TransferToSelf
        );
        assert_noop!(
            transfer(OWNER, 999, PROVIDER, NEW_OWNER),
            Error::<Test>::AgreementNotFound
        );
    });
}

#[test]
fn owner_only_calls_follow_the_transfer() {
    new_test_ext().execute_with(|| {
        priced_provider(PROVIDER, 200);
        let bucket_id = setup_agreement(PROVIDER, OWNER, 50, 100);
        assert_ok!(transfer(OWNER, bucket_id, PROVIDER, NEW_OWNER));

        assert_noop!(
            StorageProvider::top_up_agreement(
                RuntimeOrigin::signed(OWNER),
                bucket_id,
                PROVIDER,
                10,
                10_000
            ),
            Error::<Test>::NotAgreementOwner
        );
        assert_ok!(StorageProvider::top_up_agreement(
            RuntimeOrigin::signed(NEW_OWNER),
            bucket_id,
            PROVIDER,
            10,
            10_000
        ));

        // The new owner can hand it on again.
        assert_ok!(transfer(NEW_OWNER, bucket_id, PROVIDER, 5));
        assert_eq!(
            StorageAgreements::<Test>::get(bucket_id, PROVIDER)
                .unwrap()
                .owner,
            5
        );
        assert_ok!(StorageProvider::do_try_state());
    });
}

#[test]
fn replica_transfer_moves_the_sync_balance_too() {
    new_test_ext().execute_with(|| {
        const REPLICA: u64 = 3;
        replica_provider(REPLICA, 200);
        let bucket_id = create_bucket(OWNER, 0);
        setup_replica_agreement(
            REPLICA,
            OWNER,
            bucket_id,
            50,
            100,
            storage_primitives::ReplicaTerms {
                sync_balance: 100,
                min_sync_interval: 10,
                sync_price: 10,
            },
        );
        let fee = StorageAgreements::<Test>::get(bucket_id, REPLICA)
            .unwrap()
            .payment_locked;
        assert_eq!(held(HoldReason::AgreementPayment, OWNER), fee + 100);

        assert_ok!(transfer(OWNER, bucket_id, REPLICA, NEW_OWNER));

        assert_eq!(held(HoldReason::AgreementPayment, OWNER), 0);
        assert_eq!(held(HoldReason::AgreementPayment, NEW_OWNER), fee + 100);
        assert_ok!(StorageProvider::do_try_state());
    });
}
