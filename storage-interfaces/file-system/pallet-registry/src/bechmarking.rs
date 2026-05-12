//! Benchmarking setup for `pallet-drive-registry`.
//!
//! Each benchmark sets the pallet up in the configuration that exercises the
//! most expensive control flow inside the extrinsic so that generated weights
//! are upper bounds:
//!
//! * `create_drive` — request 1 primary + `MaxPrimaryProviders - 1` replica
//!   agreements, iterate the full provider set, write a max-length name, and
//!   `try_push` into a near-full `UserDrives` bounded vec.
//! * `delete_drive` — bucket has `MaxPrimaryProviders` accepted agreements
//!   AND `MaxMembers` members, so `cleanup_bucket_internal` runs the prorated
//!   refund loop and the per-member `MemberBuckets` retain at their maxima.
//! * `share_drive` — bucket sits at `MaxMembers - 1`, forcing
//!   `set_member_internal` to scan the full list and `try_push` at the
//!   capacity boundary.
//! * `unshare_drive` — bucket holds `MaxMembers`; the target is the last
//!   element so `position()` and `remove()` both do the maximum work.

use super::{Pallet as DriveRegistry, *};
use alloc::vec;
use alloc::vec::Vec;
use file_system_primitives::DriveId;
use frame_benchmarking::v2::*;
use frame_support::{
    traits::{Currency, Get},
    BoundedVec,
};
use frame_system::{pallet_prelude::BlockNumberFor, RawOrigin};
use pallet_storage_provider::{Pallet as StorageProvider, ProviderSettings};
use sp_runtime::traits::{Bounded, SaturatedConversion};
use storage_primitives::Role;

const SEED: u32 = 0;

/// Create an account with effectively unbounded balance.
fn funded_account<T: Config>(name: &'static str, index: u32) -> T::AccountId {
    let account: T::AccountId = account(name, index, SEED);
    let amount = BalanceOf::<T>::max_value() / 2u32.into();
    let _ =
        <T as pallet_storage_provider::Config>::Currency::make_free_balance_be(&account, amount);
    account
}

/// Register a storage provider that accepts both primary and replica
/// agreements with enough stake and capacity to back every agreement that
/// these benchmarks open against it.
fn create_provider<T: Config>(index: u32) -> T::AccountId {
    let provider = funded_account::<T>("provider", index);
    let multiaddr = b"/ip4/127.0.0.1/tcp/3000".to_vec();
    let public_key = [0u8; 32].to_vec();

    // Stake must cover declared capacity at MinStakePerByte.
    let capacity: u64 = 1_000_000_000;
    let capacity_balance: BalanceOf<T> = capacity.saturated_into();
    let required_for_capacity = T::MinStakePerByte::get() * capacity_balance;
    let stake = T::MinProviderStake::get().max(required_for_capacity);

    let _ = StorageProvider::<T>::register_provider(
        RawOrigin::Signed(provider.clone()).into(),
        multiaddr.try_into().unwrap(),
        public_key.try_into().unwrap(),
        stake,
    );

    let _ = StorageProvider::<T>::update_provider_settings(
        RawOrigin::Signed(provider.clone()).into(),
        ProviderSettings {
            min_duration: 1u32.into(),
            max_duration: 1_000_000u32.into(),
            price_per_byte: 1u32.into(),
            accepting_primary: true,
            replica_sync_price: Some(1u32.into()),
            accepting_extensions: true,
            max_capacity: capacity,
        },
    );

    provider
}

/// `min_providers` is `Option<u8>` in `create_drive`, so we clamp the
/// `MaxPrimaryProviders` config (typed `u32`) to `u8::MAX` before passing it.
fn max_providers_u8<T: Config>() -> u8 {
    (T::MaxPrimaryProviders::get() as u64).min(u8::MAX as u64) as u8
}

