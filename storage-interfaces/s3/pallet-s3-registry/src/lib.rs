// SPDX-License-Identifier: Apache-2.0

//! S3 Registry Pallet
//!
//! This pallet provides on-chain storage for S3-compatible bucket and object metadata.

#![cfg_attr(not(feature = "std"), no_std)]

extern crate alloc;

pub use pallet::*;

#[cfg(feature = "runtime-benchmarks")]
pub mod benchmarking;
pub mod weights;
pub use weights::WeightInfo;

#[cfg(test)]
mod mock;

#[cfg(test)]
mod tests;

use alloc::vec::Vec;
use frame_support::pallet_prelude::*;
use frame_system::pallet_prelude::*;
use s3_primitives::{
    validate_bucket_name, validate_object_key, BucketName, MaxContentTypeLen, MaxEtagLen,
    MaxMetadataEntries, MaxMetadataKeyLen, MaxMetadataValueLen, MetadataEntry, ObjectKey,
    ObjectMetadata, S3BucketId, S3BucketInfo,
};
use sp_core::H256;
use sp_runtime::{BoundedVec, SaturatedConversion};

#[frame_support::pallet]
pub mod pallet {
    use super::*;
    #[cfg(feature = "try-runtime")]
    use alloc::collections::BTreeSet;
    #[cfg(feature = "try-runtime")]
    use sp_runtime::TryRuntimeError;

    /// S3 bucket info type alias.
    pub type S3BucketInfoOf<T> =
        S3BucketInfo<<T as frame_system::Config>::AccountId, BlockNumberFor<T>>;

    /// Balance type (inherited from the storage provider pallet's Currency).
    pub type BalanceOf<T> =
        <<T as pallet_storage_provider::Config>::Currency as frame_support::traits::Currency<
            <T as frame_system::Config>::AccountId,
        >>::Balance;

    #[pallet::pallet]
    pub struct Pallet<T>(_);

    #[pallet::hooks]
    impl<T: Config> Hooks<BlockNumberFor<T>> for Pallet<T> {
        #[cfg(feature = "try-runtime")]
        fn try_state(_block: BlockNumberFor<T>) -> Result<(), TryRuntimeError> {
            Self::do_try_state()
        }
    }

