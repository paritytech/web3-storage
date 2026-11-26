// SPDX-License-Identifier: Apache-2.0

//! Storage version 0 to 1, on the state spec 4001 left on previewnet: one
//! registered provider with reserved stake, two genesis buckets of an account
//! without funds, and an entry of a storage item version 1 removed. A runtime
//! upgrade that leaves any of it undecodable breaks every client of the chain.

use super::*;
use crate::migrations::v1::{
    v0::{self, buckets},
    MigrateV0ToV1,
};
use frame_support::{
    storage::{storage_prefix, unhashed},
    traits::{
        fungible::{Inspect, Mutate},
        GetStorageVersion, OnRuntimeUpgrade, ReservableCurrency, StorageVersion,
    },
    BoundedVec,
};
use sp_core::H256;
use storage_primitives::Visibility;

type Migration = MigrateV0ToV1<Test, Balances>;

const PROVIDER: u64 = 1;
const STAKE: u64 = 1_000;
const BUCKET_ADMIN_WITHOUT_ACCOUNT: u64 = 42;
const MULTIADDR: &[u8] = b"/dns4/provider/tcp/443";

/// Every field distinct, so a mapping that swaps two of them fails.
fn v0_provider(stake: u64, deregister_at: Option<u64>) -> v0::ProviderInfo<Test> {
    v0::ProviderInfo {
        multiaddr: MULTIADDR.to_vec().try_into().unwrap(),
        public_key: test_public_key(),
        stake,
        committed_bytes: 0,
        settings: v0::ProviderSettings {
            min_duration: 100,
            max_duration: 100_000,
            price_per_byte: 3,
            accepting_primary: true,
            replica_sync_price: Some(4),
            accepting_extensions: false,
            max_capacity: 9_999,
        },
        stats: v0::ProviderStats {
            registered_at: 7,
            agreements_total: 11,
            agreements_extended: 12,
            agreements_not_extended: 13,
            agreements_burned: 14,
            total_bytes_committed: 15,
            challenges_received: 3,
            challenges_failed: 1,
        },
        deregister_at,
    }
}

fn v0_genesis_bucket() -> buckets::Bucket<Test> {
    buckets::Bucket {
        members: BoundedVec::truncate_from(vec![buckets::Member {
            account: BUCKET_ADMIN_WITHOUT_ACCOUNT,
            role: Role::Admin,
        }]),
        frozen_start_seq: None,
        min_providers: 1,
        primary_providers: Default::default(),
        snapshot: None,
        historical_roots: [(0, H256::zero()); 6],
        total_snapshots: 0,
    }
}

fn replay_state_key() -> Vec<u8> {
    [
        &storage_prefix(b"StorageProvider", b"ProviderReplayStates")[..],
        &[7u8; 16],
    ]
    .concat()
}

/// Writes the version 0 state: `provider` registered with `recorded_stake`,
/// of which `reserved` is reserved.
fn put_v0_state(provider: u64, recorded_stake: u64, reserved: u64, deregister_at: Option<u64>) {
    StorageVersion::new(0).put::<StorageProvider>();
    assert_ok!(<Balances as ReservableCurrency<u64>>::reserve(
        &provider, reserved
    ));
    v0::Providers::<Test>::insert(provider, v0_provider(recorded_stake, deregister_at));
    buckets::Buckets::<Test>::insert(0, v0_genesis_bucket());
    buckets::Buckets::<Test>::insert(1, v0_genesis_bucket());
    MemberBuckets::<Test>::insert(
        BUCKET_ADMIN_WITHOUT_ACCOUNT,
        BoundedVec::truncate_from(vec![0, 1]),
    );
    NextBucketId::<Test>::put(2);
    unhashed::put(&replay_state_key(), &5u64);
}

/// Balances counts holds in the reserved balance, so a reserved balance equal
/// to the stake and deposit holds means no plain reserve is left. The
/// provider's age survives: `registered_at` is the block it registered at,
/// not the upgrade's.
#[test]
fn provider_keeps_its_registration_with_stake_held_and_deposit_paid() {
    new_test_ext().execute_with(|| {
        put_v0_state(PROVIDER, STAKE, STAKE, None);
        run_to_block(100);

        Migration::on_runtime_upgrade();

        assert_eq!(StorageProvider::on_chain_storage_version(), 1);
        let migrated = Providers::<Test>::get(PROVIDER).expect("provider kept");
        assert_eq!(migrated.multiaddr.to_vec(), MULTIADDR.to_vec());
        assert_eq!(migrated.public_key, test_public_key());
        assert_eq!(migrated.stake, STAKE);
        assert_eq!(migrated.committed_bytes, 0);
        assert_eq!(migrated.deregister_at, None);
        assert_eq!(
            migrated.settings,
            ProviderSettings {
                min_duration: 100,
                max_duration: 100_000,
                price_per_byte: 3,
                accepting_primary: true,
                replica_sync_price: Some(4),
                accepting_extensions: false,
                max_capacity: 9_999,
                min_agreement_bytes: 0,
            }
        );
        assert_eq!(
            migrated.stats,
            ProviderStats {
                registered_at: 7,
                agreements_total: 11,
                agreements_extended: 12,
                agreements_not_extended: 13,
                agreements_burned: 14,
                total_bytes_committed: 15,
                challenges_received_authorized: 0,
                challenges_received_public: 2,
                challenges_failed: 1,
                lifetime_revenue: 0,
            }
        );
        assert_eq!(held(HoldReason::ProviderStake, PROVIDER), STAKE);
        assert_eq!(held(HoldReason::StorageDeposit, PROVIDER), 10);
        assert_eq!(Balances::reserved_balance(PROVIDER), STAKE + 10);
        assert_ok!(StorageProvider::do_try_state());
    });
}