/// Pre-fill the user's drive list to `MaxDrivesPerUser - 1` so the bounded
/// `try_push` inside `create_drive` happens right at the capacity boundary.
fn prefill_user_drives<T: Config>(user: &T::AccountId) {
    let max_drives = T::MaxDrivesPerUser::get();
    if max_drives <= 1 {
        return;
    }
    let fake_ids: Vec<DriveId> = (10_000..10_000u64 + (max_drives - 1) as u64).collect();
    let bounded: BoundedVec<DriveId, T::MaxDrivesPerUser> = fake_ids.try_into().unwrap();
    UserDrives::<T>::insert(user, bounded);
}

#[benchmarks]
mod benchmarks {
    use super::*;

    // ─────────────────────────────────────────────────────────────────────────
    // Drive lifecycle
    // ─────────────────────────────────────────────────────────────────────────

    /// Worst case:
    /// - `min_providers = MaxPrimaryProviders` → request 1 primary AND
    ///   `n - 1` replica agreements (the replica branch executes fully).
    /// - `n` providers registered → `query_available_providers` iterates the
    ///   full set for both the primary and replica searches.
    /// - `name` is at `MaxDriveNameLength` so the bounded-vec conversion and
    ///   storage write are at maximum size.
    /// - `UserDrives` for the caller is pre-filled to `MaxDrivesPerUser - 1`
    ///   so the bounded `try_push` runs at the capacity boundary.
    #[benchmark]
    fn create_drive() {
        let n = T::MaxPrimaryProviders::get();
        for i in 0..n {
            let _ = create_provider::<T>(i);
        }

        let user = funded_account::<T>("user", 0);
        prefill_user_drives::<T>(&user);

        let name = vec![b'x'; T::MaxDriveNameLength::get() as usize];
        let max_capacity: u64 = 1_000;
        let storage_period: BlockNumberFor<T> = 100u32.into();
        let payment = BalanceOf::<T>::max_value() / 10u32.into();
        let min_providers = max_providers_u8::<T>();

        #[extrinsic_call]
        create_drive(
            RawOrigin::Signed(user),
            Some(name),
            max_capacity,
            storage_period,
            payment,
            Some(min_providers),
        );
    }

    /// Worst case for `cleanup_bucket_internal`:
    /// - `MaxPrimaryProviders` storage agreements have been accepted, so the
    ///   refund/transfer/event loop runs the maximum number of times.
    /// - Bucket has `MaxMembers` members, so the per-member `MemberBuckets`
    ///   retain loop runs at its maximum.
    /// - Caller's `UserDrives` is at `MaxDrivesPerUser`, so the `retain`
    ///   that removes the deleted drive scans the full list.
    #[benchmark]
    fn delete_drive() {
        let n = T::MaxPrimaryProviders::get();
        let providers: Vec<T::AccountId> = (0..n).map(create_provider::<T>).collect();

        let user = funded_account::<T>("user", 0);
        let name = vec![b'x'; T::MaxDriveNameLength::get() as usize];
        let max_capacity: u64 = 1_000;
        let storage_period: BlockNumberFor<T> = 100u32.into();
        let payment = BalanceOf::<T>::max_value() / 10u32.into();
        let min_providers = max_providers_u8::<T>();

        // Create the drive — opens primary + (n - 1) replica agreement requests.
        let _ = DriveRegistry::<T>::create_drive(
            RawOrigin::Signed(user.clone()).into(),
            Some(name),
            max_capacity,
            storage_period,
            payment,
            Some(min_providers),
        );

        let drive_id = NextDriveId::<T>::get().saturating_sub(1);
        let drive = Drives::<T>::get(drive_id).expect("create_drive just inserted this");

        // Each provider accepts so cleanup iterates the full StorageAgreements set
        // (not the pending-request set).
        for provider in &providers {
            let _ = StorageProvider::<T>::accept_agreement(
                RawOrigin::Signed(provider.clone()).into(),
                drive.bucket_id,
            );
        }

        // Fill members up to MaxMembers (owner is already a member).
        let max_members = <T as pallet_storage_provider::Config>::MaxMembers::get();
        for i in 0..max_members.saturating_sub(1) {
            let m = funded_account::<T>("member", i);
            let _ =
                StorageProvider::<T>::set_member_internal(&user, drive.bucket_id, m, Role::Reader);
        }

        // Pre-fill UserDrives to the maximum so the `retain` after deletion
        // scans the full list. The real `drive_id` is appended last so the
        // retain pass walks every other entry first.
        let max_drives = T::MaxDrivesPerUser::get();
        if max_drives > 1 {
            let mut ids: Vec<DriveId> = (10_000..10_000u64 + (max_drives - 1) as u64).collect();
            ids.push(drive_id);
            let bounded: BoundedVec<DriveId, T::MaxDrivesPerUser> = ids.try_into().unwrap();
            UserDrives::<T>::insert(&user, bounded);
        }

        #[extrinsic_call]
        delete_drive(RawOrigin::Signed(user), drive_id);
    }

