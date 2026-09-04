// SPDX-License-Identifier: Apache-2.0

//! Mock runtime whose Config constants match the `challengesCode` Quint
//! instance exactly (specs/quint/challenges.qnt), so spec quantities map
//! one-to-one onto chain quantities with no scaling.

use frame_support::{
    derive_impl,
    traits::{ConstU16, ConstU32, ConstU64},
};
use sp_core::H256;
use sp_runtime::traits::{BlakeTwo256, IdentityLookup};

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

pub struct TestTreasury;
impl frame_support::traits::Get<u64> for TestTreasury {
    fn get() -> u64 {
        999 // the spec's `Tre` account
    }
}

impl pallet_storage_provider::Config for Test {
    type Currency = Balances;
    type Treasury = TestTreasury;
    type MinStakePerByte = ConstU64<1>;
    type MaxMultiaddrLength = ConstU32<128>;
    type MaxMembers = ConstU32<100>;
    type MaxPrimaryProviders = ConstU32<5>;
    // Zero so the bucket admin can register as a stake-less provider purely to
    // give `verify_signature` a public key to resolve (see lib.rs on the
    // Deleted-defense finding).
    type MinProviderStake = ConstU64<0>;
    type MaxChunkSize = ConstU32<262144>;
    // Spec instance constants (challengesCode).
    type ChallengeTimeout = ConstU64<3>;
    type ChallengeDeposit = ConstU64<10>;
    type MaxChallengesPerDeadline = ConstU16<2>;
    type DeregisterAnnouncementPeriod = ConstU64<4>;
    // Collapsed to zero in the model: `claim_expired_agreement` becomes
    // callable the block after expiry, matching `announceDeregP`'s
    // `now > AGREEMENT_END` guard.
    type SettlementTimeout = ConstU64<0>;
    // integrity_test needs RequestTimeout < DeregisterAnnouncementPeriod.
    type RequestTimeout = ConstU64<1>;
    type MaxBucketsPerMember = ConstU32<100>;
    type BlockNumberProvider = System;
    type AnchorBlockTimeMillis = ConstU64<6000>;
    type WeightInfo = ();
}
