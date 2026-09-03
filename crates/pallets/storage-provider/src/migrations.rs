// SPDX-License-Identifier: Apache-2.0

//! Storage migrations for `pallet-storage-provider`.

extern crate alloc;

/// v0 -> v1: the challenger-tier / bucket-visibility layout changes (#330).
///
/// Three maps change shape:
/// - `Providers`: `ProviderStats.challenges_received` is replaced by
///   `challenges_received_authorized` + `challenges_received_public`. The old
///   counter tallied challenges at *creation* (pending + defended + failed);
///   the new ones tally successful defenses per challenger tier at
///   *resolution* — semantically incompatible, so both start at 0 and the old
///   value is dropped. `challenges_failed` is preserved.
/// - `Buckets`: gains a trailing `visibility` field. Pre-existing buckets
///   migrate as `Public`, preserving the open semantics they were created
///   under (world-challengeable primaries, openly served reads); admins can
///   flip to `Private` afterwards. The `Private` fail-safe wrapper default
///   applies to *new* buckets whose creator omitted the choice, not to
///   migrating state out from under existing owners.
/// - `Challenges`: gains a trailing `authorized` bool — recomputed here via
///   [`Pallet::is_authorized`] against the (already migrated) bucket, exactly
///   as challenge creation would have snapshotted it. A challenge whose
///   bucket no longer exists migrates as `false` (public tier, the
///   provider-favorable direction).
///
/// Buckets migrate before challenges so the tier recomputation reads the new
/// layout.
pub mod v1 {
    use crate::{
        BalanceOf, BlockNumberFor, Bucket, Buckets, Challenge, Challenges, Config, Member, Pallet,
        ProviderInfo, ProviderSettings, ProviderStats, Providers,
    };
    use frame_support::{pallet_prelude::*, traits::UncheckedOnRuntimeUpgrade, weights::Weight};
    use sp_core::H256;
    use storage_primitives::{BucketSnapshot, ChunkLocation, Visibility};

    /// Pre-migration layouts, used only to decode the old values: the current
    /// layouts minus the fields introduced by #330.
    mod old {
        use super::*;

        #[derive(Decode)]
        pub struct ProviderStats<BlockNumber> {
            pub registered_at: BlockNumber,
            pub agreements_total: u32,
            pub agreements_extended: u32,
            pub agreements_not_extended: u32,
            pub agreements_burned: u32,
            pub total_bytes_committed: u64,
            #[allow(dead_code)] // decoded and deliberately dropped, see module doc
            pub challenges_received: u32,
            pub challenges_failed: u32,
        }

        #[derive(Decode)]
        pub struct ProviderInfo<T: Config> {
            pub multiaddr: BoundedVec<u8, T::MaxMultiaddrLength>,
            pub public_key: BoundedVec<u8, ConstU32<64>>,
            pub stake: BalanceOf<T>,
            pub committed_bytes: u64,
            pub settings: ProviderSettings<T>,
            pub stats: ProviderStats<BlockNumberFor<T>>,
            pub deregister_at: Option<BlockNumberFor<T>>,
        }

        #[derive(Decode)]
        pub struct Bucket<T: Config> {
            pub members: BoundedVec<Member<T>, T::MaxMembers>,
            pub frozen_start_seq: Option<u64>,
            pub min_providers: u32,
            pub primary_providers: BoundedVec<T::AccountId, T::MaxPrimaryProviders>,
            pub snapshot: Option<BucketSnapshot<BlockNumberFor<T>>>,
            pub historical_roots: [(u32, H256); 6],
            pub total_snapshots: u32,
        }

        #[derive(Decode)]
        pub struct Challenge<T: Config> {
            pub bucket_id: storage_primitives::BucketId,
            pub provider: T::AccountId,
            pub challenger: T::AccountId,
            pub mmr_root: H256,
            pub start_seq: u64,
            pub target: ChunkLocation,
            pub deposit: BalanceOf<T>,
        }
    }

    pub struct InnerMigrateV0ToV1<T>(core::marker::PhantomData<T>);

