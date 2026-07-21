// SPDX-License-Identifier: Apache-2.0

//! Tests for the v0 -> v1 `commitment_nonce` backfill migration.

use super::*;
use crate::migrations::v1::InnerMigrateV0ToV1;
use codec::Encode;
use frame_support::{storage::unhashed, traits::UncheckedOnRuntimeUpgrade, BoundedVec};
use sp_core::H256;
use storage_primitives::{BucketId, Commitment};

/// Mirrors the pre-#125 `BucketSnapshot` layout (no `commitment_nonce`), used
/// only to encode a raw old-format value for this test.
#[derive(Encode)]
struct OldBucketSnapshot {
    commitment: Commitment,
    checkpoint_block: u64,
    primary_signers: Vec<u8>,
}

/// Mirrors the pre-#125 `Bucket` layout, used only to encode a raw old-format
/// value for this test.
#[derive(Encode)]
struct OldBucket {
    members: BoundedVec<MemberOf<Test>, <Test as Config>::MaxMembers>,
    frozen_start_seq: Option<u64>,
    min_providers: u32,
    primary_providers: BoundedVec<u64, <Test as Config>::MaxPrimaryProviders>,
    snapshot: Option<OldBucketSnapshot>,
    historical_roots: [(u32, H256); 6],
    total_snapshots: u32,
}

fn put_old_bucket(bucket_id: BucketId, old: OldBucket) {
    unhashed::put_raw(&Buckets::<Test>::hashed_key_for(bucket_id), &old.encode());
}

#[test]
fn migration_backfills_commitment_nonce_on_existing_snapshot() {
    new_test_ext().execute_with(|| {
        let bucket_id: BucketId = 1;
        let member = MemberOf::<Test> {
            account: 42u64,
            role: Role::Admin,
        };
        put_old_bucket(
            bucket_id,
            OldBucket {
                members: vec![member.clone()].try_into().unwrap(),
                frozen_start_seq: None,
                min_providers: 1,
                primary_providers: vec![42u64].try_into().unwrap(),
                snapshot: Some(OldBucketSnapshot {
                    commitment: Commitment {
                        mmr_root: H256::repeat_byte(0xAB),
                        start_seq: 0,
                        leaf_count: 2,
                    },
                    checkpoint_block: 100,
                    primary_signers: vec![0b1],
                }),
                historical_roots: [(0, H256::default()); 6],
                total_snapshots: 1,
            },
        );

        InnerMigrateV0ToV1::<Test>::on_runtime_upgrade();

        let migrated = Buckets::<Test>::get(bucket_id).expect("bucket must still decode");
        assert_eq!(migrated.members.into_inner(), vec![member]);
        assert_eq!(migrated.frozen_start_seq, None);
        assert_eq!(migrated.min_providers, 1);
        assert_eq!(migrated.primary_providers.into_inner(), vec![42u64]);
        assert_eq!(migrated.total_snapshots, 1);
        let snapshot = migrated.snapshot.expect("snapshot must survive migration");
        assert_eq!(snapshot.checkpoint_block, 100);
        assert_eq!(snapshot.primary_signers, vec![0b1]);
        assert_eq!(snapshot.commitment.leaf_count, 2);
        assert_eq!(snapshot.commitment_nonce, 0);
    });
}

#[test]
fn migration_preserves_bucket_with_no_snapshot() {
    new_test_ext().execute_with(|| {
        let bucket_id: BucketId = 2;
        put_old_bucket(
            bucket_id,
            OldBucket {
                members: BoundedVec::default(),
                frozen_start_seq: Some(5),
                min_providers: 0,
                primary_providers: BoundedVec::default(),
                snapshot: None,
                historical_roots: [(0, H256::default()); 6],
                total_snapshots: 0,
            },
        );

        InnerMigrateV0ToV1::<Test>::on_runtime_upgrade();

        let migrated = Buckets::<Test>::get(bucket_id).expect("bucket must still decode");
        assert_eq!(migrated.frozen_start_seq, Some(5));
        assert!(migrated.snapshot.is_none());
    });
}
