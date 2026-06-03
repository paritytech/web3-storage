//! Mock runtime for testing the storage provider pallet.

use crate as pallet_storage_provider;
use frame_support::{
    derive_impl,
    traits::{ConstU32, ConstU64, Hooks},
};
use sp_core::H256;
use sp_runtime::{
    traits::{BlakeTwo256, IdentityLookup},
    BuildStorage,
};

type Block = frame_system::mocking::MockBlock<Test>;

frame_support::construct_runtime!(
    pub enum Test {
        System: frame_system,
        Balances: pallet_balances,
        StorageProvider: pallet_storage_provider,
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
    type AccountData = pallet_balances::AccountData<u64>;
    type OnNewAccount = ();
    type OnKilledAccount = ();
    type SystemWeightInfo = ();
    type SS58Prefix = ();
    type OnSetCode = ();
    type MaxConsumers = ConstU32<16>;
}

impl pallet_balances::Config for Test {
    type MaxLocks = ConstU32<50>;
    type MaxReserves = ();
    type ReserveIdentifier = [u8; 8];
    type Balance = u64;
    type RuntimeEvent = RuntimeEvent;
    type DustRemoval = ();
    type ExistentialDeposit = ConstU64<1>;
    type AccountStore = System;
    type WeightInfo = ();
    type FreezeIdentifier = ();
    type MaxFreezes = ();
    type RuntimeHoldReason = ();
    type RuntimeFreezeReason = ();
    type DoneSlashHandler = ();
}

// Treasury account for testing
pub struct TestTreasury;
impl frame_support::traits::Get<u64> for TestTreasury {
    fn get() -> u64 {
        999 // Treasury is account 999 in tests
    }
}

impl pallet_storage_provider::Config for Test {
    type Currency = Balances;
    type Treasury = TestTreasury;
    type MinStakePerByte = ConstU64<1>; // 1 unit per byte
    type MaxMultiaddrLength = ConstU32<128>;
    type MaxMembers = ConstU32<100>;
    type MaxPrimaryProviders = ConstU32<5>;
    type MinProviderStake = ConstU64<100>;
    type MaxChunkSize = ConstU32<262144>; // 256 KiB
    type ChallengeTimeout = ConstU64<100>;
    type MaxNonceAge = ConstU64<200>;
    type SettlementTimeout = ConstU64<50>;
    type RequestTimeout = ConstU64<100>;
    // Provider-initiated checkpoint config
    type DefaultCheckpointInterval = ConstU64<10>; // 10 blocks for testing
    type DefaultCheckpointGrace = ConstU64<5>; // 5 blocks grace
    type CheckpointReward = ConstU64<10>; // 10 units reward
    type CheckpointMissPenalty = ConstU64<50>; // 50 units penalty
    type MaxBucketsPerMember = ConstU32<100>;
    // Must be >= ChallengeTimeout (100 in this mock). Set to a small
    // multiple so tests can advance past the period quickly.
    type DeregisterAnnouncementPeriod = ConstU64<100>;
    type WeightInfo = ();
}

/// Build test externalities with default balances.
pub fn new_test_ext() -> sp_io::TestExternalities {
    let mut t = frame_system::GenesisConfig::<Test>::default()
        .build_storage()
        .unwrap();

    pallet_balances::GenesisConfig::<Test> {
        balances: vec![
            (1, 10_000),
            (2, 10_000),
            (3, 10_000),
            (4, 10_000),
            (5, 10_000),
            (6, 10_000),
            (7, 10_000),
            (8, 10_000),
        ],
        dev_accounts: None,
    }
    .assimilate_storage(&mut t)
    .unwrap();

    let mut ext: sp_io::TestExternalities = t.into();
    ext.register_extension(sp_keystore::KeystoreExt::new(
        sp_keystore::testing::MemoryKeystore::new(),
    ));
    ext
}

/// Build test externalities with custom balances.
#[allow(dead_code)]
pub fn new_test_ext_with_balances(balances: Vec<(u64, u64)>) -> sp_io::TestExternalities {
    let mut t = frame_system::GenesisConfig::<Test>::default()
        .build_storage()
        .unwrap();

    pallet_balances::GenesisConfig::<Test> {
        balances,
        dev_accounts: None,
    }
    .assimilate_storage(&mut t)
    .unwrap();

    t.into()
}

/// Run to a specific block number.
#[allow(dead_code)]
pub fn run_to_block(n: u64) {
    while System::block_number() < n {
        <System as Hooks<u64>>::on_finalize(System::block_number());
        System::set_block_number(System::block_number() + 1);
        <System as Hooks<u64>>::on_initialize(System::block_number());
    }
}