    // ─────────────────────────────────────────────────────────────────────────
    // Drive sharing
    // ─────────────────────────────────────────────────────────────────────────

    /// Worst case for `set_member_internal`:
    /// - Bucket already holds `MaxMembers - 1` members (owner + fillers), so
    ///   `iter_mut().find` scans the full list before falling through to the
    ///   `try_push` branch.
    /// - The push happens right at the capacity boundary.
    #[benchmark]
    fn share_drive() {
        let _ = create_provider::<T>(0);
        let user = funded_account::<T>("user", 0);

        let _ = DriveRegistry::<T>::create_drive(
            RawOrigin::Signed(user.clone()).into(),
            None,
            1_000u64,
            100u32.into(),
            BalanceOf::<T>::max_value() / 10u32.into(),
            Some(1),
        );
        let drive_id = NextDriveId::<T>::get().saturating_sub(1);
        let drive = Drives::<T>::get(drive_id).expect("create_drive just inserted this");

        // Push members up to MaxMembers - 1 (owner counts as the first member).
        let max_members = <T as pallet_storage_provider::Config>::MaxMembers::get();
        for i in 0..max_members.saturating_sub(2) {
            let m = funded_account::<T>("filler", i);
            let _ =
                StorageProvider::<T>::set_member_internal(&user, drive.bucket_id, m, Role::Reader);
        }

        let new_member = funded_account::<T>("new_member", 0);

        #[extrinsic_call]
        share_drive(RawOrigin::Signed(user), drive_id, new_member, Role::Writer);
    }

    /// Worst case for `remove_member_internal`:
    /// - Bucket has `MaxMembers` members, so `position()` scans the entire
    ///   list and `Vec::remove` is forced to shift the prior elements when
    ///   we remove the last non-admin entry.
    #[benchmark]
    fn unshare_drive() {
        let _ = create_provider::<T>(0);
        let user = funded_account::<T>("user", 0);

        let _ = DriveRegistry::<T>::create_drive(
            RawOrigin::Signed(user.clone()).into(),
            None,
            1_000u64,
            100u32.into(),
            BalanceOf::<T>::max_value() / 10u32.into(),
            Some(1),
        );
        let drive_id = NextDriveId::<T>::get().saturating_sub(1);
        let drive = Drives::<T>::get(drive_id).expect("create_drive just inserted this");

        // Fill the bucket to MaxMembers, with the removal target inserted last.
        let max_members = <T as pallet_storage_provider::Config>::MaxMembers::get();
        for i in 0..max_members.saturating_sub(2) {
            let m = funded_account::<T>("filler", i);
            let _ =
                StorageProvider::<T>::set_member_internal(&user, drive.bucket_id, m, Role::Reader);
        }
        let target = funded_account::<T>("target", 0);
        let _ = StorageProvider::<T>::set_member_internal(
            &user,
            drive.bucket_id,
            target.clone(),
            Role::Reader,
        );

        #[extrinsic_call]
        unshare_drive(RawOrigin::Signed(user), drive_id, target);
    }

    impl_benchmark_test_suite!(
        DriveRegistry,
        crate::mock::new_test_ext(),
        crate::mock::Test
    );
}
