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

#[cfg(test)]
mod mock;

#[cfg(test)]
mod tests;

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
    use storage_primitives::Role;

    #[pallet::pallet]
    pub struct Pallet<T>(_);

    /// Configuration trait
    #[pallet::config]
    pub trait Config: frame_system::Config + pallet_storage_provider::Config {
        /// The overarching event type
        type RuntimeEvent: From<Event<Self>> + IsType<<Self as frame_system::Config>::RuntimeEvent>;

        /// Maximum number of drives per user
        #[pallet::constant]
        type MaxDrivesPerUser: Get<u32>;

        /// Maximum length of drive name
        #[pallet::constant]
        type MaxDriveNameLength: Get<u32>;
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
        DriveInfo<T::AccountId, BlockNumberFor<T>, T::MaxDriveNameLength, BalanceOf<T>>,
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
        /// Invalid storage size (must be > 0)
        InvalidStorageSize,
        /// Invalid provider count (must be > 0)
        InvalidProviderCount,
        /// Invalid storage period (must be > 0)
        InvalidStoragePeriod,
        /// Invalid payment amount (must be > 0)
        InvalidPayment,
        /// Failed to create bucket in Layer 0
        BucketCreationFailed,
        /// Failed to cleanup bucket in Layer 0
        BucketCleanupFailed,
        /// No storage providers available
        NoProvidersAvailable,
        /// Insufficient replica providers available
        InsufficientReplicaProviders,
        /// Not authorized to share this drive (must be owner or bucket admin)
        NotAuthorizedToShare,
        /// Failed to update bucket membership in Layer 0
        MembershipUpdateFailed,
    }

    #[pallet::call]
    impl<T: Config> Pallet<T> {
        /// Create a new drive with automatic bucket creation
        ///
        /// The system automatically creates a Layer 0 bucket, selects providers,
        /// and establishes storage agreements. File/directory metadata is managed
        /// off-chain by the provider node.
        ///
        /// Parameters:
        /// - `name`: Optional human-readable name for the drive
        /// - `max_capacity`: Maximum storage capacity in bytes
        /// - `storage_period`: Storage duration in blocks
        /// - `payment`: Upfront payment tokens for storage agreements
        /// - `min_providers`: Optional minimum number of providers (default: auto)
        #[pallet::call_index(0)]
        #[pallet::weight(10_000)]
        pub fn create_drive(
            origin: OriginFor<T>,
            name: Option<Vec<u8>>,
            max_capacity: u64,
            storage_period: BlockNumberFor<T>,
            payment: BalanceOf<T>,
            min_providers: Option<u8>,
        ) -> DispatchResult {
            let who = ensure_signed(origin)?;

            // Validate inputs
            ensure!(max_capacity > 0, Error::<T>::InvalidStorageSize);
            ensure!(
                storage_period > BlockNumberFor::<T>::from(0u32),
                Error::<T>::InvalidStoragePeriod
            );
            use sp_runtime::traits::Zero;
            ensure!(!payment.is_zero(), Error::<T>::InvalidPayment);

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

            // Allocate bucket with storage parameters
            let bucket_id = Self::allocate_bucket_for_user(
                &who,
                max_capacity,
                storage_period,
                payment,
                min_providers,
            )?;

            // Get next drive ID
            let drive_id = NextDriveId::<T>::get();
            let next_id = drive_id.checked_add(1).ok_or(Error::<T>::DriveIdOverflow)?;

            // Calculate expiry block
            let current_block = <frame_system::Pallet<T>>::block_number();
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
                payment,
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
        #[pallet::weight(10_000)]
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
        #[pallet::weight(10_000)]
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
        #[pallet::weight(10_000)]
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
        /// Allocate a bucket for a user with specified storage requirements.
        fn allocate_bucket_for_user(
            user: &T::AccountId,
            max_capacity: u64,
            storage_period: BlockNumberFor<T>,
            payment: BalanceOf<T>,
            min_providers: Option<u8>,
        ) -> Result<u64, Error<T>> {
            use sp_runtime::traits::{CheckedDiv, SaturatedConversion, Zero};

            // Determine number of providers
            let num_providers: u8 = if let Some(min) = min_providers {
                ensure!(min > 0, Error::<T>::InvalidProviderCount);
                min
            } else {
                let threshold_blocks = BlockNumberFor::<T>::from(1000u32);
                if storage_period > threshold_blocks {
                    3
                } else {
                    1
                }
            };

            // Step 1: Create bucket in Layer 0
            let bucket_id = pallet_storage_provider::Pallet::<T>::create_bucket_internal(
                user,
                num_providers as u32,
            )
            .map_err(|_| Error::<T>::BucketCreationFailed)?;

            // Step 2: Calculate payment per provider
            let divisor: BalanceOf<T> = (num_providers as u32).saturated_into();
            let payment_per_provider = payment
                .checked_div(&divisor)
                .ok_or(Error::<T>::BucketCreationFailed)?;

            // Step 3: Find and request primary agreement
            let available_primary_providers =
                pallet_storage_provider::Pallet::<T>::query_available_providers(max_capacity, true);

            ensure!(
                !available_primary_providers.is_empty(),
                Error::<T>::NoProvidersAvailable
            );

            let primary_provider = &available_primary_providers[0];

            pallet_storage_provider::Pallet::<T>::request_primary_agreement_internal(
                user,
                bucket_id,
                primary_provider,
                max_capacity,
                storage_period,
                payment_per_provider,
            )
            .map_err(|_| Error::<T>::BucketCreationFailed)?;

            // Step 4: Request replica agreements (if needed)
            if num_providers > 1 {
                let available_replica_providers =
                    pallet_storage_provider::Pallet::<T>::query_available_providers(
                        max_capacity,
                        false,
                    );

                let num_replicas = (num_providers - 1) as usize;
                ensure!(
                    available_replica_providers.len() >= num_replicas,
                    Error::<T>::InsufficientReplicaProviders
                );

                let mut replica_count = 0;
                for replica_provider in available_replica_providers.iter() {
                    if replica_count >= num_replicas {
                        break;
                    }
                    if replica_provider == primary_provider {
                        continue;
                    }

                    let divisor_ten: BalanceOf<T> = 10u32.saturated_into();
                    let sync_balance = payment_per_provider
                        .checked_div(&divisor_ten)
                        .unwrap_or_else(Zero::zero);

                    pallet_storage_provider::Pallet::<T>::request_replica_agreement_internal(
                        user,
                        bucket_id,
                        replica_provider,
                        max_capacity,
                        storage_period,
                        payment_per_provider,
                        sync_balance,
                    )
                    .map_err(|_| Error::<T>::BucketCreationFailed)?;

                    replica_count += 1;
                }
            }

            Ok(bucket_id)
        }

        /// Helper: Get drive info
        #[allow(clippy::type_complexity)]
        pub fn get_drive(
            drive_id: DriveId,
        ) -> Option<DriveInfo<T::AccountId, BlockNumberFor<T>, T::MaxDriveNameLength, BalanceOf<T>>>
        {
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
