// SPDX-License-Identifier: Apache-2.0

//! # Drive Registry Pallet
//!
//! A pallet for managing Layer 1 file system drives on-chain.
//!
//! ## Overview
//!
//! This pallet provides the on-chain registry for the Layer 1 file system built on top of
//! Layer 0 (Scalable Web3 Storage). It stores the mapping between Drive IDs and their
//! underlying Layer 0 buckets. All file/directory metadata is managed off-chain by the
//! provider node (via `fs_index`).
//!
//! ## Key Concepts
//!
//! - **Drive**: A user's logical file system, mapped to a Layer 0 bucket
//! - **Multi-Drive**: Each account can create and manage multiple drives
//!
//! ## Interface
//!
//! ### Extrinsics
//!
//! - `create_drive`: Create a new drive with automatic bucket + agreement setup
//! - `delete_drive`: Remove a drive and cleanup Layer 0 resources
//!
//! ### Queries
//!
//! - `Drives`: Maps DriveId → DriveInfo
//! - `UserDrives`: Maps AccountId → Vec<DriveId>
//! - `NextDriveId`: Auto-incrementing counter for drive IDs

#![cfg_attr(not(feature = "std"), no_std)]

extern crate alloc;

pub use pallet::*;

#[cfg(feature = "runtime-benchmarks")]
pub mod benchmarking;
pub mod migrations;
pub mod weights;
pub use weights::WeightInfo;

#[cfg(test)]
mod mock;

#[cfg(test)]
mod tests;

pub mod try_state;

#[frame_support::pallet]
#[allow(clippy::let_unit_value)]
#[allow(clippy::type_complexity)]
#[allow(deprecated)]
pub mod pallet {
    use super::*;
    use alloc::vec::Vec;
    use file_system_primitives::{DriveId, DriveInfo};
    use frame_support::{pallet_prelude::*, traits::Get};
    use frame_system::pallet_prelude::*;
    use pallet_storage_provider;
    use sp_runtime::BoundedVec;
    #[cfg(feature = "try-runtime")]
    use sp_runtime::TryRuntimeError;
    use storage_primitives::Role;

    /// In-code storage version. v1 drops the `payment` field from
    /// [`DriveInfo`]; see [`crate::migrations::v1`].
    const STORAGE_VERSION: StorageVersion = StorageVersion::new(1);

    #[pallet::pallet]
    #[pallet::storage_version(STORAGE_VERSION)]
    pub struct Pallet<T>(_);

    #[pallet::hooks]
    impl<T: Config> Hooks<BlockNumberFor<T>> for Pallet<T> {
        #[cfg(feature = "try-runtime")]
        fn try_state(_block: BlockNumberFor<T>) -> Result<(), TryRuntimeError> {
            Self::do_try_state()
        }
    }

    /// Configuration trait
    #[pallet::config]
    pub trait Config:
        frame_system::Config<RuntimeEvent: From<Event<Self>>> + pallet_storage_provider::Config
    {
        /// The overarching event type
        type RuntimeEvent: From<Event<Self>> + IsType<<Self as frame_system::Config>::RuntimeEvent>;

        /// Maximum number of drives per user
        #[pallet::constant]
        type MaxDrivesPerUser: Get<u32>;

        /// Maximum length of drive name
        #[pallet::constant]
        type MaxDriveNameLength: Get<u32>;

        /// Weight information for extrinsics in this pallet.
        type WeightInfo: WeightInfo;
    }

    /// Balance type for this pallet (inherited from Currency)
    pub type BalanceOf<T> =
        <<T as pallet_storage_provider::Config>::Currency as frame_support::traits::Currency<
            <T as frame_system::Config>::AccountId,
        >>::Balance;

    /// Maps bucket ID to drive ID (1-to-1 mapping)
    #[pallet::storage]
    #[pallet::getter(fn bucket_to_drive)]
    pub type BucketToDrive<T: Config> = StorageMap<_, Blake2_128Concat, u64, DriveId>;

    /// Drive information storage
    #[pallet::storage]
    #[pallet::getter(fn drives)]
    pub type Drives<T: Config> = StorageMap<
        _,
        Blake2_128Concat,
        DriveId,
        DriveInfo<T::AccountId, BlockNumberFor<T>, T::MaxDriveNameLength>,
    >;

    /// User's drives (account -> list of drive IDs)
    #[pallet::storage]
    #[pallet::getter(fn user_drives)]
    pub type UserDrives<T: Config> = StorageMap<
        _,
        Blake2_128Concat,
        T::AccountId,
        BoundedVec<DriveId, T::MaxDrivesPerUser>,
        ValueQuery,
    >;

    /// Next drive ID counter
    #[pallet::storage]
    #[pallet::getter(fn next_drive_id)]
    pub type NextDriveId<T> = StorageValue<_, DriveId, ValueQuery>;