    impl<T: Config> UncheckedOnRuntimeUpgrade for InnerMigrateV0ToV1<T> {
        fn on_runtime_upgrade() -> Weight {
            let mut reads: u64 = 0;
            let mut writes: u64 = 0;

            Providers::<T>::translate::<old::ProviderInfo<T>, _>(|_who, o| {
                reads = reads.saturating_add(1);
                writes = writes.saturating_add(1);
                Some(ProviderInfo {
                    multiaddr: o.multiaddr,
                    public_key: o.public_key,
                    stake: o.stake,
                    committed_bytes: o.committed_bytes,
                    settings: o.settings,
                    stats: ProviderStats {
                        registered_at: o.stats.registered_at,
                        agreements_total: o.stats.agreements_total,
                        agreements_extended: o.stats.agreements_extended,
                        agreements_not_extended: o.stats.agreements_not_extended,
                        agreements_burned: o.stats.agreements_burned,
                        total_bytes_committed: o.stats.total_bytes_committed,
                        challenges_received_authorized: 0,
                        challenges_received_public: 0,
                        challenges_failed: o.stats.challenges_failed,
                    },
                    deregister_at: o.deregister_at,
                })
            });

            Buckets::<T>::translate::<old::Bucket<T>, _>(|_bucket_id, o| {
                reads = reads.saturating_add(1);
                writes = writes.saturating_add(1);
                Some(Bucket {
                    members: o.members,
                    frozen_start_seq: o.frozen_start_seq,
                    min_providers: o.min_providers,
                    primary_providers: o.primary_providers,
                    snapshot: o.snapshot,
                    historical_roots: o.historical_roots,
                    total_snapshots: o.total_snapshots,
                    visibility: Visibility::Public,
                })
            });

            Challenges::<T>::translate::<old::Challenge<T>, _>(|_deadline, _index, o| {
                // Bucket read + the agreement-prefix scan inside is_authorized;
                // attribute two reads per challenge as a generous bound (open
                // challenges are capped per deadline and short-lived).
                reads = reads.saturating_add(3);
                writes = writes.saturating_add(1);
                let authorized = Buckets::<T>::get(o.bucket_id)
                    .map(|bucket| Pallet::<T>::is_authorized(&o.challenger, o.bucket_id, &bucket))
                    .unwrap_or(false);
                Some(Challenge {
                    bucket_id: o.bucket_id,
                    provider: o.provider,
                    challenger: o.challenger,
                    mmr_root: o.mmr_root,
                    start_seq: o.start_seq,
                    target: o.target,
                    deposit: o.deposit,
                    authorized,
                })
            });

            T::DbWeight::get().reads_writes(reads, writes)
        }

        #[cfg(feature = "try-runtime")]
        fn pre_upgrade() -> Result<alloc::vec::Vec<u8>, sp_runtime::TryRuntimeError> {
            let counts = (
                Providers::<T>::iter_keys().count() as u64,
                Buckets::<T>::iter_keys().count() as u64,
                Challenges::<T>::iter_keys().count() as u64,
            );
            Ok(counts.encode())
        }

        #[cfg(feature = "try-runtime")]
        fn post_upgrade(state: alloc::vec::Vec<u8>) -> Result<(), sp_runtime::TryRuntimeError> {
            let (providers, buckets, challenges) = <(u64, u64, u64)>::decode(&mut &state[..])
                .map_err(|_| "invalid pre_upgrade state")?;
            // `iter_keys()` only enumerates keys; `iter()` fully decodes every
            // value under the NEW layout and silently skips undecodable ones,
            // so key/value count divergence is the signal that an entry
            // failed to migrate.
            ensure!(
                providers == Providers::<T>::iter_keys().count() as u64
                    && providers == Providers::<T>::iter().count() as u64,
                "Providers entry failed to migrate"
            );
            ensure!(
                buckets == Buckets::<T>::iter_keys().count() as u64
                    && buckets == Buckets::<T>::iter().count() as u64,
                "Buckets entry failed to migrate"
            );
            ensure!(
                challenges == Challenges::<T>::iter_keys().count() as u64
                    && challenges == Challenges::<T>::iter().count() as u64,
                "Challenges entry failed to migrate"
            );
            Ok(())
        }
    }

    /// Runs [`InnerMigrateV0ToV1`] only when the on-chain storage version is 0,
    /// then bumps it to 1.
    pub type MigrateV0ToV1<T> = frame_support::migrations::VersionedMigration<
        0,
        1,
        InnerMigrateV0ToV1<T>,
        Pallet<T>,
        <T as frame_system::Config>::DbWeight,
    >;
}
