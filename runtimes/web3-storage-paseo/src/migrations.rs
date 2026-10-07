// SPDX-License-Identifier: GPL-3.0-only

//! Single-block storage migrations for the Paseo Web3 Storage runtime.
//!
//! Wired into the runtime via `frame_system::Config::SingleBlockMigrations`.
//! The SDK entries are gated on an on-chain storage version. The
//! `RemovePallet` entries are not gated; remove them once the upgrade that
//! adds them is deployed.

use crate::{RocksDbWeight, Runtime};
use frame_support::{migrations::RemovePallet, parameter_types};

parameter_types! {
    pub const DriveRegistryPalletName: &'static str = "DriveRegistry";
    pub const S3RegistryPalletName: &'static str = "S3Registry";
}

/// Storage migrations run on runtime upgrade, in order.
pub type Migrations = (
    // SDK `polkadot-stable2606` bumped both pallets' in-code storage versions.
    // Each migration is gated on the on-chain version, so both are no-ops once
    // applied.
    cumulus_pallet_parachain_system::migration::Migration<Runtime>,
    cumulus_pallet_xcmp_queue::migration::v7::MigrateV6ToV7<Runtime>,
    // `pallet-drive-registry` and `pallet-s3-registry` were removed (#475).
    // Clears all their storage in one block. Not gated on a version: once the
    // prefixes are empty it only costs one read per prefix, and the entries
    // can be dropped after the upgrade is deployed.
    RemovePallet<DriveRegistryPalletName, RocksDbWeight>,
    RemovePallet<S3RegistryPalletName, RocksDbWeight>,
);