    /// Events
    #[pallet::event]
    #[pallet::generate_deposit(pub(super) fn deposit_event)]
    pub enum Event<T: Config> {
        /// A new drive was created
        DriveCreated {
            drive_id: DriveId,
            owner: T::AccountId,
            bucket_id: u64,
        },
        /// Drive was deleted
        DriveDeleted {
            drive_id: DriveId,
            owner: T::AccountId,
            bucket_id: u64,
            refunded: BalanceOf<T>,
        },
        /// Drive was shared with a member
        DriveShared {
            drive_id: DriveId,
            member: T::AccountId,
            role: Role,
        },
        /// Member was removed from a shared drive
        DriveUnshared {
            drive_id: DriveId,
            member: T::AccountId,
        },
    }

    /// Errors
    #[pallet::error]
    pub enum Error<T> {
        /// Drive does not exist
        DriveNotFound,
        /// Not the owner of the drive
        NotDriveOwner,
        /// Maximum number of drives per user exceeded
        TooManyDrives,
        /// Drive name too long
        DriveNameTooLong,
        /// Drive ID overflow
        DriveIdOverflow,
        /// Failed to cleanup bucket in Layer 0
        BucketCleanupFailed,
        /// Not authorized to share this drive (must be owner or bucket admin)
        NotAuthorizedToShare,
        /// Failed to update bucket membership in Layer 0
        MembershipUpdateFailed,
    }

    #[pallet::call]
    impl<T: Config> Pallet<T> {
        /// Create a new drive with automatic bucket creation
        ///
        /// Atomically opens the Layer 0 bucket + primary storage agreement
        /// (via `establish_storage_agreement_internal`) and records the
        /// drive metadata on top. The caller obtains `terms` and `sig`
        /// off-chain from the provider; Layer 0 enforces signature, replay
        /// window, and capacity/stake/duration/price checks — those errors
        /// surface directly so the caller can react to them.
        ///
        ///
        /// Parameters:
        /// - `name`: Optional human-readable name for the drive
        /// - `provider`: Provider account that signed the terms.
        /// - `terms`: Provider-signed agreement terms.
        /// - `sig`: Provider signature over the SCALE-encoded terms.
        #[pallet::call_index(0)]
        #[pallet::weight(<T as Config>::WeightInfo::create_drive())]
        pub fn create_drive(
            origin: OriginFor<T>,
            name: Option<Vec<u8>>,
            provider: T::AccountId,
            terms: pallet_storage_provider::AgreementTermsOf<T>,
            sig: sp_runtime::MultiSignature,
        ) -> DispatchResult {
            let who = ensure_signed(origin)?;

            // Convert name to BoundedVec
            let bounded_name = if let Some(n) = name {
                Some(BoundedVec::try_from(n).map_err(|_| Error::<T>::DriveNameTooLong)?)
            } else {
                None
            };

            // Check user hasn't exceeded max drives
            let mut user_drives = UserDrives::<T>::get(&who);
            ensure!(
                user_drives.len() < T::MaxDrivesPerUser::get() as usize,
                Error::<T>::TooManyDrives
            );

            // Snapshot the values the DriveInfo wants before handing `terms`
            // to Layer 0 (which consumes it).
            let max_capacity = terms.max_bytes;
            let storage_period = terms.duration;

            // Open the Layer 0 bucket + primary agreement atomically.
            // Layer 0 errors (bad signature, replay, capacity, price, …)
            // surface directly via `?`.
            let bucket_id =
                pallet_storage_provider::Pallet::<T>::establish_storage_agreement_internal(
                    &who, &provider, terms, &sig,
                )?;

            // Get next drive ID
            let drive_id = NextDriveId::<T>::get();
            let next_id = drive_id.checked_add(1).ok_or(Error::<T>::DriveIdOverflow)?;

            // Calculate expiry block
            let current_block = pallet_storage_provider::Pallet::<T>::current_anchor_block();
            let expires_at = current_block + storage_period;

            // Create drive info
            let drive_info = DriveInfo {
                owner: who.clone(),
                bucket_id,
                created_at: current_block,
                name: bounded_name,
                max_capacity,
                storage_period,
                expires_at,
            };

            // Store drive
            Drives::<T>::insert(drive_id, drive_info);
            user_drives
                .try_push(drive_id)
                .map_err(|_| Error::<T>::TooManyDrives)?;
            UserDrives::<T>::insert(&who, user_drives);
            NextDriveId::<T>::put(next_id);

            // Map bucket to drive (1-to-1)
            BucketToDrive::<T>::insert(bucket_id, drive_id);

            // Emit event
            Self::deposit_event(Event::DriveCreated {
                drive_id,
                owner: who,
                bucket_id,
            });

            Ok(())
        }