    #[pallet::config]
    pub trait Config:
        frame_system::Config<RuntimeEvent: From<Event<Self>>> + pallet_storage_provider::Config
    {
        /// Maximum number of buckets per user.
        #[pallet::constant]
        type MaxBucketsPerUser: Get<u32>;

        /// Maximum number of objects per bucket.
        #[pallet::constant]
        type MaxObjectsPerBucket: Get<u32>;

        /// Weight information for extrinsics in this pallet.
        type WeightInfo: WeightInfo;
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

    /// Object metadata: (S3BucketId, ObjectKey) -> ObjectMetadata
    #[pallet::storage]
    #[pallet::getter(fn objects)]
    pub type Objects<T: Config> = StorageDoubleMap<
        _,
        Blake2_128Concat,
        S3BucketId,
        Blake2_128Concat,
        ObjectKey,
        ObjectMetadata,
        OptionQuery,
    >;

    /// Next S3 bucket ID (auto-increment).
    #[pallet::storage]
    #[pallet::getter(fn next_s3_bucket_id)]
    pub type NextS3BucketId<T: Config> = StorageValue<_, S3BucketId, ValueQuery>;

    #[cfg(feature = "try-runtime")]
    impl<T: Config> Pallet<T> {
        /// Cross-storage index and counter invariants, checked on every
        /// try-runtime block and runtime-upgrade dry-run (read-only, never panics).
        pub fn do_try_state() -> Result<(), TryRuntimeError> {
            let next_id = NextS3BucketId::<T>::get();
            for (id, info) in S3Buckets::<T>::iter() {
                // `NextS3BucketId` strictly exceeds every live id.
                ensure!(
                    id < next_id,
                    "NextS3BucketId does not exceed a live S3BucketId"
                );

                // `BucketNameToId` is the exact inverse of the bucket name.
                ensure!(
                    BucketNameToId::<T>::get(&info.name) == Some(id),
                    "S3 bucket name has no matching BucketNameToId entry"
                );

                // `UserBuckets` completeness: the bucket is listed under its owner.
                ensure!(
                    UserBuckets::<T>::get(&info.owner).contains(&id),
                    "S3 bucket missing from its owner's UserBuckets"
                );

                // `object_count` / `total_size` match the actual `Objects` entries.
                let mut count: u64 = 0;
                let mut total: u64 = 0;
                for (_key, obj) in Objects::<T>::iter_prefix(id) {
                    count = count.saturating_add(1);
                    total = total
                        .checked_add(obj.size)
                        .ok_or("S3 bucket total_size overflows u64")?;
                }
                ensure!(
                    info.object_count == count,
                    "S3 bucket object_count != number of Objects"
                );
                ensure!(
                    info.total_size == total,
                    "S3 bucket total_size != sum of object sizes"
                );
            }

            // `BucketNameToId` reverse: every entry points to a live bucket with that name.
            for (name, id) in BucketNameToId::<T>::iter() {
                let info = S3Buckets::<T>::get(id)
                    .ok_or("BucketNameToId references a non-existent bucket")?;
                ensure!(
                    info.name == name,
                    "BucketNameToId key does not match the bucket's name"
                );
            }

            // `UserBuckets` correctness: no duplicates, and every entry is owned
            // by the account it is listed under.
            for (owner, ids) in UserBuckets::<T>::iter() {
                let unique: BTreeSet<S3BucketId> = ids.iter().copied().collect();
                ensure!(
                    unique.len() == ids.len(),
                    "duplicate S3 bucket id in UserBuckets entry"
                );
                for id in ids.iter() {
                    let info = S3Buckets::<T>::get(id)
                        .ok_or("UserBuckets references a non-existent bucket")?;
                    ensure!(
                        info.owner == owner,
                        "UserBuckets entry not owned by the account"
                    );
                }
            }
            Ok(())
        }
    }

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
        /// Object metadata stored.
        ObjectPut {
            s3_bucket_id: S3BucketId,
            key: Vec<u8>,
            cid: H256,
            size: u64,
        },
        /// Object deleted.
        ObjectDeleted {
            s3_bucket_id: S3BucketId,
            key: Vec<u8>,
        },
        /// Object copied.
        ObjectCopied {
            src_bucket_id: S3BucketId,
            src_key: Vec<u8>,
            dst_bucket_id: S3BucketId,
            dst_key: Vec<u8>,
        },
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
        /// Object not found.
        ObjectNotFound,
        /// Invalid object key format.
        InvalidObjectKey,
        /// Bucket is not empty.
        BucketNotEmpty,
        /// Too many objects in bucket.
        TooManyObjects,
        /// Object key too long.
        ObjectKeyTooLong,
        /// Content type too long.
        ContentTypeTooLong,
        /// Bucket total size would exceed the maximum supported value.
        BucketSizeLimitReached,
    }

