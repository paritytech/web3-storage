use crate as pallet_drive_registry;
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

// Configure a mock runtime to test the pallet.
frame_support::construct_runtime!(
    pub enum Test
    {
        System: frame_system,
        Balances: pallet_balances,
        StorageProvider: pallet_storage_provider,
        DriveRegistry: pallet_drive_registry,
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
    pub const MinProviderStake: Balance = 1_000_000_000_000; // 1 token
    pub const MinStakePerByte: Balance = 1_000; // 1_000 per byte
    pub const MaxMultiaddrLength: u32 = 100;
    pub const MaxMembers: u32 = 10;
    pub const MaxPrimaryProviders: u32 = 3;
    pub const MaxChunkSize: u32 = 256 * 1024; // 256 KiB
    pub const ChallengeTimeout: u64 = 100;
    pub const SettlementTimeout: u64 = 50;
    pub const RequestTimeout: u64 = 50;
    pub TreasuryAccount: u64 = 999; // Treasury account
    pub const DefaultCheckpointInterval: u64 = 100;
    pub const DefaultCheckpointGrace: u64 = 20;
    pub const CheckpointReward: u64 = 1_000_000_000_000;
    pub const CheckpointMissPenalty: u64 = 500_000_000_000;
}

impl pallet_storage_provider::Config for Test {
    type Currency = Balances;
    type Treasury = TreasuryAccount;
    type MinStakePerByte = MinStakePerByte;
    type MaxMultiaddrLength = MaxMultiaddrLength;
    type MaxMembers = MaxMembers;
    type MaxPrimaryProviders = MaxPrimaryProviders;
    type MaxBucketsPerMember = ConstU32<100>;
    type MinProviderStake = MinProviderStake;
    type MaxChunkSize = MaxChunkSize;
    type ChallengeTimeout = ChallengeTimeout;
    type SettlementTimeout = SettlementTimeout;
    type RequestTimeout = RequestTimeout;
    type DefaultCheckpointInterval = DefaultCheckpointInterval;
    type DefaultCheckpointGrace = DefaultCheckpointGrace;
    type CheckpointReward = CheckpointReward;
    type CheckpointMissPenalty = CheckpointMissPenalty;
    type WeightInfo = ();
}

parameter_types! {
    pub const MaxDrivesPerUser: u32 = 100;
    pub const MaxDriveNameLength: u32 = 256;
}

impl pallet_drive_registry::Config for Test {
    type RuntimeEvent = RuntimeEvent;
    type MaxDrivesPerUser = MaxDrivesPerUser;
    type MaxDriveNameLength = MaxDriveNameLength;
    type WeightInfo = ();
}

// Build genesis storage according to the mock runtime.
pub fn new_test_ext() -> sp_io::TestExternalities {
    let mut t = frame_system::GenesisConfig::<Test>::default()
        .build_storage()
        .unwrap();

    // Give test accounts some initial balance
    pallet_balances::GenesisConfig::<Test> {
        balances: vec![
            (1, 100_000_000_000_000), // Alice: 100 tokens
            (2, 100_000_000_000_000), // Bob: 100 tokens
            (3, 100_000_000_000_000), // Charlie: 100 tokens
        ],
        dev_accounts: None,
    }
    .assimilate_storage(&mut t)
    .unwrap();

    t.into()
}
