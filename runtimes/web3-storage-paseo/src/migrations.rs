// SPDX-License-Identifier: GPL-3.0-only

//! Single-block storage migrations for the Paseo Web3 Storage runtime.
//!
//! Wired into the runtime via `frame_system::Config::SingleBlockMigrations`.
//! Each entry is gated on an on-chain storage version, so the tuple is safe to
//! leave in place across releases.

use crate::Runtime;

/// Storage migrations run on runtime upgrade, in order.
pub type Migrations = (
    // Drop the `payment` field from `DriveInfo` (#105). A real data transform,
    // so it stays a `VersionedMigration` gated on the pallet's storage version.
    pallet_drive_registry::migrations::v1::MigrateV0ToV1<Runtime>,
    // SDK `polkadot-stable2606` bumped both pallets' in-code storage versions.
    // Each migration is gated on the on-chain version, so both are no-ops once
    // applied.
    cumulus_pallet_parachain_system::migration::Migration<Runtime>,
    cumulus_pallet_xcmp_queue::migration::v7::MigrateV6ToV7<Runtime>,
);
