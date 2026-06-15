//! Single-block storage migrations for the Paseo Web3 Storage runtime.
//!
//! Wired into the runtime via `frame_system::Config::SingleBlockMigrations`.
//! Each entry is idempotent (a `VersionedMigration` or an idempotent SDK
//! built-in), so the tuple is safe to leave in place across releases.

use crate::Runtime;
use frame_support::{parameter_types, weights::constants::RocksDbWeight};

parameter_types! {
    /// `StorageProvider`'s `AgreementRequests` map was deleted in #105; these
    /// name it for the `RemoveStorage` cleanup below.
    pub const StorageProviderPalletName: &'static str = "StorageProvider";
    pub const AgreementRequestsStorageName: &'static str = "AgreementRequests";
}

/// Storage migrations run on runtime upgrade, in order.
pub type Migrations = (
    // Purge the orphaned `AgreementRequests` entries left by #105. The SDK
    // built-in is idempotent (a no-op once the prefix is empty), so it needs no
    // storage-version gating and is safe to leave in place.
    frame_support::migrations::RemoveStorage<
        StorageProviderPalletName,
        AgreementRequestsStorageName,
        RocksDbWeight,
    >,
    // Drop the `payment` field from `DriveInfo` (#105). A real data transform,
    // so it stays a `VersionedMigration` gated on the pallet's storage version.
    pallet_drive_registry::migrations::v1::MigrateV0ToV1<Runtime>,
);
