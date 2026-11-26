// SPDX-License-Identifier: Apache-2.0

//! Migrates storage version 0, written by release v0.4.1 (spec 4001), to
//! version 1, which adds storage deposits and holds the provider stake.
//!
//! - Every provider keeps its registration. Its stake moves from a
//!   `ReservableCurrency` reserve to a [`HoldReason::ProviderStake`] hold, and
//!   it pays the provider storage deposit. Version 0 recorded block numbers in
//!   parachain blocks, and spec 4001 produced one every 6 s, the length of an
//!   anchor block. So durations keep their values, `registered_at` keeps the
//!   provider's age (the upgrade's anchor block minus the parachain blocks
//!   since registration), and a pending deregistration restarts its
//!   announcement period from the upgrade's anchor block. Version 0 counted a
//!   challenge as received when it was created, so the defended count is
//!   received minus failed; version 0 had no challenger tiers, so all of it
//!   counts as public. Version 0 did not record revenue, so
//!   `lifetime_revenue` starts at 0.
//! - A provider that cannot hold its stake and the storage deposit with the
//!   existential deposit left free is removed, and its stake becomes free.
//!   `post_upgrade` fails for it, so try-runtime reports it before the upgrade
//!   is enacted.
//! - Every bucket and the whole `MemberBuckets` index are removed. Version 1
//!   buckets need a storage deposit from their creator, and the only version 0
//!   buckets ever deployed are the genesis buckets of an account without
//!   funds. `pre_upgrade` fails for a bucket with a snapshot or a primary
//!   provider, and for any agreement or challenge.
//! - Storage items that version 1 removed are cleared.
//!
//! [`HoldReason::ProviderStake`]: crate::HoldReason::ProviderStake

use crate::{
    BalanceOf, BlockNumberFor, Config, Pallet, ProviderInfo, ProviderSettings, ProviderStats,
    Providers,
};
#[cfg(feature = "try-runtime")]
use alloc::vec::Vec;
use core::marker::PhantomData;
use frame_support::{
    migrations::VersionedMigration,
    pallet_prelude::*,
    storage::{storage_prefix, unhashed},
    traits::{
        Consideration, Defensive, PalletInfoAccess, ReservableCurrency, UncheckedOnRuntimeUpgrade,
    },
};
use sp_runtime::traits::{SaturatedConversion, Saturating, Zero};

/// The version 0 layouts, decoded from what spec 4001 stored.
pub(crate) mod v0 {
    use super::*;

    #[derive(Encode, Decode)]
    pub struct ProviderInfo<T: Config> {
        pub multiaddr: BoundedVec<u8, T::MaxMultiaddrLength>,
        pub public_key: BoundedVec<u8, ConstU32<64>>,
        pub stake: BalanceOf<T>,
        pub committed_bytes: u64,
        pub settings: ProviderSettings<T>,
        pub stats: ProviderStats<T>,
        pub deregister_at: Option<BlockNumberFor<T>>,
    }

    #[derive(Encode, Decode)]
    pub struct ProviderSettings<T: Config> {
        pub min_duration: BlockNumberFor<T>,
        pub max_duration: BlockNumberFor<T>,
        pub price_per_byte: BalanceOf<T>,
        pub accepting_primary: bool,
        pub replica_sync_price: Option<BalanceOf<T>>,
        pub accepting_extensions: bool,
        pub max_capacity: u64,
    }

    #[derive(Encode, Decode)]
    pub struct ProviderStats<T: Config> {
        pub registered_at: BlockNumberFor<T>,
        pub agreements_total: u32,
        pub agreements_extended: u32,
        pub agreements_not_extended: u32,
        pub agreements_burned: u32,
        pub total_bytes_committed: u64,
        pub challenges_received: u32,
        pub challenges_failed: u32,
    }

    #[frame_support::storage_alias]
    pub type Providers<T: Config> = StorageMap<
        Pallet<T>,
        Blake2_128Concat,
        <T as frame_system::Config>::AccountId,
        ProviderInfo<T>,
    >;

    #[cfg(any(feature = "try-runtime", test))]
    pub mod buckets {
        use super::*;
        use alloc::vec::Vec;
        use sp_core::H256;
        use storage_primitives::{BucketId, Role};

