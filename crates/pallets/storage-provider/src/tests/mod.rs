// SPDX-License-Identifier: Apache-2.0

//! Tests for the storage provider pallet.

use crate::{mock::*, *};
use frame_support::{assert_err, assert_noop, assert_ok};
use storage_primitives::{BucketTarget, ProviderRole, Role};

/// Helper function to create a test public key (32 bytes).
fn test_public_key() -> frame_support::BoundedVec<u8, frame_support::traits::ConstU32<64>> {
    vec![1u8; 32].try_into().unwrap()
}

/// Replica terms used by tests that only need some valid value.
fn replica_params() -> storage_primitives::ReplicaTerms<u64, u64> {
    storage_primitives::ReplicaTerms {
        sync_balance: 100,
        min_sync_interval: 10,
        sync_price: 10,
    }
}

/// Position of the first pallet event matching `pred` in this block's event
/// list.
fn event_position(pred: impl Fn(&Event<Test>) -> bool) -> usize {
    System::events()
        .iter()
        .position(|record| matches!(&record.event, RuntimeEvent::StorageProvider(e) if pred(e)))
        .expect("event emitted")
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

/// Register a provider that also takes replica agreements at a fixed sync price.
fn replica_provider(who: u64, stake: u64) {
    register_provider_with_settings(
        who,
        stake,
        ProviderSettings {
            price_per_byte: 1,
            accepting_primary: true,
            replica_sync_price: Some(10),
            ..Default::default()
        },
    );
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
mod primary_provider;
mod provider;
mod replica;
mod runtime_api;
mod signatures;
mod transfer;
mod try_state;
mod visibility;
