// SPDX-License-Identifier: Apache-2.0

//! `MigrateV0ToV1` re-encodes `Drives` entries written in the pre-#105 layout
//! (with a trailing `payment` balance) into the current `DriveInfo`, which
//! has no `payment` field.

use super::*;
use crate::{migrations::v1::InnerMigrateV0ToV1, Drives};
use codec::Encode;
use file_system_primitives::DriveId;
use frame_support::{traits::UncheckedOnRuntimeUpgrade, BoundedVec};

#[test]
fn migrates_a_v0_drive_entry_to_v1() {
    new_test_ext().execute_with(|| {
        let drive_id: DriveId = 7;
        let owner: u64 = 1;
        let bucket_id: u64 = 42;
        let created_at: pallet_storage_provider::BlockNumberFor<Test> = 10;
        let name: Option<BoundedVec<u8, MaxDriveNameLength>> =
            Some(BoundedVec::try_from(b"My Drive".to_vec()).unwrap());
        let max_capacity: u64 = 1_000;
        let storage_period: pallet_storage_provider::BlockNumberFor<Test> = 500;
        let expires_at: pallet_storage_provider::BlockNumberFor<Test> = 510;
        let payment: crate::BalanceOf<Test> = 777;

        // The v0 layout, field for field: same types and order as the current
        // `DriveInfo`, plus the trailing `payment` the migration drops. A
        // tuple encodes identically to a struct with the same fields in the
        // same order, so this is exactly what a `v0.1.1-paseo` node wrote.
        let v0_encoded = (
            owner,
            bucket_id,
            created_at,
            name.clone(),
            max_capacity,
            storage_period,
            expires_at,
            payment,
        )
            .encode();
        frame_support::storage::unhashed::put_raw(
            &Drives::<Test>::hashed_key_for(drive_id),
            &v0_encoded,
        );

        let before = Drives::<Test>::iter_keys().count();

        InnerMigrateV0ToV1::<Test>::on_runtime_upgrade();

        let after = Drives::<Test>::iter().count();
        assert_eq!(before, after, "migration dropped or duplicated entries");

        let migrated = Drives::<Test>::get(drive_id).expect("entry survives the migration");
        assert_eq!(migrated.owner, owner);
        assert_eq!(migrated.bucket_id, bucket_id);
        assert_eq!(migrated.created_at, created_at);
        assert_eq!(migrated.name, name);
        assert_eq!(migrated.max_capacity, max_capacity);
        assert_eq!(migrated.storage_period, storage_period);
        assert_eq!(migrated.expires_at, expires_at);
    });
}