    #[pallet::call]
    impl<T: Config> Pallet<T> {
        /// Create a new S3 bucket.
        ///
        /// This automatically creates an underlying Layer 0 bucket and links it
        /// to the S3 bucket. The caller becomes the owner of both buckets.
        ///
        /// Parameters:
        /// - `name`: S3 bucket name (3-63 chars, lowercase alphanumeric + hyphens)
        /// - `provider`: Explicit provider account that signed the terms.
        /// - `terms`: Provider-signed agreement terms.
        /// - `sig`: Provider signature over the SCALE-encoded terms.
        #[pallet::call_index(0)]
        #[pallet::weight(<T as Config>::WeightInfo::create_s3_bucket())]
        pub fn create_s3_bucket(
            origin: OriginFor<T>,
            name: Vec<u8>,
            provider: T::AccountId,
            terms: pallet_storage_provider::AgreementTermsOf<T>,
            sig: sp_runtime::MultiSignature,
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

            // Create Layer 0 bucket + primary agreement atomically. Layer 0
            // errors (bad signature, replay, capacity, price, …) surface
            // directly so callers can act on them.
            let layer0_bucket_id =
                pallet_storage_provider::Pallet::<T>::establish_storage_agreement_internal(
                    &who, &provider, terms, &sig,
                )?;

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
                object_count: 0,
                total_size: 0,
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
        /// The bucket must be empty and caller must be the owner.
        #[pallet::call_index(1)]
        #[pallet::weight(<T as Config>::WeightInfo::delete_s3_bucket())]
        pub fn delete_s3_bucket(origin: OriginFor<T>, s3_bucket_id: S3BucketId) -> DispatchResult {
            let who = ensure_signed(origin)?;

            let bucket_info =
                S3Buckets::<T>::get(s3_bucket_id).ok_or(Error::<T>::BucketNotFound)?;
            ensure!(bucket_info.owner == who, Error::<T>::NotBucketOwner);
            ensure!(bucket_info.object_count == 0, Error::<T>::BucketNotEmpty);

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

        /// Store or update object metadata.
        #[pallet::call_index(2)]
        #[pallet::weight(<T as Config>::WeightInfo::put_object_metadata())]
        pub fn put_object_metadata(
            origin: OriginFor<T>,
            s3_bucket_id: S3BucketId,
            key: Vec<u8>,
            cid: H256,
            size: u64,
            content_type: Vec<u8>,
            user_metadata: Vec<(Vec<u8>, Vec<u8>)>,
        ) -> DispatchResult {
            let who = ensure_signed(origin)?;

            // Validate key
            ensure!(validate_object_key(&key), Error::<T>::InvalidObjectKey);

            // Get and verify bucket ownership
            let mut bucket_info =
                S3Buckets::<T>::get(s3_bucket_id).ok_or(Error::<T>::BucketNotFound)?;
            ensure!(bucket_info.owner == who, Error::<T>::NotBucketOwner);

            // Convert key to bounded vec
            let bounded_key: ObjectKey = key
                .clone()
                .try_into()
                .map_err(|_| Error::<T>::ObjectKeyTooLong)?;

            // Convert content type
            let bounded_content_type: BoundedVec<u8, MaxContentTypeLen> =
                content_type
                    .try_into()
                    .map_err(|_| Error::<T>::ContentTypeTooLong)?;

            // Compute ETag (hex of CID)
            let etag_bytes = Self::cid_to_etag(&cid);
            let bounded_etag: BoundedVec<u8, MaxEtagLen> =
                etag_bytes.try_into().unwrap_or_default();

            // Convert user metadata
            let bounded_metadata: BoundedVec<MetadataEntry, MaxMetadataEntries> = user_metadata
                .into_iter()
                .filter_map(|(k, v)| {
                    let key: BoundedVec<u8, MaxMetadataKeyLen> = k.try_into().ok()?;
                    let value: BoundedVec<u8, MaxMetadataValueLen> = v.try_into().ok()?;
                    Some(MetadataEntry { key, value })
                })
                .take(MaxMetadataEntries::get() as usize)
                .collect::<Vec<_>>()
                .try_into()
                .unwrap_or_default();

            // Get current timestamp
            let timestamp = frame_system::Pallet::<T>::block_number().saturated_into::<u64>();

            // Check if this is an update or new object
            match Objects::<T>::get(s3_bucket_id, &bounded_key) {
                None => {
                    ensure!(
                        bucket_info.object_count < T::MaxObjectsPerBucket::get() as u64,
                        Error::<T>::TooManyObjects
                    );
                    bucket_info.object_count = bucket_info.object_count.saturating_add(1);
                }
                Some(old_metadata) => {
                    bucket_info.total_size =
                        bucket_info.total_size.saturating_sub(old_metadata.size);
                }
            }

            // Update bucket stats
            bucket_info.total_size = bucket_info
                .total_size
                .checked_add(size)
                .ok_or(Error::<T>::BucketSizeLimitReached)?;
            S3Buckets::<T>::insert(s3_bucket_id, bucket_info);

            // Create metadata
            let metadata = ObjectMetadata {
                cid,
                size,
                last_modified: timestamp,
                content_type: bounded_content_type,
                etag: bounded_etag,
                user_metadata: bounded_metadata,
            };

            // Store object metadata
            Objects::<T>::insert(s3_bucket_id, &bounded_key, metadata);

            Self::deposit_event(Event::ObjectPut {
                s3_bucket_id,
                key,
                cid,
                size,
            });

            Ok(())
        }

        /// Delete object metadata.
        #[pallet::call_index(3)]
        #[pallet::weight(<T as Config>::WeightInfo::delete_object_metadata())]
        pub fn delete_object_metadata(
            origin: OriginFor<T>,
            s3_bucket_id: S3BucketId,
            key: Vec<u8>,
        ) -> DispatchResult {
            let who = ensure_signed(origin)?;

            // Get and verify bucket ownership
            let mut bucket_info =
                S3Buckets::<T>::get(s3_bucket_id).ok_or(Error::<T>::BucketNotFound)?;
            ensure!(bucket_info.owner == who, Error::<T>::NotBucketOwner);

            // Convert key
            let bounded_key: ObjectKey = key
                .clone()
                .try_into()
                .map_err(|_| Error::<T>::ObjectKeyTooLong)?;

            // Get and remove object
            let metadata =
                Objects::<T>::take(s3_bucket_id, &bounded_key).ok_or(Error::<T>::ObjectNotFound)?;

            // Update bucket stats
            bucket_info.object_count = bucket_info.object_count.saturating_sub(1);
            bucket_info.total_size = bucket_info.total_size.saturating_sub(metadata.size);
            S3Buckets::<T>::insert(s3_bucket_id, bucket_info);

            Self::deposit_event(Event::ObjectDeleted { s3_bucket_id, key });

            Ok(())
        }

        /// Copy object metadata from one location to another.
        #[pallet::call_index(4)]
        #[pallet::weight(<T as Config>::WeightInfo::copy_object_metadata())]
        pub fn copy_object_metadata(
            origin: OriginFor<T>,
            src_bucket_id: S3BucketId,
            src_key: Vec<u8>,
            dst_bucket_id: S3BucketId,
            dst_key: Vec<u8>,
        ) -> DispatchResult {
            let who = ensure_signed(origin)?;

            // Verify source bucket ownership
            let src_bucket =
                S3Buckets::<T>::get(src_bucket_id).ok_or(Error::<T>::BucketNotFound)?;
            ensure!(src_bucket.owner == who, Error::<T>::NotBucketOwner);

            // Verify destination bucket ownership
            let dst_bucket =
                S3Buckets::<T>::get(dst_bucket_id).ok_or(Error::<T>::BucketNotFound)?;
            ensure!(dst_bucket.owner == who, Error::<T>::NotBucketOwner);

            // Convert keys
            let bounded_src_key: ObjectKey = src_key
                .clone()
                .try_into()
                .map_err(|_| Error::<T>::ObjectKeyTooLong)?;
            let bounded_dst_key: ObjectKey = dst_key
                .clone()
                .try_into()
                .map_err(|_| Error::<T>::ObjectKeyTooLong)?;

            // Get source object
            let mut metadata = Objects::<T>::get(src_bucket_id, &bounded_src_key)
                .ok_or(Error::<T>::ObjectNotFound)?;

            // Update last modified
            metadata.last_modified =
                frame_system::Pallet::<T>::block_number().saturated_into::<u64>();

            // Update destination bucket stats (re-read if same bucket since src was read separately)
            let mut dst_bucket =
                S3Buckets::<T>::get(dst_bucket_id).ok_or(Error::<T>::BucketNotFound)?;

            match Objects::<T>::get(dst_bucket_id, &bounded_dst_key) {
                None => {
                    ensure!(
                        dst_bucket.object_count < T::MaxObjectsPerBucket::get() as u64,
                        Error::<T>::TooManyObjects
                    );
                    dst_bucket.object_count = dst_bucket.object_count.saturating_add(1);
                }
                Some(old) => {
                    dst_bucket.total_size = dst_bucket.total_size.saturating_sub(old.size);
                }
            }

            dst_bucket.total_size = dst_bucket
                .total_size
                .checked_add(metadata.size)
                .ok_or(Error::<T>::BucketSizeLimitReached)?;
            S3Buckets::<T>::insert(dst_bucket_id, dst_bucket);

            // Store copy
            Objects::<T>::insert(dst_bucket_id, &bounded_dst_key, metadata);

            Self::deposit_event(Event::ObjectCopied {
                src_bucket_id,
                src_key,
                dst_bucket_id,
                dst_key,
            });

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

        /// Get object metadata.
        pub fn get_object(s3_bucket_id: S3BucketId, key: &[u8]) -> Option<ObjectMetadata> {
            let bounded_key: ObjectKey = key.to_vec().try_into().ok()?;
            Objects::<T>::get(s3_bucket_id, &bounded_key)
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

        /// Convert CID to ETag (hex string).
        fn cid_to_etag(cid: &H256) -> Vec<u8> {
            let bytes = cid.as_bytes();
            let mut result = Vec::with_capacity(64);
            for byte in bytes {
                result.push(Self::hex_char(byte >> 4));
                result.push(Self::hex_char(byte & 0x0f));
            }
            result
        }

        fn hex_char(nibble: u8) -> u8 {
            match nibble {
                0..=9 => b'0' + nibble,
                10..=15 => b'a' + nibble - 10,
                _ => b'0',
            }
        }
    }
}
