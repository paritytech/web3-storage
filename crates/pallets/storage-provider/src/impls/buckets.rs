// SPDX-License-Identifier: Apache-2.0

use crate::Member;
use crate::*;
use frame_support::{pallet_prelude::*, traits::Consideration};
use storage_primitives::{BucketId, Commitment, Role, Visibility};

impl<T: Config> Pallet<T> {
    /// Removes a bucket that has no agreements left: releases every member
    /// deposit to its depositor and clears the member's reverse-index entry,
    /// deletes the record and releases the bucket deposit to its creator.
    pub(crate) fn remove_bucket_internal(bucket_id: BucketId, bucket: Bucket<T>) -> DispatchResult {
        for member in bucket.members {
            member.deposit.drop(&member.depositor)?;
            Self::remove_from_member_index(&member.account, bucket_id);
        }

        Buckets::<T>::remove(bucket_id);
        bucket.deposit.drop(&bucket.creator)?;

        Self::deposit_event(Event::BucketDeleted { bucket_id });

        Ok(())
    }

    /// Creates a bucket with `admin` as its sole admin.
    ///
    /// Parameters:
    /// - `admin`: Account that will be the bucket admin.
    /// - `min_providers`: Minimum number of primary providers required to
    ///   sign each checkpoint.
    /// - `initial_primary`: Optional provider to seed as the bucket's
    ///   first `primary_providers` entry. Used by
    ///   `create_bucket_with_primary_internal` to atomically create the
    ///   bucket together with its primary agreement; pass `None` for
    ///   buckets that will register primaries later.
    /// - `visibility`: Read visibility. Creation surfaces that omit the
    ///   choice must pass `Visibility::Private` (fail-safe default).
    ///
    /// Returns: bucket_id
    pub(crate) fn create_bucket_internal(
        admin: &T::AccountId,
        min_providers: u32,
        initial_primary: Option<&T::AccountId>,
        visibility: Visibility,
    ) -> Result<BucketId, DispatchError> {
        ensure!(
            min_providers >= 1 && min_providers <= T::MaxPrimaryProviders::get(),
            Error::<T>::InvalidMinProviders
        );

        let bucket_id = NextBucketId::<T>::get();
        NextBucketId::<T>::put(bucket_id.saturating_add(1));

        let admin_member = Member {
            account: admin.clone(),
            role: Role::Admin,
            depositor: admin.clone(),
            deposit: T::StorageDeposit::new(admin, Self::member_footprint())?,
        };

        let mut members = BoundedVec::new();
        members
            .try_push(admin_member)
            .map_err(|_| Error::<T>::MaxMembersReached)?;

        let mut primary_providers = BoundedVec::new();
        if let Some(p) = initial_primary {
            primary_providers
                .try_push(p.clone())
                .map_err(|_| Error::<T>::MaxPrimaryProvidersReached)?;
        }

        let bucket = Bucket {
            members,
            frozen_start_seq: None,
            min_providers,
            primary_providers,
            snapshot: None,
            historical_roots: [(0, Commitment::default()); 6],
            total_snapshots: 0,
            visibility,
            creator: admin.clone(),
            deposit: T::StorageDeposit::new(admin, Self::bucket_footprint())?,
        };

        Buckets::<T>::insert(bucket_id, bucket);

        // Update reverse index for creator
        MemberBuckets::<T>::try_mutate(admin, |buckets| {
            buckets
                .try_push(bucket_id)
                .map_err(|_| Error::<T>::TooManyBucketsForMember)
        })?;

        Self::deposit_event(Event::BucketCreated {
            bucket_id,
            admin: admin.clone(),
        });

        Ok(bucket_id)
    }
}
