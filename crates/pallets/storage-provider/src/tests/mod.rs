// SPDX-License-Identifier: Apache-2.0

//! Tests for the storage provider pallet.

use crate::{mock::*, *};
use frame_support::{assert_err, assert_noop, assert_ok};
use storage_primitives::{ProviderRole, Role};

/// Helper function to create a test public key (32 bytes).
fn test_public_key() -> frame_support::BoundedVec<u8, frame_support::traits::ConstU32<64>> {
    vec![1u8; 32].try_into().unwrap()
}

/// What this pallet is holding from `who` under one reason.
fn held(reason: HoldReason, who: u64) -> u64 {
    use frame_support::traits::fungible::InspectHold;
    Balances::balance_on_hold(&reason.into(), &who)
}

/// A provider that actually charges, so agreements escrow a non-zero amount.
/// The default mock settings price at zero, which would make hold assertions
/// trivially true.
fn priced_provider(who: u64, stake: u64) {
    register_provider_with_settings(
        who,
        stake,
        ProviderSettings {
            price_per_byte: 1,
            accepting_primary: true,
            ..Default::default()
        },
    );
}

/// Wipe a provider's stake the way a failed challenge does: slash the held
/// collateral into the treasury and zero the bookkeeping. Goes through the same
/// helper production uses, so the `Holds` ledger stays consistent with
/// `ProviderInfo::stake`.
fn slash_provider_stake(provider: u64) {
    Providers::<Test>::mutate(provider, |maybe_provider| {
        if let Some(info) = maybe_provider {
            let slashed = StorageProvider::slash_stake_to_treasury(&provider, info.stake);
            assert_eq!(slashed, info.stake, "entire stake should have been slashed");
            info.stake = 0;
        }
    });
}

mod agreement;
mod auto_matching;
mod bucket;
mod challenge;
mod checkpoint;
mod end_agreement;
mod error_paths;
mod extend_topup;
mod genesis;
mod holds;
mod member_buckets;
mod misc;
mod provider;
mod replica;
mod runtime_api;
mod try_state;
