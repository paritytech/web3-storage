// SPDX-License-Identifier: Apache-2.0

//! Every movement of money this pallet makes, in one place.
//!
//! Each helper fixes a [`Precision`]; that is why they exist rather than being
//! inlined. The split is per [`HoldReason`], not per caller:
//!
//! * Stake and agreement money: [`Precision::Exact`], errors propagate. The
//!   bookkeeping says exactly what is held, so a shortfall is a broken
//!   invariant, not something to silently under-pay.
//! * Challenge money and slashes: [`Precision::BestEffort`], infallible.
//!   [`Pallet::slash_provider_for_failed_challenge`] is shared with the
//!   `on_initialize` sweep, which has no caller to return an error to.

use crate::*;
use frame_support::{
    defensive,
    pallet_prelude::*,
    traits::{
        fungible::{Balanced, BalancedHold, MutateHold},
        tokens::{Fortitude, Precision, Preservation, Restriction},
    },
};
use sp_runtime::{traits::Zero, Saturating};

impl<T: Config> Pallet<T> {
    /// Hold a provider's collateral. The only hold that is ever slashed.
    pub(crate) fn hold_stake(who: &T::AccountId, amount: BalanceOf<T>) -> DispatchResult {
        T::Currency::hold(&HoldReason::ProviderStake.into(), who, amount)
    }

    /// Return collateral once the provider is no longer slashable.
    pub(crate) fn release_stake(who: &T::AccountId, amount: BalanceOf<T>) -> DispatchResult {
        Self::release_exact(&HoldReason::ProviderStake.into(), who, amount)
    }

    /// Escrow a client's prepaid storage fee.
    pub(crate) fn hold_payment(who: &T::AccountId, amount: BalanceOf<T>) -> DispatchResult {
        T::Currency::hold(&HoldReason::AgreementPayment.into(), who, amount)
    }

    /// Escrow for an agreement on behalf of `payer`.
    ///
    /// `extend_agreement` and `top_up_replica_sync_balance` are permissionless,
    /// but settlement always pays out of the owner's hold — so a third party's
    /// funds move to the owner first. Otherwise the hold and `payment_locked`
    /// would sit on different accounts.
    pub(crate) fn escrow_from(
        payer: &T::AccountId,
        owner: &T::AccountId,
        amount: BalanceOf<T>,
    ) -> DispatchResult {
        if amount.is_zero() {
            return Ok(());
        }
        if payer == owner {
            return Self::hold_payment(owner, amount);
        }
        // One step: never briefly spendable on the owner's account.
        T::Currency::transfer_and_hold(
            &HoldReason::AgreementPayment.into(),
            payer,
            owner,
            amount,
            Precision::Exact,
            Preservation::Preserve,
            Fortitude::Polite,
        )
        .map(|_| ())
    }

    /// Return escrowed fee to the owner.
    pub(crate) fn release_payment(who: &T::AccountId, amount: BalanceOf<T>) -> DispatchResult {
        Self::release_exact(&HoldReason::AgreementPayment.into(), who, amount)
    }

    /// Pay escrowed fee out to `dest` (provider, or treasury for the burned
    /// share). Atomic, so the funds are never briefly spendable mid-settlement.
    pub(crate) fn settle_payment(
        owner: &T::AccountId,
        dest: &T::AccountId,
        amount: BalanceOf<T>,
    ) -> DispatchResult {
        if amount.is_zero() {
            return Ok(());
        }
        T::Currency::transfer_on_hold(
            &HoldReason::AgreementPayment.into(),
            owner,
            dest,
            amount,
            Precision::Exact,
            Restriction::Free,
            Fortitude::Polite,
        )
        .map(|_| ())
    }

    /// Hold a challenger's anti-spam deposit.
    pub(crate) fn hold_challenge_deposit(
        who: &T::AccountId,
        amount: BalanceOf<T>,
    ) -> DispatchResult {
        T::Currency::hold(&HoldReason::ChallengeDeposit.into(), who, amount)
    }

    /// Refund a challenger's deposit.
    pub(crate) fn release_challenge_deposit(who: &T::AccountId, amount: BalanceOf<T>) {
        let _ = T::Currency::release(
            &HoldReason::ChallengeDeposit.into(),
            who,
            amount,
            Precision::BestEffort,
        );
    }

    /// Pay the provider their share of a deposit for the work of responding.
    /// Returns what moved, so the caller refunds the rest instead of stranding
    /// it.
    pub(crate) fn pay_challenge_deposit(
        challenger: &T::AccountId,
        provider: &T::AccountId,
        amount: BalanceOf<T>,
    ) -> BalanceOf<T> {
        if amount.is_zero() {
            return Zero::zero();
        }
        T::Currency::transfer_on_hold(
            &HoldReason::ChallengeDeposit.into(),
            challenger,
            provider,
            amount,
            Precision::BestEffort,
            Restriction::Free,
            Fortitude::Polite,
        )
        .unwrap_or_else(|_| Zero::zero())
    }

    /// Slash held collateral into the treasury, leaving total issuance
    /// unchanged. Returns what was slashed, for the caller to write back to
    /// `ProviderInfo::stake`.
    pub(crate) fn slash_stake_to_treasury(
        who: &T::AccountId,
        amount: BalanceOf<T>,
    ) -> BalanceOf<T> {
        if amount.is_zero() {
            return Zero::zero();
        }
        let (credit, not_slashed) =
            T::Currency::slash(&HoldReason::ProviderStake.into(), who, amount);
        let slashed = amount.saturating_sub(not_slashed);
        // If the treasury cannot accept the credit, dropping it burns the
        // funds. That is the safe failure, but it must not be silent.
        if let Err(dropped) = T::Currency::resolve(&T::Treasury::get(), credit) {
            defensive!("storage-provider: treasury refused slashed funds; burning");
            drop(dropped);
        }
        slashed
    }

    fn release_exact(
        reason: &T::RuntimeHoldReason,
        who: &T::AccountId,
        amount: BalanceOf<T>,
    ) -> DispatchResult {
        if amount.is_zero() {
            return Ok(());
        }
        T::Currency::release(reason, who, amount, Precision::Exact).map(|_| ())
    }
}
