// SPDX-License-Identifier: Apache-2.0

//! Tests for the v0 -> v1 challenger-tier / bucket-visibility migration (#330).

use super::*;
use crate::migrations::v1::InnerMigrateV0ToV1;
use codec::Encode;
use frame_support::{storage::unhashed, traits::UncheckedOnRuntimeUpgrade, BoundedVec};
use sp_core::H256;
use storage_primitives::BucketId;

/// Mirrors the pre-#330 `ProviderStats` layout (single `challenges_received`).
#[derive(Encode)]
struct V0ProviderStats {
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
struct V0ProviderInfo {
    multiaddr: BoundedVec<u8, <Test as Config>::MaxMultiaddrLength>,
    public_key: BoundedVec<u8, frame_support::traits::ConstU32<64>>,
    stake: u64,
    committed_bytes: u64,
    settings: ProviderSettings<Test>,
    stats: V0ProviderStats,
    deregister_at: Option<u64>,
}

/// Mirrors the pre-#330 `Bucket` layout (no `visibility`).
#[derive(Encode)]
struct V0Bucket {
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
struct V0Challenge {
    bucket_id: BucketId,
    provider: u64,
    challenger: u64,
    mmr_root: H256,
    start_seq: u64,
    target: storage_primitives::ChunkLocation,
    deposit: u64,
}

fn v0_bucket(admin: u64) -> V0Bucket {
    V0Bucket {
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
fn v1_migrates_all_three_layouts() {
    new_test_ext().execute_with(|| {
        // Old-layout provider with non-zero counters.
        unhashed::put_raw(
            &Providers::<Test>::hashed_key_for(2u64),
            &V0ProviderInfo {
                multiaddr: b"/ip4/127.0.0.1/tcp/3000".to_vec().try_into().unwrap(),
                public_key: vec![1u8; 32].try_into().unwrap(),
                stake: 200,
                committed_bytes: 50,
                settings: ProviderSettings::default(),
                stats: V0ProviderStats {
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
            &v0_bucket(1).encode(),
        );
        let challenge = |challenger: u64| V0Challenge {
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

        InnerMigrateV0ToV1::<Test>::on_runtime_upgrade();

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
