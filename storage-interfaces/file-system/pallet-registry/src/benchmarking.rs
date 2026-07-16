// SPDX-License-Identifier: Apache-2.0

//! Benchmarking setup for `pallet-drive-registry`.
//!
//! Each benchmark sets the pallet up in the configuration that exercises the
//! most expensive control flow inside the extrinsic so that generated weights
//! are upper bounds:
//!
//! * `create_drive` — provider with full max-capacity stake (so Layer 0 runs
//!   signature / replay / capacity / stake / duration / price), write a
//!   max-length name, and `try_push` into a near-full `UserDrives` bounded
//!   vec.
//! * `delete_drive` — bucket has one accepted primary agreement AND
//!   `MaxMembers` members, so `cleanup_bucket_internal` runs both refund and
//!   per-member retain loops; caller's `UserDrives` is at `MaxDrivesPerUser`.
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
use frame_system::RawOrigin;
use pallet_storage_provider::{AgreementTermsOf, Pallet as StorageProvider, ProviderSettings};
use sp_runtime::traits::{Bounded, SaturatedConversion, Saturating};
use storage_primitives::{AgreementTerms, Role};

const SEED: u32 = 0;

/// Key type used by the benchmarking keystore for provider signing material.
const KEY_TYPE: sp_core::crypto::KeyTypeId = sp_core::crypto::KeyTypeId(*b"drbn");

/// Create an account with effectively unbounded balance.
fn funded_account<T: Config>(name: &'static str, index: u32) -> T::AccountId {
    let account: T::AccountId = account(name, index, SEED);
    let amount = BalanceOf::<T>::max_value() / 2u32.into();
    let _ =
        <T as pallet_storage_provider::Config>::Currency::make_free_balance_be(&account, amount);
    account
}

/// Register a storage provider that accepts primary agreements with enough
/// stake and capacity to back the benchmarks. Returns both the account and
/// the sr25519 public key registered against it so the `create_drive`
/// benchmark can sign terms with the matching keystore key.
fn create_provider<T: Config>(index: u32) -> (T::AccountId, sp_core::sr25519::Public) {
    let provider = funded_account::<T>("provider", index);
    let multiaddr = b"/ip4/127.0.0.1/tcp/3000".to_vec();

    // Generate an sr25519 key in the runtime keystore so the pallet can
    // verify signatures over agreement terms.
    let seed = alloc::format!("//DriveBenchProvider{index}");
    let public_key = sp_io::crypto::sr25519_generate(KEY_TYPE, Some(seed.into_bytes()));

    // Stake must cover declared capacity at MinStakePerByte.
    let capacity: u64 = 1_000_000_000;
    let capacity_balance: BalanceOf<T> = capacity.saturated_into();
    let required_for_capacity = T::MinStakePerByte::get() * capacity_balance;
    let stake = T::MinProviderStake::get().max(required_for_capacity);

    let _ = StorageProvider::<T>::register_provider(
        RawOrigin::Signed(provider.clone()).into(),
        multiaddr.try_into().unwrap(),
        public_key.0.to_vec().try_into().unwrap(),
        stake,
    );

    let _ = StorageProvider::<T>::update_provider_settings(
        RawOrigin::Signed(provider.clone()).into(),
        ProviderSettings::<T> {
            min_duration: 1u32.into(),
            max_duration: 1_000_000u32.into(),
            price_per_byte: 1u32.into(),
            accepting_primary: true,
            replica_sync_price: Some(1u32.into()),
            accepting_extensions: true,
            max_capacity: capacity,
        },
    );

    (provider, public_key)
}

/// Sign agreement terms with the provider's keystore key.
fn sign_terms<T: Config>(
    public_key: &sp_core::sr25519::Public,
    terms: &AgreementTermsOf<T>,
) -> sp_runtime::MultiSignature {
    let hash = sp_io::hashing::blake2_256(&terms.signing_payload());
    let sig = sp_io::crypto::sr25519_sign(KEY_TYPE, public_key, &hash)
        .expect("benchmarking keystore signs with a key it generated");
    sp_runtime::MultiSignature::Sr25519(sig)
}

/// Build primary terms with the standard benchmark shape.
fn make_primary_terms<T: Config>(owner: &T::AccountId, nonce: u64) -> AgreementTermsOf<T> {
    AgreementTerms {
        owner: owner.clone(),
        max_bytes: 1_000u64,
        duration: 100u32.into(),
        price_per_byte: 1u32.into(),
        valid_until: frame_system::Pallet::<T>::block_number()
            .saturating_add(<T as pallet_storage_provider::Config>::RequestTimeout::get()),
        nonce,
        bucket_id: None,
        replica_params: None,
    }
}