        /// Delete a drive completely
        ///
        /// Ends all storage agreements with prorated refunds, pays providers for
        /// time served, removes the bucket from Layer 0, and removes the drive.
        ///
        /// Parameters:
        /// - `drive_id`: The drive to delete
        #[pallet::call_index(2)]
        #[pallet::weight(<T as Config>::WeightInfo::delete_drive())]
        pub fn delete_drive(origin: OriginFor<T>, drive_id: DriveId) -> DispatchResult {
            let who = ensure_signed(origin)?;

            // Get drive and verify ownership
            let drive = Drives::<T>::get(drive_id).ok_or(Error::<T>::DriveNotFound)?;
            ensure!(drive.owner == who, Error::<T>::NotDriveOwner);

            // Call Layer 0 to cleanup bucket and all agreements
            let total_refunded = pallet_storage_provider::Pallet::<T>::cleanup_bucket_internal(
                drive.bucket_id,
                &who,
            )
            .map_err(|_| Error::<T>::BucketCleanupFailed)?;

            // Remove bucket-to-drive mapping
            BucketToDrive::<T>::remove(drive.bucket_id);

            // Remove from user's drive list
            let mut user_drives = UserDrives::<T>::get(&who);
            user_drives.retain(|&id| id != drive_id);
            UserDrives::<T>::insert(&who, user_drives);

            // Remove drive
            Drives::<T>::remove(drive_id);

            // Emit event
            Self::deposit_event(Event::DriveDeleted {
                drive_id,
                owner: who,
                bucket_id: drive.bucket_id,
                refunded: total_refunded,
            });

            Ok(())
        }

        /// Share a drive with another account by adding them as a member of
        /// the underlying Layer 0 bucket.
        ///
        /// The caller must be the drive owner or an Admin of the underlying bucket.
        ///
        /// Parameters:
        /// - `drive_id`: The drive to share
        /// - `member`: Account to add
        /// - `role`: Role to assign (Admin, Writer, or Reader)
        #[pallet::call_index(3)]
        #[pallet::weight(<T as Config>::WeightInfo::share_drive())]
        pub fn share_drive(
            origin: OriginFor<T>,
            drive_id: DriveId,
            member: T::AccountId,
            role: Role,
        ) -> DispatchResult {
            let who = ensure_signed(origin)?;

            let drive = Drives::<T>::get(drive_id).ok_or(Error::<T>::DriveNotFound)?;

            // Drive owner always has permission; non-owners must be bucket Admin
            if drive.owner != who {
                // Delegate the admin check to set_member_internal which calls ensure_admin
                // If the caller isn't an admin, set_member_internal will return NotBucketAdmin
            }

            pallet_storage_provider::Pallet::<T>::set_member_internal(
                &who,
                drive.bucket_id,
                member.clone(),
                role,
            )
            .map_err(|_| Error::<T>::MembershipUpdateFailed)?;

            Self::deposit_event(Event::DriveShared {
                drive_id,
                member,
                role,
            });

            Ok(())
        }

        /// Remove a member's access to a shared drive.
        ///
        /// The caller must be the drive owner or an Admin of the underlying bucket.
        ///
        /// Parameters:
        /// - `drive_id`: The drive to unshare
        /// - `member`: Account to remove
        #[pallet::call_index(4)]
        #[pallet::weight(<T as Config>::WeightInfo::unshare_drive())]
        pub fn unshare_drive(
            origin: OriginFor<T>,
            drive_id: DriveId,
            member: T::AccountId,
        ) -> DispatchResult {
            let who = ensure_signed(origin)?;

            let drive = Drives::<T>::get(drive_id).ok_or(Error::<T>::DriveNotFound)?;

            pallet_storage_provider::Pallet::<T>::remove_member_internal(
                &who,
                drive.bucket_id,
                member.clone(),
            )
            .map_err(|_| Error::<T>::MembershipUpdateFailed)?;

            Self::deposit_event(Event::DriveUnshared { drive_id, member });

            Ok(())
        }
    }

    impl<T: Config> Pallet<T> {
        /// Helper: Get drive info
        #[allow(clippy::type_complexity)]
        pub fn get_drive(
            drive_id: DriveId,
        ) -> Option<DriveInfo<T::AccountId, BlockNumberFor<T>, T::MaxDriveNameLength>> {
            Drives::<T>::get(drive_id)
        }

        /// Helper: List all drives for a user
        pub fn list_user_drives(account: &T::AccountId) -> Vec<DriveId> {
            UserDrives::<T>::get(account).into_inner()
        }

        /// Helper: Check if user owns drive
        pub fn is_drive_owner(drive_id: DriveId, account: &T::AccountId) -> bool {
            if let Some(drive) = Drives::<T>::get(drive_id) {
                drive.owner == *account
            } else {
                false
            }
        }
    }
}
