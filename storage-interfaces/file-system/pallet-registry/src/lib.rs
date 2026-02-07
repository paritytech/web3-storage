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
    use file_system_primitives::{AgreementId, Cid, CommitStrategy, DriveId, DriveInfo};
    use frame_support::{pallet_prelude::*, traits::Get};
    use frame_system::pallet_prelude::*;
    use pallet_storage_provider;
    use sp_runtime::{
        traits::{AtLeast32BitUnsigned, MaybeSerializeDeserialize, Member},
        BoundedVec,
    };
    use sp_std::vec::Vec;

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
    pub type BalanceOf<T> = <<T as pallet_storage_provider::Config>::Currency as frame_support::traits::Currency<<T as frame_system::Config>::AccountId>>::Balance;

    /// Maps bucket ID to drive ID (1-to-1 mapping)
    /// Ensures each bucket is used by at most one drive
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
        /// A new drive was created with storage agreements
        /// [drive_id, owner, bucket_id, agreement_ids, root_cid]
        DriveCreatedWithStorage {
            drive_id: DriveId,
            owner: T::AccountId,
            bucket_id: u64,
            agreement_ids: Vec<AgreementId>,
            root_cid: Cid,
        },
        /// Pending changes were committed to on-chain root CID
        /// [drive_id, old_root_cid, new_root_cid]
        ChangesCommitted {
            drive_id: DriveId,
            old_root_cid: Cid,
            new_root_cid: Cid,
        },
        /// A dispute was raised for a failed challenge
        /// [drive_id, agreement_id, challenge_id]
        DisputeRaised {
            drive_id: DriveId,
            agreement_id: AgreementId,
            challenge_id: u64,
        },
        /// A failed provider was replaced with a new one
        /// [drive_id, old_agreement_id, new_agreement_id, new_provider]
        ProviderReplaced {
            drive_id: DriveId,
            old_agreement_id: AgreementId,
            new_agreement_id: AgreementId,
            new_provider: T::AccountId,
        },
        /// User created drive on an assigned bucket
        /// [drive_id, owner, bucket_id, root_cid]
        DriveCreatedOnBucket {
            drive_id: DriveId,
            owner: T::AccountId,
            bucket_id: u64,
            root_cid: Cid,
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
        /// Too many storage agreements for drive
        TooManyAgreements,
        /// No pending changes to commit
        NoPendingChanges,
        /// Agreement not found for this drive
        AgreementNotFound,
        /// Layer 0 storage operation failed
        StorageProviderError,
        /// Bucket not found in Layer 0
        BucketNotFound,
        /// User does not have sufficient permissions on bucket
        InsufficientBucketPermissions,
        /// Bucket is already used by another drive
        BucketAlreadyUsed,
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
        /// No storage providers available
        NoProvidersAvailable,
        /// Insufficient replica providers available
        InsufficientReplicaProviders,
    }

    #[pallet::call]
    impl<T: Config> Pallet<T> {
        /// Create a new drive with automatic bucket creation (USER-FACING API)
        ///
        /// This is the recommended way for users to create drives. The system automatically:
        /// - Creates a bucket in Layer 0
        /// - Requests storage agreements with providers
        /// - Sets up the drive infrastructure
        ///
        /// Users don't need to understand buckets or agreements - they just get a drive!
        ///
        /// Parameters:
        /// - `name`: Optional human-readable name for the drive
        /// - `max_capacity`: Maximum storage capacity in bytes (e.g., 10 GB = 10_000_000_000)
        /// - `storage_period`: Storage duration in blocks (e.g., 500 blocks)
        /// - `payment`: Upfront payment tokens for storage agreements
        /// - `min_providers`: Optional minimum number of providers (default: 3 for long-term, 1 for short-term)
        ///   - Determines replication: 1 = primary only, 3 = 1 primary + 2 replicas, etc.
        ///   - System automatically selects this many providers for storage
        /// - `commit_strategy`: Optional strategy for committing changes to on-chain root CID
        ///   - `None`: Uses default (Batched every 100 blocks)
        ///   - `Some(Immediate)`: Commit every change immediately (expensive but real-time)
        ///   - `Some(Batched { interval })`: Commit changes in batches after N blocks
        ///   - `Some(Manual)`: User manually triggers commits via `commit_changes` extrinsic
        ///
        /// Other bucket configurations use sensible defaults.
        /// Advanced users can customize these via Layer 0 APIs directly.
        ///
        /// Returns: drive_id via DriveCreated event
        #[pallet::call_index(0)]
        #[pallet::weight(10_000)]
        pub fn create_drive(
            origin: OriginFor<T>,
            name: Option<Vec<u8>>,
            max_capacity: u64,
            storage_period: BlockNumberFor<T>,
            payment: BalanceOf<T>,
            min_providers: Option<u8>,
            commit_immediately: bool,
            commit_interval: Option<u32>,
        ) -> DispatchResult {
            let who = ensure_signed(origin)?;

            // Validate inputs
            ensure!(max_capacity > 0, Error::<T>::InvalidStorageSize);
            ensure!(storage_period > BlockNumberFor::<T>::from(0u32), Error::<T>::InvalidStoragePeriod);
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
            // This internally calls Layer 0 to create bucket and request agreements
            // Default bucket configuration (can be customized in allocate_bucket_for_user):
            // - Replication: Based on min_providers or storage_period
            // - Provider selection: Automatic based on availability and capacity
            let bucket_id = Self::allocate_bucket_for_user(
                &who,
                max_capacity,
                storage_period,
                payment,
                min_providers,
            )?;

            // Create empty root directory CID (empty drive)
            let root_cid = Cid::zero();

            // Get next drive ID
            let drive_id = NextDriveId::<T>::get();
            let next_id = drive_id.checked_add(1).ok_or(Error::<T>::DriveIdOverflow)?;

            // Calculate expiry block
            let current_block = <frame_system::Pallet<T>>::block_number();
            let expires_at = current_block + storage_period;

            // Construct commit strategy from parameters
            let strategy = if commit_immediately {
                CommitStrategy::Immediate
            } else if let Some(interval) = commit_interval {
                CommitStrategy::Batched { interval }
            } else {
                CommitStrategy::Manual
            };

            // Create drive info
            let drive_info = DriveInfo {
                owner: who.clone(),
                bucket_id,
                root_cid,
                pending_root_cid: None,
                commit_strategy: strategy,
                created_at: current_block,
                last_committed_at: current_block,
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
                root_cid,
            });

            Ok(())
        }

        /// Create a new drive with an existing bucket (INTERNAL/LEGACY API)
        ///
        /// **DEPRECATED**: This is a low-level API for when you already have a bucket.
        /// Most users should use `create_drive()` instead.
        ///
        /// Parameters:
        /// - `bucket_id`: Existing Layer 0 bucket ID
        /// - `root_cid`: Initial root CID (typically zero/empty for new drive)
        /// - `name`: Optional human-readable name for the drive
        #[deprecated(note = "Use create_drive() instead - it handles bucket creation automatically")]
        #[pallet::call_index(9)]
        #[pallet::weight(10_000)]
        pub fn create_drive_with_bucket(
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
            let current_block = <frame_system::Pallet<T>>::block_number();
            let drive_info = DriveInfo {
                owner: who.clone(),
                bucket_id,
                root_cid,
                pending_root_cid: None,
                commit_strategy: CommitStrategy::default(),
                created_at: current_block,
                last_committed_at: current_block,
                name: bounded_name,
                // Legacy API: use default values for new fields
                max_capacity: 0, // Unknown/untracked
                storage_period: BlockNumberFor::<T>::from(0u32), // Indefinite
                expires_at: current_block, // No expiry
                payment: Zero::zero(), // Not tracked
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

        /// Create a drive on an assigned bucket (SIMPLIFIED USER FLOW)
        ///
        /// This is the recommended way for users to create drives. The admin has already:
        /// - Created a bucket in Layer 0
        /// - Established storage agreements with providers
        /// - Assigned the user as Reader+Writer to the bucket
        ///
        /// The user simply creates a drive on their assigned bucket without managing
        /// any infrastructure details.
        ///
        /// Parameters:
        /// - `bucket_id`: The bucket ID assigned by admin
        /// - `root_cid`: Initial root CID (typically zero/empty for new drive)
        /// - `name`: Optional drive name
        #[pallet::call_index(8)]
        #[pallet::weight(10_000)]
        pub fn create_drive_on_bucket(
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

            // 1. Check bucket exists in Layer 0
            // TODO: Implement bucket existence check once Layer 0 pallet interface is finalized
            // ensure!(
            //     pallet_storage_provider::Pallet::<T>::buckets(bucket_id).is_some(),
            //     Error::<T>::BucketNotFound
            // );

            // 2. Check user has Reader+Writer permissions on bucket
            // TODO: Implement bucket permission check once Layer 0 exposes membership queries
            // According to SIMPLIFIED_FLOWS.md, the user must have:
            // - Role::Reader | Role::Writer on the bucket
            // This validation will be added when Layer 0 pallet provides the necessary queries.
            // ensure!(
            //     bucket.has_role(&who, Role::Reader | Role::Writer),
            //     Error::<T>::InsufficientBucketPermissions
            // );

            // 3. Check bucket is not already used by another drive (1-to-1 mapping)
            ensure!(
                !BucketToDrive::<T>::contains_key(bucket_id),
                Error::<T>::BucketAlreadyUsed
            );

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
            let current_block = <frame_system::Pallet<T>>::block_number();
            let drive_info = DriveInfo {
                owner: who.clone(),
                bucket_id,
                root_cid,
                pending_root_cid: None,
                commit_strategy: CommitStrategy::default(),
                created_at: current_block,
                last_committed_at: current_block,
                name: bounded_name,
                // Bucket-based API: use default values for new fields
                max_capacity: 0, // Unknown/untracked
                storage_period: BlockNumberFor::<T>::from(0u32), // Indefinite
                expires_at: current_block, // No expiry
                payment: Zero::zero(), // Not tracked
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
            Self::deposit_event(Event::DriveCreatedOnBucket {
                drive_id,
                owner: who,
                bucket_id,
                root_cid,
            });

            Ok(())
        }

        /// Create a drive with storage agreements (DEPRECATED)
        ///
        /// **DEPRECATED**: Use `create_drive_on_bucket` instead.
        ///
        /// This function is kept for backwards compatibility but requires users to
        /// manage agreements manually. The new simplified flow has admins manage
        /// buckets and agreements, while users just create drives on assigned buckets.
        ///
        /// Parameters:
        /// - `bucket_id`: Existing bucket ID from Layer 0
        /// - `agreement_ids`: Existing agreement IDs for this drive's storage
        /// - `batched_commits`: If true, uses batched strategy; if false, uses manual
        /// - `batch_interval`: If using batched, commit every N blocks
        /// - `root_cid`: Initial root CID
        /// - `name`: Optional drive name
        #[deprecated(note = "Use create_drive_on_bucket instead")]
        #[pallet::call_index(4)]
        #[pallet::weight(10_000)]
        pub fn create_drive_with_storage(
            origin: OriginFor<T>,
            bucket_id: u64,
            agreement_ids: Vec<AgreementId>,
            batched_commits: bool,
            batch_interval: u32,
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

            // Create drive
            let drive_id = NextDriveId::<T>::get();
            let next_id = drive_id.checked_add(1).ok_or(Error::<T>::DriveIdOverflow)?;

            // Construct commit strategy from parameters
            let commit_strategy = if batched_commits {
                CommitStrategy::Batched { interval: batch_interval }
            } else {
                CommitStrategy::Manual
            };

            let current_block = <frame_system::Pallet<T>>::block_number();
            let drive_info = DriveInfo {
                owner: who.clone(),
                bucket_id,
                root_cid,
                pending_root_cid: None,
                commit_strategy,
                created_at: current_block,
                last_committed_at: current_block,
                name: bounded_name,
                // Deprecated API: use default values for new fields
                max_capacity: 0, // Unknown/untracked
                storage_period: BlockNumberFor::<T>::from(0u32), // Indefinite
                expires_at: current_block, // No expiry
                payment: Zero::zero(), // Not tracked
            };

            // Store drive
            Drives::<T>::insert(drive_id, drive_info);
            user_drives
                .try_push(drive_id)
                .map_err(|_| Error::<T>::TooManyDrives)?;
            UserDrives::<T>::insert(&who, user_drives);
            NextDriveId::<T>::put(next_id);

            // Emit event (keeping old event for backwards compatibility)
            Self::deposit_event(Event::DriveCreatedWithStorage {
                drive_id,
                owner: who,
                bucket_id,
                agreement_ids,
                root_cid,
            });

            Ok(())
        }

        /// Commit pending changes to the on-chain root CID
        ///
        /// This is used with Manual or Batched commit strategies to explicitly
        /// update the on-chain root CID with pending changes.
        ///
        /// Parameters:
        /// - `drive_id`: The drive to commit
        #[pallet::call_index(5)]
        #[pallet::weight(10_000)]
        pub fn commit_changes(origin: OriginFor<T>, drive_id: DriveId) -> DispatchResult {
            let who = ensure_signed(origin)?;

            // Get drive and verify ownership
            let mut drive = Drives::<T>::get(drive_id).ok_or(Error::<T>::DriveNotFound)?;
            ensure!(drive.owner == who, Error::<T>::NotDriveOwner);

            // Check there are pending changes
            let new_root_cid = drive.pending_root_cid.ok_or(Error::<T>::NoPendingChanges)?;

            let old_root_cid = drive.root_cid;
            drive.root_cid = new_root_cid;
            drive.pending_root_cid = None;
            drive.last_committed_at = <frame_system::Pallet<T>>::block_number();

            // Update storage
            Drives::<T>::insert(drive_id, drive);

            // Emit event
            Self::deposit_event(Event::ChangesCommitted {
                drive_id,
                old_root_cid,
                new_root_cid,
            });

            Ok(())
        }

        /// Raise a dispute for a failed storage challenge (DEPRECATED)
        ///
        /// **DEPRECATED**: In the simplified bucket-based model, admins manage infrastructure
        /// and handle disputes at the Layer 0 level. Users no longer manage agreements directly.
        ///
        /// This function is kept for backwards compatibility but is no longer functional
        /// with the new drive model.
        ///
        /// Parameters:
        /// - `drive_id`: The drive affected
        /// - `agreement_id`: The agreement with the failing provider
        /// - `challenge_id`: The challenge that failed
        #[deprecated(note = "Admin handles disputes at Layer 0. Users do not manage agreements.")]
        #[pallet::call_index(6)]
        #[pallet::weight(10_000)]
        pub fn raise_drive_dispute(
            origin: OriginFor<T>,
            drive_id: DriveId,
            agreement_id: AgreementId,
            challenge_id: u64,
        ) -> DispatchResult {
            let who = ensure_signed(origin)?;

            // Get drive and verify ownership
            let drive = Drives::<T>::get(drive_id).ok_or(Error::<T>::DriveNotFound)?;
            ensure!(drive.owner == who, Error::<T>::NotDriveOwner);

            // NOTE: In the new bucket-based model, users don't manage agreements.
            // The admin handles all infrastructure issues at Layer 0.
            // This function is deprecated and should not be used.

            // Emit event for backwards compatibility
            Self::deposit_event(Event::DisputeRaised {
                drive_id,
                agreement_id,
                challenge_id,
            });

            Ok(())
        }

        /// Replace a failed provider with a new one (DEPRECATED)
        ///
        /// **DEPRECATED**: In the simplified bucket-based model, admins manage infrastructure
        /// and handle provider replacements at the Layer 0 level. Users no longer manage
        /// agreements directly.
        ///
        /// This function is kept for backwards compatibility but is no longer functional
        /// with the new drive model.
        ///
        /// Parameters:
        /// - `drive_id`: The drive to update
        /// - `failed_agreement_id`: The agreement to replace
        /// - `new_agreement_id`: The new agreement ID
        #[deprecated(note = "Admin handles provider replacement at Layer 0. Users do not manage agreements.")]
        #[pallet::call_index(7)]
        #[pallet::weight(10_000)]
        pub fn replace_provider(
            origin: OriginFor<T>,
            drive_id: DriveId,
            failed_agreement_id: AgreementId,
            new_agreement_id: AgreementId,
        ) -> DispatchResult {
            let who = ensure_signed(origin)?;

            // Get drive and verify ownership
            let drive = Drives::<T>::get(drive_id).ok_or(Error::<T>::DriveNotFound)?;
            ensure!(drive.owner == who, Error::<T>::NotDriveOwner);

            // NOTE: In the new bucket-based model, users don't manage agreements.
            // The admin handles all infrastructure issues at Layer 0.
            // This function is deprecated and should not be used.

            // Emit event for backwards compatibility
            Self::deposit_event(Event::ProviderReplaced {
                drive_id,
                old_agreement_id: failed_agreement_id,
                new_agreement_id,
                new_provider: who,
            });

            Ok(())
        }
    }

    impl<T: Config> Pallet<T> {
        /// Allocate a bucket for a user with specified storage requirements
        ///
        /// This function encapsulates the Layer 0 bucket creation logic.
        /// It automatically:
        /// 1. Creates a bucket in Layer 0
        /// 2. Selects suitable providers based on availability and capacity
        /// 3. Requests storage agreements with providers
        /// 4. Returns the bucket_id
        ///
        /// **Default Bucket Configuration:**
        /// - Replication strategy: Based on min_providers or storage_period
        /// - Provider selection: Automatic based on availability/capacity
        /// - Number of providers:
        ///   - If min_providers specified: uses that value
        ///   - Otherwise: 3 (1 primary + 2 replicas) for periods > 1000 blocks, 1 for shorter
        /// - Payment distribution: Split equally across all providers
        fn allocate_bucket_for_user(
            user: &T::AccountId,
            max_capacity: u64,
            storage_period: BlockNumberFor<T>,
            payment: BalanceOf<T>,
            min_providers: Option<u8>,
        ) -> Result<u64, Error<T>> {
            use sp_runtime::traits::CheckedDiv;

            // Determine number of providers
            let num_providers: u8 = if let Some(min) = min_providers {
                // Use explicitly specified minimum
                ensure!(min > 0, Error::<T>::InvalidProviderCount);
                min
            } else {
                // Auto-determine based on storage period
                let threshold_blocks = BlockNumberFor::<T>::from(1000u32);
                if storage_period > threshold_blocks {
                    3 // 1 primary + 2 replicas for long-term storage
                } else {
                    1 // Primary only for short-term storage
                }
            };

            // Step 1: Create bucket in Layer 0 with min_providers requirement
            let bucket_id = pallet_storage_provider::Pallet::<T>::create_bucket_internal(
                user,
                num_providers as u32,
            )
            .map_err(|_| Error::<T>::BucketCreationFailed)?;

            // Step 2: Calculate payment per provider
            use sp_runtime::traits::SaturatedConversion;
            let divisor: BalanceOf<T> = (num_providers as u32).saturated_into();
            let payment_per_provider = payment
                .checked_div(&divisor)
                .ok_or(Error::<T>::BucketCreationFailed)?;

            // Step 3: Find available providers for primary storage
            let available_primary_providers =
                pallet_storage_provider::Pallet::<T>::query_available_providers(
                    max_capacity,
                    true, // accepting_primary
                );

            ensure!(
                !available_primary_providers.is_empty(),
                Error::<T>::NoProvidersAvailable
            );

            // Select first available provider for primary
            let primary_provider = &available_primary_providers[0];

            // Step 4: Request primary agreement
            pallet_storage_provider::Pallet::<T>::request_primary_agreement_internal(
                user,
                bucket_id,
                primary_provider,
                max_capacity,
                storage_period,
                payment_per_provider,
            )
            .map_err(|_| Error::<T>::BucketCreationFailed)?;

            // Step 5: Request replica agreements (if num_providers > 1)
            if num_providers > 1 {
                let available_replica_providers =
                    pallet_storage_provider::Pallet::<T>::query_available_providers(
                        max_capacity,
                        false, // accepting replicas
                    );

                // Ensure we have enough replica providers
                let num_replicas = (num_providers - 1) as usize;
                ensure!(
                    available_replica_providers.len() >= num_replicas,
                    Error::<T>::InsufficientReplicaProviders
                );

                // Request replica agreements (skip primary provider if it's in the list)
                let mut replica_count = 0;
                for replica_provider in available_replica_providers.iter() {
                    if replica_count >= num_replicas {
                        break;
                    }

                    // Skip if this is the primary provider
                    if replica_provider == primary_provider {
                        continue;
                    }

                    // Calculate sync balance (10% of payment for sync operations)
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
        pub fn get_drive(
            drive_id: DriveId,
        ) -> Option<DriveInfo<T::AccountId, BlockNumberFor<T>, T::MaxDriveNameLength, BalanceOf<T>>> {
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
