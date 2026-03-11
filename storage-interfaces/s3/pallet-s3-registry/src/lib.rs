//! S3 Registry Pallet
//!
//! This pallet provides on-chain storage for S3-compatible bucket metadata.
//! Object metadata is managed off-chain by the provider node's S3 index.

#![cfg_attr(not(feature = "std"), no_std)]

extern crate alloc;

pub use pallet::*;

#[cfg(test)]
mod mock;

#[cfg(test)]
mod tests;

use alloc::vec::Vec;
use frame_support::pallet_prelude::*;
use frame_system::pallet_prelude::*;
use s3_primitives::{validate_bucket_name, BucketName, S3BucketId, S3BucketInfo};
use sp_runtime::{traits::CheckedDiv, BoundedVec, SaturatedConversion};

#[frame_support::pallet]
pub mod pallet {
    use super::*;

    /// S3 bucket info type alias.
    pub type S3BucketInfoOf<T> =
        S3BucketInfo<<T as frame_system::Config>::AccountId, BlockNumberFor<T>>;

    /// Balance type from the storage provider pallet's currency.
    pub type BalanceOf<T> = pallet_storage_provider::BalanceOf<T>;

    #[pallet::pallet]
    pub struct Pallet<T>(_);

    #[pallet::config]
    pub trait Config:
        frame_system::Config<RuntimeEvent: From<Event<Self>>> + pallet_storage_provider::Config
    {
        /// Maximum number of buckets per user.
        #[pallet::constant]
        type MaxBucketsPerUser: Get<u32>;
    }

    /// S3 bucket registry: S3BucketId -> S3BucketInfo
    #[pallet::storage]
    #[pallet::getter(fn s3_buckets)]
    pub type S3Buckets<T: Config> =
        StorageMap<_, Blake2_128Concat, S3BucketId, S3BucketInfoOf<T>, OptionQuery>;

    /// Bucket name to ID mapping for uniqueness and lookup.
    #[pallet::storage]
    #[pallet::getter(fn bucket_name_to_id)]
    pub type BucketNameToId<T: Config> =
        StorageMap<_, Blake2_128Concat, BucketName, S3BucketId, OptionQuery>;

    /// User's S3 buckets.
    #[pallet::storage]
    #[pallet::getter(fn user_buckets)]
    pub type UserBuckets<T: Config> = StorageMap<
        _,
        Blake2_128Concat,
        T::AccountId,
        BoundedVec<S3BucketId, T::MaxBucketsPerUser>,
        ValueQuery,
    >;

    /// Next S3 bucket ID (auto-increment).
    #[pallet::storage]
    #[pallet::getter(fn next_s3_bucket_id)]
    pub type NextS3BucketId<T: Config> = StorageValue<_, S3BucketId, ValueQuery>;

    #[pallet::event]
    #[pallet::generate_deposit(pub(super) fn deposit_event)]
    pub enum Event<T: Config> {
        /// S3 bucket created.
        S3BucketCreated {
            s3_bucket_id: S3BucketId,
            name: Vec<u8>,
            layer0_bucket_id: u64,
            owner: T::AccountId,
        },
        /// S3 bucket deleted.
        S3BucketDeleted { s3_bucket_id: S3BucketId },
    }

    #[pallet::error]
    pub enum Error<T> {
        /// Bucket name already exists.
        BucketNameExists,
        /// Invalid bucket name format.
        InvalidBucketName,
        /// Bucket not found.
        BucketNotFound,
        /// Not the bucket owner/admin.
        NotBucketOwner,
        /// Too many buckets for user.
        TooManyBuckets,
        /// Layer 0 bucket creation failed.
        Layer0BucketCreationFailed,
        /// No storage providers are available with sufficient capacity.
        NoProvidersAvailable,
        /// Fewer providers available than the requested minimum.
        InsufficientProviders,
        /// Provider agreement request failed.
        AgreementRequestFailed,
    }

