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
    paseo_constants::{currency::UNIT, time::HOURS},
    AccountId, Balance, Balances, BlockNumber, Runtime, RuntimeEvent,
};

parameter_types! {
    pub const MinProviderStake: Balance = 1_000 * UNIT;  // 1000 tokens minimum stake
    pub const BaseChallengeDeposit: Balance = 100 * UNIT;
    pub const ChallengeTimeout: BlockNumber = 48 * HOURS;  // 48 hours to respond
    pub const SettlementTimeout: BlockNumber = 24 * HOURS;
    pub const RequestTimeout: BlockNumber = 6 * HOURS;
    // 1 token (1e12) per 1 GB (1e9 bytes) = 1000 per byte
    pub const MinStakePerByte: Balance = 1_000;
    pub const DefaultCheckpointInterval: BlockNumber = 100;
    pub const DefaultCheckpointGrace: BlockNumber = 20;
    pub const CheckpointReward: Balance = 1_000_000_000_000; // 1 token
    pub const CheckpointMissPenalty: Balance = 500_000_000_000; // 0.5 token
    /// Must be `>= ChallengeTimeout` so any challenge created up to the
    /// announcement block matures before the provider can withdraw stake.
    pub const DeregisterAnnouncementPeriod: BlockNumber = 48 * HOURS;
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
    type BaseChallengeDeposit = BaseChallengeDeposit;
    type ChallengeTimeout = ChallengeTimeout;
    type SettlementTimeout = SettlementTimeout;
    type RequestTimeout = RequestTimeout;
    type DefaultCheckpointInterval = DefaultCheckpointInterval;
    type DefaultCheckpointGrace = DefaultCheckpointGrace;
    type CheckpointReward = CheckpointReward;
    type CheckpointMissPenalty = CheckpointMissPenalty;
    type MaxBucketsPerMember = ConstU32<1000>;
    type DeregisterAnnouncementPeriod = DeregisterAnnouncementPeriod;
    type WeightInfo = crate::weights::pallet_storage_provider::WeightInfo<Runtime>;
}