/// The stake moved from a reserve to a hold, so leaving the network returns
/// it. Left reserved, the pallet would release nothing and the stake would
/// stay locked.
#[test]
fn migrated_provider_can_deregister_and_get_its_stake_back() {
    new_test_ext().execute_with(|| {
        put_v0_state(PROVIDER, STAKE, STAKE, None);
        let balance_before = Balances::total_balance(&PROVIDER);
        Migration::on_runtime_upgrade();

        assert_ok!(StorageProvider::deregister_provider(RuntimeOrigin::signed(
            PROVIDER
        )));
        run_to_block(System::block_number() + 150);
        assert_ok!(StorageProvider::complete_deregister(RuntimeOrigin::signed(
            PROVIDER
        )));

        assert_eq!(held(HoldReason::ProviderStake, PROVIDER), 0);
        assert_eq!(held(HoldReason::StorageDeposit, PROVIDER), 0);
        assert_eq!(Balances::free_balance(PROVIDER), balance_before);
    });
}

/// A version 0 deadline counts parachain blocks, not anchor blocks. The
/// provider stays slashable for a full announcement period from the upgrade,
/// never less.
#[test]
fn pending_deregistration_waits_a_full_announcement_period_again() {
    new_test_ext().execute_with(|| {
        put_v0_state(PROVIDER, STAKE, STAKE, Some(5));
        let upgrade_block = System::block_number();

        Migration::on_runtime_upgrade();

        let deregister_at = upgrade_block + 150;
        assert_eq!(
            Providers::<Test>::get(PROVIDER).unwrap().deregister_at,
            Some(deregister_at)
        );
        run_to_block(deregister_at - 1);
        assert_noop!(
            StorageProvider::complete_deregister(RuntimeOrigin::signed(PROVIDER)),
            Error::<Test>::DeregisterPeriodNotElapsed
        );
        run_to_block(deregister_at);
        assert_ok!(StorageProvider::complete_deregister(RuntimeOrigin::signed(
            PROVIDER
        )));
    });
}

/// The recorded stake is only what the hold can back: a provider whose reserve
/// is smaller than its recorded stake keeps the reserved amount as its stake,
/// so `ProviderInfo::stake` matches the hold.
#[test]
fn stake_is_what_was_actually_reserved() {
    new_test_ext().execute_with(|| {
        put_v0_state(PROVIDER, STAKE, 600, None);

        Migration::on_runtime_upgrade();

        assert_eq!(Providers::<Test>::get(PROVIDER).unwrap().stake, 600);
        assert_eq!(held(HoldReason::ProviderStake, PROVIDER), 600);
        assert_ok!(StorageProvider::do_try_state());
    });
}

/// The genesis buckets cannot pay a version 1 storage deposit: their admin
/// has no account. Removing them must leave no reverse-index entry behind, and
/// keep `NextBucketId` so a new bucket never reuses a removed bucket's id.
#[test]
fn genesis_buckets_and_removed_items_are_cleared() {
    new_test_ext().execute_with(|| {
        put_v0_state(PROVIDER, STAKE, STAKE, None);

        Migration::on_runtime_upgrade();

        assert_eq!(Buckets::<Test>::iter_keys().count(), 0);
        assert!(!MemberBuckets::<Test>::contains_key(
            BUCKET_ADMIN_WITHOUT_ACCOUNT
        ));
        assert!(!unhashed::exists(&replay_state_key()));
        assert_eq!(NextBucketId::<Test>::get(), 2);
        assert_ok!(StorageProvider::create_bucket(
            RuntimeOrigin::signed(2),
            1,
            Visibility::Private
        ));
        assert!(Buckets::<Test>::contains_key(2));
    });
}

/// A provider without free funds for the deposit is removed rather than
/// failing the upgrade, and its stake is free again, not held or reserved.
#[test]
fn provider_that_cannot_pay_the_deposit_is_removed_with_its_stake_free() {
    new_test_ext().execute_with(|| {
        let poor_provider = 9;
        <Balances as Mutate<u64>>::set_balance(&poor_provider, 505);
        put_v0_state(poor_provider, 500, 500, None);

        Migration::on_runtime_upgrade();

        assert!(!Providers::<Test>::contains_key(poor_provider));
        assert_eq!(held(HoldReason::ProviderStake, poor_provider), 0);
        assert_eq!(Balances::reserved_balance(poor_provider), 0);
        assert_eq!(Balances::free_balance(poor_provider), 505);
        assert_ok!(StorageProvider::do_try_state());
    });
}
