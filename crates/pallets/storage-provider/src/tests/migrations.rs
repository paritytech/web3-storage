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
    members: BoundedVec<Member<Test>, <Test as Config>::MaxMembers>,
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
        let member = Member {
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

// ─────────────────────────────────────────────────────────────────────────────
// v1 -> v2: challenger tier + bucket visibility (#330)
// ─────────────────────────────────────────────────────────────────────────────

/// Mirrors the pre-#330 `ProviderStats` layout (single `challenges_received`).
#[derive(Encode)]
struct V1ProviderStats {
    registered_at: u64,
    agreements_total: u32,
    agreements_extended: u32,
    agreements_not_extended: u32,
    agreements_burned: u32,
    total_bytes_committed: u64,
    challenges_received: u32,
    challenges_failed: u32,
}

/// Mirrors the pre-#330 `ProviderInfo` layout (old stats, otherwise current).
#[derive(Encode)]
struct V1ProviderInfo {
    multiaddr: BoundedVec<u8, <Test as Config>::MaxMultiaddrLength>,
    public_key: BoundedVec<u8, frame_support::traits::ConstU32<64>>,
    stake: u64,
    committed_bytes: u64,
    settings: ProviderSettings<Test>,
    stats: V1ProviderStats,
    deregister_at: Option<u64>,
}

/// Mirrors the pre-#330 `Bucket` layout (no `visibility`, snapshot already
/// carries `commitment_nonce` — i.e. the v1 layout).
#[derive(Encode)]
struct V1Bucket {
    members: BoundedVec<Member<Test>, <Test as Config>::MaxMembers>,
    frozen_start_seq: Option<u64>,
    min_providers: u32,
    primary_providers: BoundedVec<u64, <Test as Config>::MaxPrimaryProviders>,
    snapshot: Option<storage_primitives::BucketSnapshot<u64>>,
    historical_roots: [(u32, H256); 6],
    total_snapshots: u32,
}

/// Mirrors the pre-#330 `Challenge` layout (no `authorized`).
#[derive(Encode)]
struct V1Challenge {
    bucket_id: BucketId,
    provider: u64,
    challenger: u64,
    mmr_root: H256,
    start_seq: u64,
    target: storage_primitives::ChunkLocation,
    deposit: u64,
}

fn v1_bucket(admin: u64) -> V1Bucket {
    V1Bucket {
        members: vec![Member {
            account: admin,
            role: Role::Admin,
        }]
        .try_into()
        .unwrap(),
        frozen_start_seq: None,
        min_providers: 1,
        primary_providers: vec![2u64].try_into().unwrap(),
        snapshot: None,
        historical_roots: [(0, H256::zero()); 6],
        total_snapshots: 0,
    }
}

#[test]
fn v2_migrates_all_three_layouts() {
    new_test_ext().execute_with(|| {
        use crate::migrations::v2::InnerMigrateV1ToV2;

        // Old-layout provider with non-zero counters.
        unhashed::put_raw(
            &Providers::<Test>::hashed_key_for(2u64),
            &V1ProviderInfo {
                multiaddr: b"/ip4/127.0.0.1/tcp/3000".to_vec().try_into().unwrap(),
                public_key: vec![1u8; 32].try_into().unwrap(),
                stake: 200,
                committed_bytes: 50,
                settings: ProviderSettings::default(),
                stats: V1ProviderStats {
                    registered_at: 1,
                    agreements_total: 3,
                    agreements_extended: 1,
                    agreements_not_extended: 0,
                    agreements_burned: 0,
                    total_bytes_committed: 500,
                    challenges_received: 7,
                    challenges_failed: 2,
                },
                deregister_at: None,
            }
            .encode(),
        );

        // Old-layout bucket (admin 1) and two old-layout challenges against
        // it: one from the admin (authorized), one from a stranger (public).
        unhashed::put_raw(
            &Buckets::<Test>::hashed_key_for(0u64),
            &v1_bucket(1).encode(),
        );
        let challenge = |challenger: u64| V1Challenge {
            bucket_id: 0,
            provider: 2,
            challenger,
            mmr_root: H256::repeat_byte(0xAB),
            start_seq: 0,
            target: storage_primitives::ChunkLocation {
                leaf_index: 0,
                chunk_index: 0,
            },
            deposit: 100,
        };
        unhashed::put_raw(
            &Challenges::<Test>::hashed_key_for(101u64, 0u16),
            &challenge(1).encode(),
        );
        unhashed::put_raw(
            &Challenges::<Test>::hashed_key_for(101u64, 1u16),
            &challenge(9).encode(),
        );

        InnerMigrateV1ToV2::<Test>::on_runtime_upgrade();

        let provider = Providers::<Test>::get(2).expect("provider decodes under new layout");
        assert_eq!(provider.stake, 200);
        assert_eq!(provider.stats.agreements_total, 3);
        // At-creation count is dropped; resolution-time tier counters start fresh.
        assert_eq!(provider.stats.challenges_received_authorized, 0);
        assert_eq!(provider.stats.challenges_received_public, 0);
        assert_eq!(provider.stats.challenges_failed, 2);

        // Pre-existing buckets keep the open semantics they were created under.
        let bucket = Buckets::<Test>::get(0).expect("bucket decodes under new layout");
        assert_eq!(bucket.visibility, storage_primitives::Visibility::Public);

        // Tier recomputed as challenge creation would have snapshotted it.
        assert!(
            Challenges::<Test>::get(101, 0).unwrap().authorized,
            "admin is authorized tier"
        );
        assert!(
            !Challenges::<Test>::get(101, 1).unwrap().authorized,
            "stranger is public tier"
        );
    });
}