        #[derive(Encode, Decode)]
        pub struct Member<T: Config> {
            pub account: T::AccountId,
            pub role: Role,
        }

        #[derive(Encode, Decode)]
        pub struct BucketSnapshot<BlockNumber> {
            pub mmr_root: H256,
            pub start_seq: u64,
            pub leaf_count: u64,
            pub checkpoint_block: BlockNumber,
            pub primary_signers: Vec<u8>,
        }

        #[derive(Encode, Decode)]
        pub struct Bucket<T: Config> {
            pub members: BoundedVec<Member<T>, T::MaxMembers>,
            pub frozen_start_seq: Option<u64>,
            pub min_providers: u32,
            pub primary_providers: BoundedVec<T::AccountId, T::MaxPrimaryProviders>,
            pub snapshot: Option<BucketSnapshot<BlockNumberFor<T>>>,
            pub historical_roots: [(u32, H256); 6],
            pub total_snapshots: u32,
        }

        #[frame_support::storage_alias]
        pub type Buckets<T: Config> = StorageMap<Pallet<T>, Blake2_128Concat, BucketId, Bucket<T>>;
    }
}

/// Reads and writes per migrated provider: the entry, the reserve, the stake
/// hold and the deposit hold.
const PROVIDER_READS_WRITES: u64 = 6;

/// Storage items the migration empties: the buckets and their index, and
/// the items version 1 removed.
const CLEARED_ITEMS: [&[u8]; 7] = [
    b"Buckets",
    b"MemberBuckets",
    b"CheckpointConfigs",
    b"CheckpointPool",
    b"CheckpointRewards",
    b"LastCheckpointWindow",
    b"ProviderReplayStates",
];

fn item_prefix<T: Config>(item: &[u8]) -> [u8; 32] {
    storage_prefix(<Pallet<T> as PalletInfoAccess>::name().as_bytes(), item)
}

/// The version 0 to 1 migration, without the storage version check. Use
/// [`MigrateV0ToV1`].
///
/// `OldCurrency` is the currency version 0 reserved the provider stake with,
/// the runtime's balances pallet.
pub struct InnerMigrateV0ToV1<T, OldCurrency>(PhantomData<(T, OldCurrency)>);

impl<T, OldCurrency> InnerMigrateV0ToV1<T, OldCurrency>
where
    T: Config,
    OldCurrency: ReservableCurrency<T::AccountId, Balance = BalanceOf<T>>,
{
    fn migrate_provider(
        who: &T::AccountId,
        old: v0::ProviderInfo<T>,
        anchor_block: BlockNumberFor<T>,
        parachain_block: u64,
    ) -> Option<ProviderInfo<T>> {
        let stake = old
            .stake
            .saturating_sub(OldCurrency::unreserve(who, old.stake));
        Pallet::<T>::hold_stake(who, stake).ok()?;
        let Ok(deposit) = T::StorageDeposit::new(who, Pallet::<T>::provider_footprint()) else {
            let _ = Pallet::<T>::release_stake(who, stake).defensive();
            return None;
        };
        let parachain_blocks_since_registration =
            parachain_block.saturating_sub(old.stats.registered_at.saturated_into());
        Some(ProviderInfo {
            multiaddr: old.multiaddr,
            public_key: old.public_key,
            stake,
            committed_bytes: old.committed_bytes,
            settings: ProviderSettings {
                min_duration: old.settings.min_duration,
                max_duration: old.settings.max_duration,
                price_per_byte: old.settings.price_per_byte,
                accepting_primary: old.settings.accepting_primary,
                replica_sync_price: old.settings.replica_sync_price,
                accepting_extensions: old.settings.accepting_extensions,
                max_capacity: old.settings.max_capacity,
                min_agreement_bytes: 0,
            },
            stats: ProviderStats {
                registered_at: anchor_block
                    .saturating_sub(parachain_blocks_since_registration.saturated_into()),
                agreements_total: old.stats.agreements_total,
                agreements_extended: old.stats.agreements_extended,
                agreements_not_extended: old.stats.agreements_not_extended,
                agreements_burned: old.stats.agreements_burned,
                total_bytes_committed: old.stats.total_bytes_committed,
                challenges_received_authorized: 0,
                challenges_received_public: old
                    .stats
                    .challenges_received
                    .saturating_sub(old.stats.challenges_failed),
                challenges_failed: old.stats.challenges_failed,
                lifetime_revenue: Zero::zero(),
            },
            deregister_at: old
                .deregister_at
                .map(|_| anchor_block.saturating_add(T::DeregisterAnnouncementPeriod::get())),
            deposit,
        })
    }
}

