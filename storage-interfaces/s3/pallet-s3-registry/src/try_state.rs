// SPDX-License-Identifier: Apache-2.0

//! `try_state` invariant checks. They assert cross-storage index and counter
//! invariants against live state, catching violations that the extrinsic
//! guards (bypassed by `setStorage`/migrations) cannot. The
//! `Hooks::try_state` hook invoking them on every block / runtime-upgrade
//! dry-run is still gated by `try-runtime`, but the checks themselves are
//! always compiled and callable.
//!
//! Read-only, never panics: a violated invariant returns `TryRuntimeError`.

use crate::*;
use alloc::collections::BTreeSet;
use frame_support::pallet_prelude::*;
use s3_primitives::S3BucketId;
use sp_runtime::TryRuntimeError;

impl<T: Config> Pallet<T> {
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
            let info =
                S3Buckets::<T>::get(id).ok_or("BucketNameToId references a non-existent bucket")?;
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
