// SPDX-License-Identifier: Apache-2.0

//! Storage migrations for `pallet-storage-provider`.

/// v0 -> v1: re-encode `Buckets` and `Providers` into their current layouts.
///
/// Two changes landed without a migration, on the assumption that the affected
/// keys would be purged from the deployed chain by hand before the next
/// upgrade. They were not, so `v0.4.1-paseo` state still uses the old layouts
/// and `try_decode_entire_state` rejects it:
///
/// - `Bucket` gained `visibility` (#330 appended it, #419 moved it to second
///   position). Entries written by `v0.4.1-paseo` have no such field.
/// - `ProviderStats` split `challenges_received` into
///   `challenges_received_authorized` / `challenges_received_public` (#330) and
///   gained `lifetime_revenue` (#397).
pub mod v1 {
    use crate::pallet::{
        BalanceOf, BlockNumberFor, Bucket, Buckets, Config, Member, Pallet, ProviderInfo,
        ProviderSettings, ProviderStats, Providers,
    };
    use frame_support::{pallet_prelude::*, traits::UncheckedOnRuntimeUpgrade, weights::Weight};
    use storage_primitives::{BucketSnapshot, Visibility};

    /// The pre-migration types, used only to decode the old values. Each
    /// mirrors the `v0.4.1-paseo` layout; every field type they name is
    /// unchanged, so only the shapes differ.
    mod old {
        use super::*;
        use sp_core::H256;

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
        pub struct ProviderStats<T: Config> {
            pub registered_at: BlockNumberFor<T>,
            pub agreements_total: u32,
            pub agreements_extended: u32,
            pub agreements_not_extended: u32,
            pub agreements_burned: u32,
            pub total_bytes_committed: u64,
            // The single counter that #330 split in two.
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
            pub stats: ProviderStats<T>,
            pub deregister_at: Option<BlockNumberFor<T>>,
        }
    }

    /// v0 → v1: re-encodes every stored `Bucket` and `ProviderInfo`.
    pub struct InnerMigrateV0ToV1<T>(core::marker::PhantomData<T>);

    impl<T: Config> UncheckedOnRuntimeUpgrade for InnerMigrateV0ToV1<T> {
        fn on_runtime_upgrade() -> Weight {
            let mut translated = 0u64;

            Buckets::<T>::translate::<old::Bucket<T>, _>(|_bucket_id, old| {
                translated = translated.saturating_add(1);
                Some(Bucket {
                    members: old.members,
                    // The fail-safe default #330 applies wherever the choice
                    // is omitted: a bucket created before the field existed
                    // never consented to public reads.
                    visibility: Visibility::Private,
                    frozen_start_seq: old.frozen_start_seq,
                    min_providers: old.min_providers,
                    primary_providers: old.primary_providers,
                    snapshot: old.snapshot,
                    historical_roots: old.historical_roots,
                    total_snapshots: old.total_snapshots,
                })
            });

            Providers::<T>::translate::<old::ProviderInfo<T>, _>(|_account, old| {
                translated = translated.saturating_add(1);
                Some(ProviderInfo {
                    multiaddr: old.multiaddr,
                    public_key: old.public_key,
                    stake: old.stake,
                    committed_bytes: old.committed_bytes,
                    settings: old.settings,
                    stats: ProviderStats {
                        registered_at: old.stats.registered_at,
                        agreements_total: old.stats.agreements_total,
                        agreements_extended: old.stats.agreements_extended,
                        agreements_not_extended: old.stats.agreements_not_extended,
                        agreements_burned: old.stats.agreements_burned,
                        total_bytes_committed: old.stats.total_bytes_committed,
                        // The public challenger tier did not exist in the old
                        // layout, so every challenge counted so far was an
                        // authorized one.
                        challenges_received_authorized: old.stats.challenges_received,
                        challenges_received_public: 0,
                        challenges_failed: old.stats.challenges_failed,
                        // Revenue accounting starts at this upgrade; the old
                        // layout recorded no running total to carry over.
                        lifetime_revenue: Default::default(),
                    },
                    deregister_at: old.deregister_at,
                })
            });

            // One read + one write per migrated entry.
            T::DbWeight::get().reads_writes(translated, translated)
        }

        #[cfg(feature = "try-runtime")]
        fn pre_upgrade() -> Result<alloc::vec::Vec<u8>, sp_runtime::TryRuntimeError> {
            // Count keys under the OLD layout so post_upgrade can confirm none
            // were dropped. Keys decode independently of the value layout.
            let counts = (
                Buckets::<T>::iter_keys().count() as u64,
                Providers::<T>::iter_keys().count() as u64,
            );
            Ok(counts.encode())
        }

        #[cfg(feature = "try-runtime")]
        fn post_upgrade(state: alloc::vec::Vec<u8>) -> Result<(), sp_runtime::TryRuntimeError> {
            let (buckets_before, providers_before) =
                <(u64, u64)>::decode(&mut &state[..]).map_err(|_| "invalid pre_upgrade state")?;
            // `iter()` fully decodes values under the NEW layout; if any entry
            // still failed to decode these counts would be short.
            ensure!(
                buckets_before == Buckets::<T>::iter().count() as u64,
                "Buckets entry count changed during migration"
            );
            ensure!(
                providers_before == Providers::<T>::iter().count() as u64,
                "Providers entry count changed during migration"
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
