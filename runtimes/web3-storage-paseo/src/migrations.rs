// SPDX-License-Identifier: GPL-3.0-only

//! Single-block storage migrations for the Paseo Web3 Storage runtime.
//!
//! Wired into the runtime via `frame_system::Config::SingleBlockMigrations`.
//! Each entry is gated on an on-chain storage version or is itself idempotent,
//! so the tuple is safe to leave in place across releases.

use crate::{Balances, Runtime, RuntimeHoldReason};
use frame_support::{
    defensive,
    traits::{
        fungible::{InspectHold, MutateHold},
        OnRuntimeUpgrade, ReservableCurrency,
    },
    weights::Weight,
};
use pallet_storage_provider::{HoldReason, Providers};
use sp_runtime::traits::Zero;

/// Records each provider's stake as a `ProviderStake` hold.
///
/// #372 replaced `ReservableCurrency` with the `fungible` hold API but shipped
/// no migration, so stake locked by an earlier runtime sits in the account's
/// `reserved` balance with no matching entry in `Balances::Holds`. The pallet
/// reads `balance_on_hold`, which only consults `Holds`, so it sees zero and
/// its `try_state` bookkeeping check fails.
///
/// Unreserving and immediately holding the same amount leaves `free` and
/// `reserved` exactly as they were and only adds the missing `Holds` entry. No
/// balance moves. Providers that already hold their stake are skipped, so
/// re-running this is a no-op.
pub struct RecordProviderStakeAsHold;

impl OnRuntimeUpgrade for RecordProviderStakeAsHold {
    fn on_runtime_upgrade() -> Weight {
        let reason: RuntimeHoldReason = HoldReason::ProviderStake.into();
        let mut reads = 0u64;
        let mut writes = 0u64;

        for (provider, info) in Providers::<Runtime>::iter() {
            reads = reads.saturating_add(1);

            // Already migrated, or nothing to record.
            if info.stake.is_zero() || !Balances::balance_on_hold(&reason, &provider).is_zero() {
                continue;
            }
            // Without this the unreserve below would come up short and the
            // hold would then fail, leaving the stake neither reserved nor
            // held.
            if Balances::reserved_balance(&provider) < info.stake {
                defensive!(
                    "paseo migration: provider reserved balance is below its recorded stake"
                );
                continue;
            }

            Balances::unreserve(&provider, info.stake);
            match Balances::hold(&reason, &provider, info.stake) {
                Ok(()) => writes = writes.saturating_add(1),
                Err(_) => {
                    // Unreachable after the guard above, but leaving the stake
                    // neither reserved nor held would be worse than not
                    // converting it.
                    defensive!("paseo migration: holding a provider stake failed; re-reserving");
                    let _ = Balances::reserve(&provider, info.stake);
                }
            }
        }

        // Each converted provider costs an unreserve and a hold on top of the
        // read of its record.
        <Runtime as frame_system::Config>::DbWeight::get()
            .reads_writes(reads, writes.saturating_mul(2))
    }
}

/// Storage migrations run on runtime upgrade, in order.
pub type Migrations = (
    // Re-encode `Buckets` and `Providers` after the `Bucket::visibility`
    // (#330, #419) and `ProviderStats` (#330, #397) layout changes, which
    // shipped without one.
    pallet_storage_provider::migrations::v1::MigrateV0ToV1<Runtime>,
    // Must follow the re-encode above, which is what makes `Providers`
    // readable again.
    RecordProviderStakeAsHold,
    // Drop the `payment` field from `DriveInfo` (#105). A real data transform,
    // so it stays a `VersionedMigration` gated on the pallet's storage version.
    pallet_drive_registry::migrations::v1::MigrateV0ToV1<Runtime>,
    // SDK `polkadot-stable2606` bumped both pallets' in-code storage versions.
    // Each migration is gated on the on-chain version, so both are no-ops once
    // applied.
    cumulus_pallet_parachain_system::migration::Migration<Runtime>,
    cumulus_pallet_xcmp_queue::migration::v7::MigrateV6ToV7<Runtime>,
);
