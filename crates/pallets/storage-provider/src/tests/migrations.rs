// SPDX-License-Identifier: Apache-2.0

//! `MigrateV0ToV1` re-encodes `Buckets` and `Providers` entries written in the
//! `v0.4.1-paseo` layout, before `Bucket` gained `visibility` and before
//! `ProviderStats` split `challenges_received` and gained `lifetime_revenue`.

use super::*;
use crate::migrations::v1::InnerMigrateV0ToV1;
use codec::Encode;
use frame_support::{traits::UncheckedOnRuntimeUpgrade, BoundedVec};
use sp_core::H256;
use storage_primitives::{BucketId, BucketSnapshot, Visibility};

#[test]
fn migrates_a_v0_bucket_by_defaulting_visibility_to_private() {
    new_test_ext().execute_with(|| {
        let bucket_id: BucketId = 3;
        let members = vec![Member {
            account: 1u64,
            role: Role::Admin,
        }];
        let primary_providers = vec![2u64];
        let historical_roots = [(7u32, H256::repeat_byte(9)); 6];

        // The v0 layout: the current `Bucket` minus `visibility`.
        let v0_encoded = (
            members.clone(),
            Some(11u64),
            2u32,
            primary_providers.clone(),
            None::<BucketSnapshot<BlockNumberFor<Test>>>,
            historical_roots,
            5u32,
        )
            .encode();
        frame_support::storage::unhashed::put_raw(
            &Buckets::<Test>::hashed_key_for(bucket_id),
            &v0_encoded,
        );

        InnerMigrateV0ToV1::<Test>::on_runtime_upgrade();

        let bucket = Buckets::<Test>::get(bucket_id).expect("bucket survives the migration");
        assert_eq!(
            bucket.visibility,
            Visibility::Private,
            "a bucket created before the field existed never consented to public reads"
        );
        assert_eq!(bucket.members.into_inner(), members);
        assert_eq!(bucket.frozen_start_seq, Some(11));
        assert_eq!(bucket.min_providers, 2);
        assert_eq!(bucket.primary_providers.into_inner(), primary_providers);
        assert_eq!(bucket.snapshot, None);
        assert_eq!(bucket.historical_roots, historical_roots);
        assert_eq!(bucket.total_snapshots, 5);
    });
}

#[test]
fn migrates_a_v0_provider_by_splitting_the_challenge_counter() {
    new_test_ext().execute_with(|| {
        let account: u64 = 4;
        let multiaddr: BoundedVec<u8, <Test as Config>::MaxMultiaddrLength> =
            b"/ip4/127.0.0.1/tcp/30333".to_vec().try_into().unwrap();
        let public_key = test_public_key();
        let settings = ProviderSettings::<Test> {
            min_duration: 10,
            max_duration: 1_000,
            price_per_byte: 3,
            accepting_primary: true,
            replica_sync_price: Some(7),
            accepting_extensions: false,
            max_capacity: 4_096,
        };

        // The v0 layout: `ProviderStats` with a single `challenges_received`
        // counter and no `lifetime_revenue`.
        let v0_stats = (
            21u64,    // registered_at
            6u32,     // agreements_total
            2u32,     // agreements_extended
            1u32,     // agreements_not_extended
            3u32,     // agreements_burned
            9_000u64, // total_bytes_committed
            8u32,     // challenges_received
            2u32,     // challenges_failed
        );
        let v0_encoded = (
            multiaddr.clone(),
            public_key.clone(),
            500u64,   // stake
            2_048u64, // committed_bytes
            settings.clone(),
            v0_stats,
            Some(99u64), // deregister_at
        )
            .encode();
        frame_support::storage::unhashed::put_raw(
            &Providers::<Test>::hashed_key_for(account),
            &v0_encoded,
        );

        InnerMigrateV0ToV1::<Test>::on_runtime_upgrade();

        let provider = Providers::<Test>::get(account).expect("provider survives the migration");
        assert_eq!(
            provider.stats.challenges_received_authorized, 8,
            "the public tier did not exist, so every counted challenge was authorized"
        );
        assert_eq!(provider.stats.challenges_received_public, 0);
        assert_eq!(provider.stats.challenges_failed, 2);
        assert_eq!(provider.stats.lifetime_revenue, 0);

        assert_eq!(provider.multiaddr, multiaddr);
        assert_eq!(provider.public_key, public_key);
        assert_eq!(provider.stake, 500);
        assert_eq!(provider.committed_bytes, 2_048);
        assert_eq!(provider.settings, settings);
        assert_eq!(provider.stats.registered_at, 21);
        assert_eq!(provider.stats.agreements_total, 6);
        assert_eq!(provider.stats.agreements_extended, 2);
        assert_eq!(provider.stats.agreements_not_extended, 1);
        assert_eq!(provider.stats.agreements_burned, 3);
        assert_eq!(provider.stats.total_bytes_committed, 9_000);
        assert_eq!(provider.deregister_at, Some(99));
    });
}