    #[pallet::call]
    impl<T: Config> Pallet<T> {
        /// Create a new S3 bucket with automatic provider matching.
        ///
        /// This automatically creates an underlying Layer 0 bucket, finds available
        /// storage providers, and requests storage agreements with them. The caller
        /// becomes the owner of both buckets.
        ///
        /// Parameters:
        /// - `name`: S3 bucket name (3-63 chars, lowercase alphanumeric + hyphens)
        /// - `min_providers`: Minimum number of storage providers required
        /// - `max_capacity`: Maximum bytes for the bucket
        /// - `storage_period`: Duration in blocks
        /// - `max_payment`: Total payment budget across all providers
        #[pallet::call_index(0)]
        #[pallet::weight(Weight::from_parts(100_000_000, 0))]
        pub fn create_s3_bucket(
            origin: OriginFor<T>,
            name: Vec<u8>,
            min_providers: u32,
            max_capacity: u64,
            storage_period: BlockNumberFor<T>,
            max_payment: BalanceOf<T>,
        ) -> DispatchResult {
            let who = ensure_signed(origin)?;

            // Validate bucket name
            ensure!(validate_bucket_name(&name), Error::<T>::InvalidBucketName);

            // Convert to bounded vec
            let bounded_name: BucketName = name
                .clone()
                .try_into()
                .map_err(|_| Error::<T>::InvalidBucketName)?;

            // Check name uniqueness
            ensure!(
                !BucketNameToId::<T>::contains_key(&bounded_name),
                Error::<T>::BucketNameExists
            );

            // Check user bucket limit
            let mut user_buckets = UserBuckets::<T>::get(&who);
            ensure!(
                user_buckets.len() < T::MaxBucketsPerUser::get() as usize,
                Error::<T>::TooManyBuckets
            );

            // Create Layer 0 bucket internally (this makes caller the admin)
            let layer0_bucket_id =
                pallet_storage_provider::Pallet::<T>::create_bucket_internal(&who, min_providers)?;

            // Auto-associate providers: find available providers and request agreements
            let available_providers =
                pallet_storage_provider::Pallet::<T>::query_available_providers(
                    max_capacity,
                    true, // accepting_primary
                );

            ensure!(
                !available_providers.is_empty(),
                Error::<T>::NoProvidersAvailable
            );
            ensure!(
                available_providers.len() >= min_providers as usize,
                Error::<T>::InsufficientProviders
            );

            // Split payment equally across providers
            let divisor: BalanceOf<T> = min_providers.saturated_into();
            let payment_per_provider = max_payment
                .checked_div(&divisor)
                .ok_or(Error::<T>::AgreementRequestFailed)?;

            // Request primary agreements with the first min_providers available
            for provider in available_providers.iter().take(min_providers as usize) {
                pallet_storage_provider::Pallet::<T>::request_primary_agreement_internal(
                    &who,
                    layer0_bucket_id,
                    provider,
                    max_capacity,
                    storage_period,
                    payment_per_provider,
                )
                .map_err(|_| Error::<T>::AgreementRequestFailed)?;
            }

            // Generate new S3 bucket ID
            let s3_bucket_id = NextS3BucketId::<T>::get();
            NextS3BucketId::<T>::put(s3_bucket_id.saturating_add(1));

            // Create bucket info
            let bucket_info = S3BucketInfo {
                s3_bucket_id,
                name: bounded_name.clone(),
                layer0_bucket_id,
                owner: who.clone(),
                created_at: frame_system::Pallet::<T>::block_number(),
            };

            // Store bucket
            S3Buckets::<T>::insert(s3_bucket_id, bucket_info);
            BucketNameToId::<T>::insert(&bounded_name, s3_bucket_id);

            // Update user buckets
            user_buckets
                .try_push(s3_bucket_id)
                .map_err(|_| Error::<T>::TooManyBuckets)?;
            UserBuckets::<T>::insert(&who, user_buckets);

            Self::deposit_event(Event::S3BucketCreated {
                s3_bucket_id,
                name,
                layer0_bucket_id,
                owner: who,
            });

            Ok(())
        }

        /// Delete an S3 bucket.
        ///
        /// Caller must be the owner.
        #[pallet::call_index(1)]
        #[pallet::weight(Weight::from_parts(50_000_000, 0))]
        pub fn delete_s3_bucket(origin: OriginFor<T>, s3_bucket_id: S3BucketId) -> DispatchResult {
            let who = ensure_signed(origin)?;

            let bucket_info =
                S3Buckets::<T>::get(s3_bucket_id).ok_or(Error::<T>::BucketNotFound)?;
            ensure!(bucket_info.owner == who, Error::<T>::NotBucketOwner);

            // Remove from storage
            S3Buckets::<T>::remove(s3_bucket_id);
            BucketNameToId::<T>::remove(&bucket_info.name);

            // Update user buckets
            let mut user_buckets = UserBuckets::<T>::get(&who);
            user_buckets.retain(|&id| id != s3_bucket_id);
            UserBuckets::<T>::insert(&who, user_buckets);

            Self::deposit_event(Event::S3BucketDeleted { s3_bucket_id });

            Ok(())
        }
    }

    impl<T: Config> Pallet<T> {
        /// Get bucket by name.
        pub fn get_bucket_by_name(name: &[u8]) -> Option<S3BucketInfoOf<T>> {
            let bounded_name: BucketName = name.to_vec().try_into().ok()?;
            let bucket_id = BucketNameToId::<T>::get(&bounded_name)?;
            S3Buckets::<T>::get(bucket_id)
        }

        /// Check if user is bucket owner.
        pub fn is_bucket_owner(s3_bucket_id: S3BucketId, account: &T::AccountId) -> bool {
            S3Buckets::<T>::get(s3_bucket_id)
                .map(|b| &b.owner == account)
                .unwrap_or(false)
        }

        /// Get the Layer 0 bucket ID for an S3 bucket.
        pub fn get_layer0_bucket_id(s3_bucket_id: S3BucketId) -> Option<u64> {
            S3Buckets::<T>::get(s3_bucket_id).map(|b| b.layer0_bucket_id)
        }
    }
}
