// SPDX-License-Identifier: Apache-2.0

//! Funds are held under a [`HoldReason`], not reserved: each claim on an
//! account is tagged and individually addressable.

use super::*;

#[test]
fn provider_stake_is_held_under_its_own_reason() {
    new_test_ext().execute_with(|| {
        register_provider(2, 200);

        assert_eq!(held(HoldReason::ProviderStake, 2), 200);
        // Tagged, so it is not confusable with the other two claims.
        assert_eq!(held(HoldReason::AgreementPayment, 2), 0);
        assert_eq!(held(HoldReason::ChallengeDeposit, 2), 0);
        // …and it still shows up in the aggregate reserved figure, so nothing
        // that reads `reserved_balance` (explorers, other pallets) regresses.
        assert_eq!(Balances::reserved_balance(2), 200);
    });
}

#[test]
fn the_three_claims_coexist_on_one_account() {
    new_test_ext().execute_with(|| {
        // Account 2 wears all three hats at once: a registered provider, the
        // owner of an agreement it bought from another provider, and a
        // challenger. Under reserves these would be one indistinguishable
        // number; under holds each is separately accounted.
        register_provider(2, 200);
        priced_provider(4, 200);
        let bucket_id = setup_agreement(4, 2, 50, 100);
        let agreement = StorageAgreements::<Test>::get(bucket_id, 4).unwrap();

        assert_eq!(held(HoldReason::ProviderStake, 2), 200);
        assert_eq!(
            held(HoldReason::AgreementPayment, 2),
            agreement.payment_locked
        );
        assert_eq!(
            Balances::reserved_balance(2),
            200 + agreement.payment_locked
        );
    });
}

#[test]
fn releasing_stake_cannot_eat_into_escrow() {
    new_test_ext().execute_with(|| {
        // The failure mode reserves permitted: account 2 holds both collateral
        // and escrow, and a release of more stake than exists must fail rather
        // than silently draining the escrow sitting next to it.
        register_provider(2, 200);
        priced_provider(4, 200);
        let bucket_id = setup_agreement(4, 2, 50, 100);
        let escrow = StorageAgreements::<Test>::get(bucket_id, 4)
            .unwrap()
            .payment_locked;
        assert!(escrow > 0, "test needs a non-zero escrow to be meaningful");

        assert!(
            StorageProvider::release_stake(&2, 200 + escrow).is_err(),
            "over-release must fail, not spill into the neighbouring hold"
        );
        assert_eq!(held(HoldReason::AgreementPayment, 2), escrow);
    });
}

#[test]
fn slashing_stake_leaves_escrow_and_deposits_untouched() {
    new_test_ext().execute_with(|| {
        register_provider(2, 200);
        priced_provider(4, 200);
        let bucket_id = setup_agreement(4, 2, 50, 100);
        let escrow = StorageAgreements::<Test>::get(bucket_id, 4)
            .unwrap()
            .payment_locked;

        // Slashing targets the ProviderStake hold specifically.
        let slashed = StorageProvider::slash_stake_to_treasury(&2, 200);

        assert_eq!(slashed, 200);
        assert_eq!(held(HoldReason::ProviderStake, 2), 0);
        assert_eq!(
            held(HoldReason::AgreementPayment, 2),
            escrow,
            "a slash must not touch the account's escrowed storage fees"
        );
    });
}

#[test]
fn challenge_deposit_is_held_and_released_under_its_own_reason() {
    new_test_ext().execute_with(|| {
        register_provider(3, 200);

        assert_ok!(StorageProvider::hold_challenge_deposit(&3, 100));
        assert_eq!(held(HoldReason::ChallengeDeposit, 3), 100);
        assert_eq!(
            held(HoldReason::ProviderStake, 3),
            200,
            "the deposit must not be confused with this account's collateral"
        );

        StorageProvider::release_challenge_deposit(&3, 100);
        assert_eq!(held(HoldReason::ChallengeDeposit, 3), 0);
        assert_eq!(held(HoldReason::ProviderStake, 3), 200);
    });
}

#[test]
fn settling_an_agreement_clears_the_escrow_hold() {
    new_test_ext().execute_with(|| {
        priced_provider(2, 200);
        let bucket_id = setup_agreement(2, 1, 50, 100);
        let escrow = StorageAgreements::<Test>::get(bucket_id, 2)
            .unwrap()
            .payment_locked;
        assert!(escrow > 0);
        assert_eq!(held(HoldReason::AgreementPayment, 1), escrow);

        let provider_before = Balances::free_balance(2);
        run_to_block(101);
        assert_ok!(StorageProvider::end_agreement(
            RuntimeOrigin::signed(1),
            bucket_id,
            2,
            storage_primitives::EndAction::Pay,
        ));

        assert_eq!(
            held(HoldReason::AgreementPayment, 1),
            0,
            "settlement must leave nothing stranded on hold"
        );
        assert_eq!(Balances::free_balance(2), provider_before + escrow);
    });
}

#[test]
fn third_party_extension_escrows_on_the_owner() {
    new_test_ext().execute_with(|| {
        // Extending is permissionless while the price has not risen — account 5
        // keeps account 1's data alive. The escrow must still land on the
        // owner, because that is the account settlement pays out from;
        // escrowing it on the payer would leave `payment_locked` and the hold
        // on two different accounts, and settlement would find nothing there.
        priced_provider(2, 200);
        let bucket_id = setup_agreement(2, 1, 50, 100);
        let before = StorageAgreements::<Test>::get(bucket_id, 2).unwrap();
        let payer_free_before = Balances::free_balance(5);

        assert_ok!(StorageProvider::extend_agreement(
            RuntimeOrigin::signed(5),
            bucket_id,
            2,
            50,
            10_000,
        ));

        let after = StorageAgreements::<Test>::get(bucket_id, 2).unwrap();
        let added = after.payment_locked - before.payment_locked;
        assert!(added > 0, "extension should have escrowed something");
        assert_eq!(
            held(HoldReason::AgreementPayment, 1),
            after.payment_locked,
            "the whole escrow sits on the owner, matching payment_locked"
        );
        assert_eq!(held(HoldReason::AgreementPayment, 5), 0);
        assert_eq!(Balances::free_balance(5), payer_free_before - added);

        // The invariant check agrees, and settlement can now actually pay out.
        assert_ok!(StorageProvider::do_try_state());
        let provider_before = Balances::free_balance(2);
        // The extension re-based the agreement, so settle within the window
        // that starts at the *new* expiry, not the original one.
        run_to_block(after.expires_at + 1);
        assert_ok!(StorageProvider::end_agreement(
            RuntimeOrigin::signed(1),
            bucket_id,
            2,
            storage_primitives::EndAction::Pay,
        ));
        assert_eq!(held(HoldReason::AgreementPayment, 1), 0);
        assert_eq!(
            Balances::free_balance(2),
            provider_before + after.payment_locked
        );
    });
}

#[test]
fn try_state_catches_bookkeeping_with_no_funds_behind_it() {
    new_test_ext().execute_with(|| {
        register_provider(2, 200);
        assert_ok!(StorageProvider::do_try_state());

        // Inflate the recorded stake without holding anything more: exactly the
        // drift that an opaque `reserved` figure could not have detected.
        Providers::<Test>::mutate(2, |maybe_provider| {
            if let Some(info) = maybe_provider {
                info.stake = 500;
            }
        });

        assert!(StorageProvider::do_try_state().is_err());
    });
}