impl<T, OldCurrency> UncheckedOnRuntimeUpgrade for InnerMigrateV0ToV1<T, OldCurrency>
where
    T: Config,
    OldCurrency: ReservableCurrency<T::AccountId, Balance = BalanceOf<T>>,
{
    fn on_runtime_upgrade() -> Weight {
        let anchor_block = Pallet::<T>::current_anchor_block();
        let parachain_block: u64 = frame_system::Pallet::<T>::block_number().saturated_into();
        let mut reads: u64 = 2;
        let mut writes: u64 = 0;

        Providers::<T>::translate::<v0::ProviderInfo<T>, _>(|who, old| {
            reads += PROVIDER_READS_WRITES;
            writes += PROVIDER_READS_WRITES;
            Self::migrate_provider(&who, old, anchor_block, parachain_block)
        });

        for item in CLEARED_ITEMS {
            let result = unhashed::clear_prefix(&item_prefix::<T>(item), None, None);
            reads += u64::from(result.loops);
            writes += u64::from(result.unique);
        }

        T::DbWeight::get().reads_writes(reads, writes)
    }

    #[cfg(feature = "try-runtime")]
    fn pre_upgrade() -> Result<Vec<u8>, sp_runtime::TryRuntimeError> {
        ensure!(
            v0::Providers::<T>::iter_keys().count() == v0::Providers::<T>::iter_values().count(),
            "a version 0 Providers entry does not decode"
        );
        ensure!(
            v0::buckets::Buckets::<T>::iter_keys().count()
                == v0::buckets::Buckets::<T>::iter_values().count(),
            "a version 0 Buckets entry does not decode"
        );
        ensure!(
            v0::buckets::Buckets::<T>::iter_values()
                .all(|bucket| bucket.snapshot.is_none() && bucket.primary_providers.is_empty()),
            "a version 0 bucket has data; the migration would delete it"
        );
        for item in [
            &b"StorageAgreements"[..],
            b"Challenges",
            b"CheckpointPool",
            b"CheckpointRewards",
        ] {
            ensure!(
                !unhashed::contains_prefixed_key(&item_prefix::<T>(item)),
                "version 0 agreements, challenges or checkpoint funds exist; the migration does not convert them"
            );
        }
        let providers: Vec<T::AccountId> = v0::Providers::<T>::iter_keys().collect();
        Ok(providers.encode())
    }

    #[cfg(feature = "try-runtime")]
    fn post_upgrade(state: Vec<u8>) -> Result<(), sp_runtime::TryRuntimeError> {
        let providers: Vec<T::AccountId> =
            Decode::decode(&mut &state[..]).map_err(|_| "pre_upgrade state does not decode")?;
        ensure!(
            providers.iter().all(Providers::<T>::contains_key),
            "a provider could not hold its stake and the storage deposit with the existential deposit left free and was removed; fund it before the upgrade"
        );
        ensure!(
            Providers::<T>::iter_keys().count() == Providers::<T>::iter_values().count(),
            "a Providers entry does not decode"
        );
        for item in CLEARED_ITEMS {
            ensure!(
                !unhashed::contains_prefixed_key(&item_prefix::<T>(item)),
                "a bucket, a MemberBuckets entry or a removed storage item remains"
            );
        }
        Ok(())
    }
}

/// Migrates storage version 0 to 1; a no-op on any other version. See the
/// module documentation for what it does.
pub type MigrateV0ToV1<T, OldCurrency> = VersionedMigration<
    0,
    1,
    InnerMigrateV0ToV1<T, OldCurrency>,
    Pallet<T>,
    <T as frame_system::Config>::DbWeight,
>;
