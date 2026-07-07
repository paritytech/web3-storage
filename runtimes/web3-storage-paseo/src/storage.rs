// SPDX-License-Identifier: GPL-3.0-only

//! Storage configuration
//!

use frame_support::{
    parameter_types,
    traits::{ConstU32, Get},
    PalletId,
};
use sp_runtime::traits::AccountIdConversion;

use crate::{
    paseo_constants::{currency::UNIT, relay_time::HOURS},
    AccountId, Balance, Balances, BlockNumber, Runtime, RuntimeEvent,
};

// Storage-backed parameters: each value below is the default, but can be
// overridden at runtime via `system.setStorage` (under sudo/governance) without
// a runtime upgrade. Useful for tuning previewnet timing without redeploying the
// wasm. Each `pub storage X` exposes `X::key()` / `X::set()` and reads the
// current value from unhashed storage, falling back to the default.
//
// Every duration below is measured in RELAY chain blocks (6s), not parachain
// blocks: the storage pallet reads its clock from
// `cumulus_pallet_parachain_system::RelaychainDataProvider`, so these keep
// their wall-clock meaning when the parachain block time changes.
parameter_types! {
    pub storage MinProviderStake: Balance = 1_000 * UNIT;  // 1000 tokens minimum stake
    pub storage ChallengeTimeout: BlockNumber = 48 * HOURS;
    // Replay-protection window for `CommitmentPayload::nonce`. A signature whose
    // nonce is older than this is rejected. Set wide enough to accommodate
    // normal off-chain choreography (provider signs, client builds & broadcasts
    // tx, tx finalises) without forcing re-signing.
    pub storage MaxNonceAge: BlockNumber = 24 * HOURS;
    // Reserved from the challenger when opening a challenge. 1 token at 12
    // decimals = floor on spam economics. Previously hardcoded `100u32`
    // (1e-10 of a token) which made challenge spam effectively free.
    pub storage ChallengeDeposit: Balance = UNIT;
    pub storage SettlementTimeout: BlockNumber = 24 * HOURS;
    pub storage RequestTimeout: BlockNumber = 6 * HOURS;
    // 1 token (1e12) per 1 GB (1e9 bytes) = 1000 per byte
    pub storage MinStakePerByte: Balance = 1_000;
    pub storage DefaultCheckpointInterval: BlockNumber = 100; // relay blocks (~10 min)
    pub storage DefaultCheckpointGrace: BlockNumber = 20; // relay blocks (~2 min)
    pub storage CheckpointReward: Balance = 1_000_000_000_000; // 1 token
    pub storage CheckpointMissPenalty: Balance = 500_000_000_000; // 0.5 token
    /// Must be `> ChallengeTimeout` so any challenge opened up to the
    /// announcement block matures (provider stays slashable) before the
    /// provider can withdraw stake, and `> RequestTimeout` so a
    /// pre-deregistration agreement quote expires before re-registration (the
    /// re-register replay defense). Both are checked in `integrity_test`.
    /// Value: the 48h challenge window plus a 6h grace.
    pub storage DeregisterAnnouncementPeriod: BlockNumber = 54 * HOURS;
    /// Caps the challenges sharing one deadline (relay block) and the
    /// `on_initialize` sweep's per-block slash budget. Generous: only
    /// challenges created while the chain sits on the same relay parent
    /// share a deadline.
    pub storage MaxChallengesPerDeadline: u16 = 1_000;
}

/// Treasury account that receives slashed funds.
///
/// Derived from a well-known `PalletId`. In production this would be backed by a
/// proper treasury pallet.
pub struct TreasuryAccount;
impl Get<AccountId> for TreasuryAccount {
    fn get() -> AccountId {
        AccountIdConversion::<AccountId>::into_account_truncating(&PalletId(*b"py/trsry"))
    }
}

// --------------------------------
// Drive Registry Pallet Config
// --------------------------------

impl pallet_drive_registry::Config for Runtime {
    type RuntimeEvent = RuntimeEvent;
    type MaxDriveNameLength = ConstU32<128>;
    type MaxDrivesPerUser = ConstU32<100>;
    type WeightInfo = crate::weights::pallet_drive_registry::WeightInfo<Runtime>;
}

// --------------------------------
// S3 Registry Pallet Config
// --------------------------------

impl pallet_s3_registry::Config for Runtime {
    type MaxBucketsPerUser = ConstU32<100>;
    type MaxObjectsPerBucket = ConstU32<100000>;
    type WeightInfo = crate::weights::pallet_s3_registry::WeightInfo<Runtime>;
}

// --------------------------------
// Storage Provider Pallet Config
// --------------------------------

impl pallet_storage_provider::Config for Runtime {
    type Currency = Balances;
    type Treasury = TreasuryAccount;
    type MinStakePerByte = MinStakePerByte;
    type MaxMultiaddrLength = ConstU32<128>;
    type MaxMembers = ConstU32<100>;
    type MaxPrimaryProviders = ConstU32<5>;
    type MinProviderStake = MinProviderStake;
    type MaxChunkSize = ConstU32<262144>; // 256 KiB
    type ChallengeTimeout = ChallengeTimeout;
    type ChallengeDeposit = ChallengeDeposit;
    type MaxNonceAge = MaxNonceAge;
    type SettlementTimeout = SettlementTimeout;
    type RequestTimeout = RequestTimeout;
    type DefaultCheckpointInterval = DefaultCheckpointInterval;
    type DefaultCheckpointGrace = DefaultCheckpointGrace;
    type CheckpointReward = CheckpointReward;
    type CheckpointMissPenalty = CheckpointMissPenalty;
    type MaxBucketsPerMember = ConstU32<1000>;
    type DeregisterAnnouncementPeriod = DeregisterAnnouncementPeriod;
    type MaxChallengesPerDeadline = MaxChallengesPerDeadline;
    type BlockNumberProvider = cumulus_pallet_parachain_system::RelaychainDataProvider<Runtime>;
    type WeightInfo = crate::weights::pallet_storage_provider::WeightInfo<Runtime>;
}
