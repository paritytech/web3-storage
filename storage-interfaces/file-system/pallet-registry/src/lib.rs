//! # Drive Registry Pallet
//!
//! A pallet for managing Layer 1 file system drives on-chain.
//!
//! ## Overview
//!
//! This pallet provides the on-chain registry for the Layer 1 file system built on top of
//! Layer 0 (Scalable Web3 Storage). It stores the mapping between Drive IDs and their current
//! root CIDs, which point to the root DirectoryNode in Layer 0 storage.
//!
//! ## Key Concepts
//!
//! - **Drive**: A user's logical file system, mapped to a Layer 0 bucket
//! - **RootCID**: The content ID of the root directory, updated each time the drive changes
//! - **Multi-Drive**: Each account can create and manage multiple drives
//!
//! ## Interface
//!
//! ### Extrinsics
//!
//! - `create_drive`: Create a new drive associated with a bucket
//! - `update_root_cid`: Update the root CID of a drive after changes
//! - `delete_drive`: Remove a drive (requires bucket to be empty/burned)
//! - `update_drive_name`: Update the human-readable name of a drive
//!
//! ### Queries
//!
//! - `Drives`: Maps DriveId → DriveInfo
//! - `UserDrives`: Maps AccountId → Vec<DriveId>
//! - `NextDriveId`: Auto-incrementing counter for drive IDs

#![cfg_attr(not(feature = "std"), no_std)]

pub use pallet::*;

#[cfg(test)]
mod mock;

#[cfg(test)]
mod tests;

#[frame_support::pallet]
pub mod pallet {
    use super::*;
    use file_system_primitives::{Cid, DriveId, DriveInfo};
    use frame_support::{pallet_prelude::*, traits::Get};
    use frame_system::pallet_prelude::*;
    use sp_runtime::BoundedVec;
    use sp_std::vec::Vec;

    #[pallet::pallet]
    pub struct Pallet<T>(_);

    /// Configuration trait
    #[pallet::config]
    pub trait Config: frame_system::Config {
        /// The overarching event type
        type RuntimeEvent: From<Event<Self>> + IsType<<Self as frame_system::Config>::RuntimeEvent>;

        /// Maximum number of drives per user
        #[pallet::constant]
        type MaxDrivesPerUser: Get<u32>;

        /// Maximum length of drive name
        #[pallet::constant]
        type MaxDriveNameLength: Get<u32>;
    }

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
        /// [drive_id, owner, bucket_id, root_cid]
        DriveCreated {
            drive_id: DriveId,
            owner: T::AccountId,
            bucket_id: u64,
            root_cid: Cid,
        },
        /// Drive root CID was updated
        /// [drive_id, old_root_cid, new_root_cid]
        RootCIDUpdated {
            drive_id: DriveId,
            old_root_cid: Cid,
            new_root_cid: Cid,
        },
        /// Drive was deleted
        /// [drive_id, owner]
        DriveDeleted {
            drive_id: DriveId,
            owner: T::AccountId,
        },
        /// Drive name was updated
        /// [drive_id, name]
        DriveNameUpdated {
            drive_id: DriveId,
            name: Option<Vec<u8>>,
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
    }

    #[pallet::call]
    impl<T: Config> Pallet<T> {
        /// Create a new drive
        ///
        /// Parameters:
        /// - `bucket_id`: The Layer 0 bucket ID where drive data will be stored
        /// - `root_cid`: Initial root CID (typically zero/empty for new drive)
        /// - `name`: Optional human-readable name for the drive
        #[pallet::call_index(0)]
        #[pallet::weight(10_000)]
        pub fn create_drive(
            origin: OriginFor<T>,
            bucket_id: u64,
            root_cid: Cid,
            name: Option<Vec<u8>>,
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

            // Get next drive ID
            let drive_id = NextDriveId::<T>::get();
            let next_id = drive_id.checked_add(1).ok_or(Error::<T>::DriveIdOverflow)?;

            // Create drive info
            let drive_info = DriveInfo {
                owner: who.clone(),
                bucket_id,
                root_cid,
                created_at: <frame_system::Pallet<T>>::block_number(),
                name: bounded_name,
            };

            // Store drive
            Drives::<T>::insert(drive_id, drive_info);
            user_drives
                .try_push(drive_id)
                .map_err(|_| Error::<T>::TooManyDrives)?;
            UserDrives::<T>::insert(&who, user_drives);
            NextDriveId::<T>::put(next_id);

            // Emit event
            Self::deposit_event(Event::DriveCreated {
                drive_id,
                owner: who,
                bucket_id,
                root_cid,
            });

            Ok(())
        }

        /// Update the root CID of a drive
        ///
        /// This should be called after making changes to the drive's directory structure
        /// and uploading the new root DirectoryNode to Layer 0.
        ///
        /// Parameters:
        /// - `drive_id`: The drive to update
        /// - `new_root_cid`: The new root CID
        #[pallet::call_index(1)]
        #[pallet::weight(10_000)]
        pub fn update_root_cid(
            origin: OriginFor<T>,
            drive_id: DriveId,
            new_root_cid: Cid,
        ) -> DispatchResult {
            let who = ensure_signed(origin)?;

            // Get drive and verify ownership
            let mut drive = Drives::<T>::get(drive_id).ok_or(Error::<T>::DriveNotFound)?;
            ensure!(drive.owner == who, Error::<T>::NotDriveOwner);

            let old_root_cid = drive.root_cid;
            drive.root_cid = new_root_cid;

            // Update storage
            Drives::<T>::insert(drive_id, drive);

            // Emit event
            Self::deposit_event(Event::RootCIDUpdated {
                drive_id,
                old_root_cid,
                new_root_cid,
            });

            Ok(())
        }

        /// Delete a drive
        ///
        /// Removes the drive from the registry. Note: This does not delete data from Layer 0.
        ///
        /// Parameters:
        /// - `drive_id`: The drive to delete
        #[pallet::call_index(2)]
        #[pallet::weight(10_000)]
        pub fn delete_drive(origin: OriginFor<T>, drive_id: DriveId) -> DispatchResult {
            let who = ensure_signed(origin)?;

            // Get drive and verify ownership
            let drive = Drives::<T>::get(drive_id).ok_or(Error::<T>::DriveNotFound)?;
            ensure!(drive.owner == who, Error::<T>::NotDriveOwner);

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
            });

            Ok(())
        }

        /// Update drive name
        ///
        /// Parameters:
        /// - `drive_id`: The drive to update
        /// - `name`: New name (or None to clear)
        #[pallet::call_index(3)]
        #[pallet::weight(10_000)]
        pub fn update_drive_name(
            origin: OriginFor<T>,
            drive_id: DriveId,
            name: Option<Vec<u8>>,
        ) -> DispatchResult {
            let who = ensure_signed(origin)?;

            // Convert name to BoundedVec
            let bounded_name = if let Some(n) = name.clone() {
                Some(BoundedVec::try_from(n).map_err(|_| Error::<T>::DriveNameTooLong)?)
            } else {
                None
            };

            // Get drive and verify ownership
            let mut drive = Drives::<T>::get(drive_id).ok_or(Error::<T>::DriveNotFound)?;
            ensure!(drive.owner == who, Error::<T>::NotDriveOwner);

            // Update name
            drive.name = bounded_name;
            Drives::<T>::insert(drive_id, drive);

            // Emit event
            Self::deposit_event(Event::DriveNameUpdated { drive_id, name });

            Ok(())
        }
    }

    impl<T: Config> Pallet<T> {
        /// Helper: Get drive info
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
