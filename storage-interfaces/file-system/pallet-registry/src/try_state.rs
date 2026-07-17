// SPDX-License-Identifier: Apache-2.0

//! `try_state` invariant checks. They assert cross-storage index invariants
//! against live state, catching violations that the extrinsic guards
//! (bypassed by `setStorage`/migrations) cannot. The `Hooks::try_state` hook
//! invoking them on every block / runtime-upgrade dry-run is still gated by
//! `try-runtime`, but the checks themselves are always compiled and callable.
//!
//! Read-only, never panics: a violated invariant returns `TryRuntimeError`.

use crate::*;
use alloc::collections::BTreeSet;
use file_system_primitives::DriveId;
use frame_support::pallet_prelude::*;
use sp_runtime::TryRuntimeError;

impl<T: Config> Pallet<T> {
    pub fn do_try_state() -> Result<(), TryRuntimeError> {
        Self::check_bucket_to_drive()?;
        Self::check_drives()?;
        Self::check_user_drives()?;
        Ok(())
    }

    /// `BucketToDrive` is consistent, has no dangling entries, and is
    /// injective on drive ids (two buckets never map to one drive).
    fn check_bucket_to_drive() -> Result<(), TryRuntimeError> {
        let mut seen_drives: BTreeSet<DriveId> = BTreeSet::new();
        for (bucket_id, drive_id) in BucketToDrive::<T>::iter() {
            let drive = Drives::<T>::get(drive_id)
                .ok_or("BucketToDrive references a non-existent drive")?;
            ensure!(
                drive.bucket_id == bucket_id,
                "BucketToDrive maps a bucket to a drive with a different bucket_id"
            );
            ensure!(
                seen_drives.insert(drive_id),
                "BucketToDrive maps two buckets to the same drive (not injective)"
            );
        }
        Ok(())
    }

    /// Per-drive invariants: `NextDriveId` strictly exceeds every live drive
    /// id, and the `BucketToDrive` / `UserDrives` indexes list each drive.
    fn check_drives() -> Result<(), TryRuntimeError> {
        let next_id = NextDriveId::<T>::get();
        for (drive_id, drive) in Drives::<T>::iter() {
            ensure!(
                drive_id < next_id,
                "NextDriveId does not exceed a live DriveId"
            );
            // `BucketToDrive` completeness: each live drive maps back from its bucket.
            ensure!(
                BucketToDrive::<T>::get(drive.bucket_id) == Some(drive_id),
                "live drive has no matching BucketToDrive entry"
            );
            // `UserDrives` completeness: each live drive is listed under its owner.
            ensure!(
                UserDrives::<T>::get(&drive.owner).contains(&drive_id),
                "live drive missing from its owner's UserDrives"
            );
        }
        Ok(())
    }

    /// `UserDrives` correctness: no duplicates, and every entry is owned by
    /// the account it is listed under.
    fn check_user_drives() -> Result<(), TryRuntimeError> {
        for (owner, drive_ids) in UserDrives::<T>::iter() {
            let unique: BTreeSet<DriveId> = drive_ids.iter().copied().collect();
            ensure!(
                unique.len() == drive_ids.len(),
                "duplicate drive id in UserDrives entry"
            );
            for drive_id in drive_ids.iter() {
                let drive = Drives::<T>::get(drive_id)
                    .ok_or("UserDrives references a non-existent drive")?;
                ensure!(
                    drive.owner == owner,
                    "UserDrives entry not owned by the account"
                );
            }
        }
        Ok(())
    }
}