/// Open a drive for `user` via the signed-terms path; returns
/// `(drive_id, bucket_id)`.
fn create_drive_for<T: Config>(
    user: &T::AccountId,
    provider: &T::AccountId,
    provider_pk: &sp_core::sr25519::Public,
    nonce: u64,
) -> (DriveId, u64) {
    let terms = make_primary_terms::<T>(user, nonce);
    let sig = sign_terms::<T>(provider_pk, &terms);
    let _ = DriveRegistry::<T>::create_drive(
        RawOrigin::Signed(user.clone()).into(),
        None,
        provider.clone(),
        terms,
        sig,
    );
    let drive_id = NextDriveId::<T>::get().saturating_sub(1);
    let drive = Drives::<T>::get(drive_id).expect("create_drive just inserted this");
    (drive_id, drive.bucket_id)
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
    /// - Provider with full max-capacity stake registered so the Layer 0
    ///   signature / replay / capacity / stake / duration / price checks all
    ///   run.
    /// - `name` is at `MaxDriveNameLength` so the bounded-vec conversion and
    ///   storage write are at maximum size.
    /// - `UserDrives` for the caller is pre-filled to `MaxDrivesPerUser - 1`
    ///   so the bounded `try_push` runs at the capacity boundary.
    #[benchmark]
    fn create_drive() {
        let (provider, provider_pk) = create_provider::<T>(0);

        let user = funded_account::<T>("user", 0);
        prefill_user_drives::<T>(&user);

        let name = vec![b'x'; T::MaxDriveNameLength::get() as usize];
        let terms = make_primary_terms::<T>(&user, 1);
        let sig = sign_terms::<T>(&provider_pk, &terms);

        #[extrinsic_call]
        create_drive(RawOrigin::Signed(user), Some(name), provider, terms, sig);
    }

    /// Worst case for `cleanup_bucket_internal`:
    /// - One accepted primary agreement (the establish-flow path).
    /// - Bucket has `MaxMembers` members, so the per-member `MemberBuckets`
    ///   retain loop runs at its maximum.
    /// - Caller's `UserDrives` is at `MaxDrivesPerUser`, so the `retain`
    ///   that removes the deleted drive scans the full list.
    #[benchmark]
    fn delete_drive() {
        let (provider, provider_pk) = create_provider::<T>(0);
        let user = funded_account::<T>("user", 0);

        let (drive_id, bucket_id) = create_drive_for::<T>(&user, &provider, &provider_pk, 1);

        // Fill members up to MaxMembers (owner is already a member).
        let max_members = <T as pallet_storage_provider::Config>::MaxMembers::get();
        for i in 0..max_members.saturating_sub(1) {
            let m = funded_account::<T>("member", i);
            let _ = StorageProvider::<T>::set_member_internal(&user, bucket_id, m, Role::Reader);
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
        let (provider, provider_pk) = create_provider::<T>(0);
        let user = funded_account::<T>("user", 0);
        let (drive_id, bucket_id) = create_drive_for::<T>(&user, &provider, &provider_pk, 1);

        // Push members up to MaxMembers - 1 (owner counts as the first member).
        let max_members = <T as pallet_storage_provider::Config>::MaxMembers::get();
        for i in 0..max_members.saturating_sub(2) {
            let m = funded_account::<T>("filler", i);
            let _ = StorageProvider::<T>::set_member_internal(&user, bucket_id, m, Role::Reader);
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
        let (provider, provider_pk) = create_provider::<T>(0);
        let user = funded_account::<T>("user", 0);
        let (drive_id, bucket_id) = create_drive_for::<T>(&user, &provider, &provider_pk, 1);

        // Fill the bucket to MaxMembers, with the removal target inserted last.
        let max_members = <T as pallet_storage_provider::Config>::MaxMembers::get();
        for i in 0..max_members.saturating_sub(2) {
            let m = funded_account::<T>("filler", i);
            let _ = StorageProvider::<T>::set_member_internal(&user, bucket_id, m, Role::Reader);
        }
        let target = funded_account::<T>("target", 0);
        let _ = StorageProvider::<T>::set_member_internal(
            &user,
            bucket_id,
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
