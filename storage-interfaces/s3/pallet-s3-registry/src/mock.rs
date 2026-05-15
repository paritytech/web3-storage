//! Mock runtime for S3 Registry pallet tests.

use crate as pallet_s3_registry;
use frame_support::{
    derive_impl, parameter_types,
    traits::{ConstU32, ConstU64},
};
use sp_core::H256;
use sp_runtime::{
    traits::{BlakeTwo256, IdentityLookup},
    BuildStorage,
};

type Block = frame_system::mocking::MockBlock<Test>;
type Balance = u128;

frame_support::construct_runtime!(
    pub enum Test {
        System: frame_system,
        Balances: pallet_balances,
        StorageProvider: pallet_storage_provider,
        S3Registry: pallet_s3_registry,
    }
);

#[derive_impl(frame_system::config_preludes::TestDefaultConfig)]
impl frame_system::Config for Test {
    type BaseCallFilter = frame_support::traits::Everything;
    type BlockWeights = ();
    type BlockLength = ();
    type DbWeight = ();
    type RuntimeOrigin = RuntimeOrigin;
    type RuntimeCall = RuntimeCall;
    type Nonce = u64;
    type Hash = H256;
    type Hashing = BlakeTwo256;
    type AccountId = u64;
    type Lookup = IdentityLookup<Self::AccountId>;
    type Block = Block;
    type RuntimeEvent = RuntimeEvent;
    type BlockHashCount = ConstU64<250>;
    type Version = ();
    type PalletInfo = PalletInfo;
    type AccountData = pallet_balances::AccountData<Balance>;
    type OnNewAccount = ();
    type OnKilledAccount = ();
    type SystemWeightInfo = ();
    type SS58Prefix = ();
    type OnSetCode = ();
    type MaxConsumers = ConstU32<16>;
}

parameter_types! {
    pub const ExistentialDeposit: Balance = 1;
}

impl pallet_balances::Config for Test {
    type MaxLocks = ();
    type MaxReserves = ConstU32<2>;
    type ReserveIdentifier = [u8; 8];
    type Balance = Balance;
    type RuntimeEvent = RuntimeEvent;
    type DustRemoval = ();
    type ExistentialDeposit = ExistentialDeposit;
    type AccountStore = System;
    type WeightInfo = ();
    type FreezeIdentifier = ();
    type MaxFreezes = ();
    type RuntimeHoldReason = ();
    type RuntimeFreezeReason = ();
    type DoneSlashHandler = ();
}

parameter_types! {
    pub const MinProviderStake: Balance = 1_000_000_000_000;
    pub const MinStakePerByte: Balance = 1_000;
    pub const MaxMultiaddrLength: u32 = 128;
    pub const MaxMembers: u32 = 100;
    pub const MaxPrimaryProviders: u32 = 5;
    pub const MaxChunkSize: u32 = 262144;
    pub const ChallengeTimeout: u64 = 100;
    pub const SettlementTimeout: u64 = 50;
    pub const RequestTimeout: u64 = 25;
    pub const DefaultCheckpointInterval: u64 = 100;
    pub const DefaultCheckpointGrace: u64 = 20;
    pub const CheckpointReward: Balance = 1_000_000_000_000;
    pub const CheckpointMissPenalty: Balance = 500_000_000_000;
    pub TreasuryAccount: u64 = 999;
}

impl pallet_storage_provider::Config for Test {
    type Currency = Balances;
    type Treasury = TreasuryAccount;
    type MinStakePerByte = MinStakePerByte;
    type MaxMultiaddrLength = ConstU32<128>;
    type MaxMembers = ConstU32<100>;
    type MaxPrimaryProviders = ConstU32<5>;
    type MaxBucketsPerMember = ConstU32<100>;
    type MinProviderStake = MinProviderStake;
    type MaxChunkSize = ConstU32<262144>;
    type ChallengeTimeout = ChallengeTimeout;
    type SettlementTimeout = SettlementTimeout;
    type RequestTimeout = RequestTimeout;
    type DefaultCheckpointInterval = DefaultCheckpointInterval;
    type DefaultCheckpointGrace = DefaultCheckpointGrace;
    type CheckpointReward = CheckpointReward;
    type CheckpointMissPenalty = CheckpointMissPenalty;
    type DeregisterAnnouncementPeriod = ConstU64<100>;
    type WeightInfo = ();
}

impl pallet_s3_registry::Config for Test {
    type MaxBucketsPerUser = ConstU32<100>;
    type MaxObjectsPerBucket = ConstU32<1000>;
    type WeightInfo = ();
}

/// Build test externalities.
pub fn new_test_ext() -> sp_io::TestExternalities {
    let mut t = frame_system::GenesisConfig::<Test>::default()
        .build_storage()
        .unwrap();

    pallet_balances::GenesisConfig::<Test> {
        balances: vec![
            (1, 100_000_000_000_000),
            (2, 100_000_000_000_000),
            (3, 100_000_000_000_000),
        ],
        dev_accounts: None,
    }
    .assimilate_storage(&mut t)
    .unwrap();

    let mut ext = sp_io::TestExternalities::new(t);
    ext.execute_with(|| {
        System::set_block_number(1);
    });
    ext
}
