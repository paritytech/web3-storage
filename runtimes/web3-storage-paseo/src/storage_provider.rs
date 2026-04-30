//! Runtime configuration for `pallet_storage_provider`.
//!
//! Holds the runtime-level constants used by the storage provider pallet,
//! the `TreasuryAccount` that receives slashed funds, and the
//! `pallet_storage_provider::Config` impl for [`Runtime`].

use frame_support::{
    parameter_types,
    traits::{ConstU32, Get},
    PalletId,
};
use sp_runtime::traits::AccountIdConversion;

use crate::{
    paseo_constants::{currency::UNIT, time::HOURS},
    AccountId, Balance, Balances, BlockNumber, Runtime,
};

parameter_types! {
    pub const MinProviderStake: Balance = 1_000 * UNIT;  // 1000 tokens minimum stake
    pub const ChallengeTimeout: BlockNumber = 48 * HOURS;  // 48 hours to respond
    pub const SettlementTimeout: BlockNumber = 24 * HOURS;
    pub const RequestTimeout: BlockNumber = 6 * HOURS;
    // 1 token (1e12) per 1 GB (1e9 bytes) = 1000 per byte
    pub const MinStakePerByte: Balance = 1_000;
    pub const DefaultCheckpointInterval: BlockNumber = 100;
    pub const DefaultCheckpointGrace: BlockNumber = 20;
    pub const CheckpointReward: Balance = 1_000_000_000_000; // 1 token
    pub const CheckpointMissPenalty: Balance = 500_000_000_000; // 0.5 token
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
    type SettlementTimeout = SettlementTimeout;
    type RequestTimeout = RequestTimeout;
    type DefaultCheckpointInterval = DefaultCheckpointInterval;
    type DefaultCheckpointGrace = DefaultCheckpointGrace;
    type CheckpointReward = CheckpointReward;
    type CheckpointMissPenalty = CheckpointMissPenalty;
    type MaxBucketsPerMember = ConstU32<1000>;
    type WeightInfo = crate::weights::pallet_storage_provider::WeightInfo<Runtime>;
}
