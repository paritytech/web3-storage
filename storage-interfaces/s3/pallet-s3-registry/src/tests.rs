//! Tests for S3 Registry pallet.

use crate::{mock::*, Error, S3Buckets};
use frame_support::{assert_noop, assert_ok, traits::ConstU32, BoundedVec};
use pallet_storage_provider::ProviderSettings;

fn test_public_key() -> BoundedVec<u8, ConstU32<64>> {
    vec![1u8; 32].try_into().unwrap()
}

/// Register provider (account 3) with accepting_primary = true, capacity 200.
fn setup_provider() {
    let multiaddr: BoundedVec<u8, ConstU32<128>> =
        b"/ip4/127.0.0.1/tcp/3000".to_vec().try_into().unwrap();
    assert_ok!(StorageProvider::register_provider(
        RuntimeOrigin::signed(3),
        multiaddr,
        test_public_key(),
        10_000_000_000_000 // Must exceed MinProviderStake (1_000_000_000_000)
    ));
    let settings = ProviderSettings {
        min_duration: 10u64,
        max_duration: 10_000u64,
        price_per_byte: 0u128,
        accepting_primary: true,
        replica_sync_price: None,
        accepting_extensions: true,
        max_capacity: 10_000_000_000, // stake / MinStakePerByte
    };
    assert_ok!(StorageProvider::update_provider_settings(
        RuntimeOrigin::signed(3),
        settings
    ));
}

#[test]
fn create_s3_bucket_works() {
    new_test_ext().execute_with(|| {
        assert_ok!(S3Registry::create_s3_bucket(
            RuntimeOrigin::signed(1),
            b"my-bucket".to_vec(),
            1, // min_providers
        ));

        let bucket = S3Buckets::<Test>::get(0).unwrap();
        assert_eq!(bucket.name.as_slice(), b"my-bucket");
        assert_eq!(bucket.owner, 1);
        assert_eq!(bucket.object_count, 0);
        assert_eq!(bucket.total_size, 0);
        // Layer 0 bucket should have been created automatically
        assert!(pallet_storage_provider::Buckets::<Test>::get(bucket.layer0_bucket_id).is_some());
    });
}

#[test]
fn create_s3_bucket_sets_min_providers() {
    new_test_ext().execute_with(|| {
        assert_ok!(S3Registry::create_s3_bucket(
            RuntimeOrigin::signed(1),
            b"multi-provider-bucket".to_vec(),
            2, // min_providers
        ));

        let bucket = S3Buckets::<Test>::get(0).unwrap();
        // Layer 0 bucket should have min_providers set
        let l0_bucket =
            pallet_storage_provider::Buckets::<Test>::get(bucket.layer0_bucket_id).unwrap();
        assert_eq!(l0_bucket.min_providers, 2);
    });
}

#[test]
fn create_s3_bucket_fails_invalid_name() {
    new_test_ext().execute_with(|| {
        // Too short
        assert_noop!(
            S3Registry::create_s3_bucket(RuntimeOrigin::signed(1), b"ab".to_vec(), 1,),
            Error::<Test>::InvalidBucketName
        );

        // Uppercase not allowed
        assert_noop!(
            S3Registry::create_s3_bucket(RuntimeOrigin::signed(1), b"MyBucket".to_vec(), 1,),
            Error::<Test>::InvalidBucketName
        );
    });
}

#[test]
fn create_s3_bucket_fails_duplicate_name() {
    new_test_ext().execute_with(|| {
        assert_ok!(S3Registry::create_s3_bucket(
            RuntimeOrigin::signed(1),
            b"my-bucket".to_vec(),
            1,
        ));

        assert_noop!(
            S3Registry::create_s3_bucket(RuntimeOrigin::signed(1), b"my-bucket".to_vec(), 1,),
            Error::<Test>::BucketNameExists
        );
    });
}

#[test]
fn delete_s3_bucket_works() {
    new_test_ext().execute_with(|| {
        assert_ok!(S3Registry::create_s3_bucket(
            RuntimeOrigin::signed(1),
            b"my-bucket".to_vec(),
            1,
        ));

        assert_ok!(S3Registry::delete_s3_bucket(RuntimeOrigin::signed(1), 0));

        assert!(S3Buckets::<Test>::get(0).is_none());
    });
}

#[test]
fn delete_s3_bucket_fails_not_owner() {
    new_test_ext().execute_with(|| {
        assert_ok!(S3Registry::create_s3_bucket(
            RuntimeOrigin::signed(1),
            b"my-bucket".to_vec(),
            1,
        ));

        assert_noop!(
            S3Registry::delete_s3_bucket(RuntimeOrigin::signed(2), 0),
            Error::<Test>::NotBucketOwner
        );
    });
}

#[test]
fn delete_s3_bucket_fails_not_found() {
    new_test_ext().execute_with(|| {
        assert_noop!(
            S3Registry::delete_s3_bucket(RuntimeOrigin::signed(1), 999),
            Error::<Test>::BucketNotFound
        );
    });
}

#[test]
fn put_and_get_object_metadata_works() {
    new_test_ext().execute_with(|| {
        assert_ok!(S3Registry::create_s3_bucket(
            RuntimeOrigin::signed(1),
            b"my-bucket".to_vec(),
            1,
        ));

        let cid = sp_core::H256::repeat_byte(0xAB);
        assert_ok!(S3Registry::put_object_metadata(
            RuntimeOrigin::signed(1),
            0,
            b"photos/cat.jpg".to_vec(),
            cid,
            1024,
            b"image/jpeg".to_vec(),
            vec![],
        ));

        // Check bucket stats updated
        let bucket = S3Buckets::<Test>::get(0).unwrap();
        assert_eq!(bucket.object_count, 1);
        assert_eq!(bucket.total_size, 1024);

        // Check object exists
        let obj = S3Registry::get_object(0, b"photos/cat.jpg").unwrap();
        assert_eq!(obj.cid, cid);
        assert_eq!(obj.size, 1024);
    });
}

#[test]
fn delete_object_metadata_works() {
    new_test_ext().execute_with(|| {
        assert_ok!(S3Registry::create_s3_bucket(
            RuntimeOrigin::signed(1),
            b"my-bucket".to_vec(),
            1,
        ));

        let cid = sp_core::H256::repeat_byte(0xAB);
        assert_ok!(S3Registry::put_object_metadata(
            RuntimeOrigin::signed(1),
            0,
            b"photos/cat.jpg".to_vec(),
            cid,
            1024,
            b"image/jpeg".to_vec(),
            vec![],
        ));

        assert_ok!(S3Registry::delete_object_metadata(
            RuntimeOrigin::signed(1),
            0,
            b"photos/cat.jpg".to_vec(),
        ));

        // Check bucket stats updated
        let bucket = S3Buckets::<Test>::get(0).unwrap();
        assert_eq!(bucket.object_count, 0);
        assert_eq!(bucket.total_size, 0);

        // Check object removed
        assert!(S3Registry::get_object(0, b"photos/cat.jpg").is_none());
    });
}

#[test]
fn delete_nonempty_bucket_fails() {
    new_test_ext().execute_with(|| {
        assert_ok!(S3Registry::create_s3_bucket(
            RuntimeOrigin::signed(1),
            b"my-bucket".to_vec(),
            1,
        ));

        let cid = sp_core::H256::repeat_byte(0xAB);
        assert_ok!(S3Registry::put_object_metadata(
            RuntimeOrigin::signed(1),
            0,
            b"file.txt".to_vec(),
            cid,
            100,
            b"text/plain".to_vec(),
            vec![],
        ));

        assert_noop!(
            S3Registry::delete_s3_bucket(RuntimeOrigin::signed(1), 0),
            Error::<Test>::BucketNotEmpty
        );
    });
}

// --- create_s3_bucket_with_storage tests ---

#[test]
fn create_s3_bucket_with_storage_works() {
    new_test_ext().execute_with(|| {
        setup_provider();

        assert_ok!(S3Registry::create_s3_bucket_with_storage(
            RuntimeOrigin::signed(1),
            b"my-bucket".to_vec(),
            100,  // max_capacity
            500,  // duration
            1000, // max_payment
        ));

        let bucket = S3Buckets::<Test>::get(0).unwrap();
        assert_eq!(bucket.name.as_slice(), b"my-bucket");
        assert_eq!(bucket.owner, 1);
        assert_eq!(bucket.object_count, 0);
        assert_eq!(bucket.total_size, 0);

        // Layer 0 bucket should exist
        assert!(pallet_storage_provider::Buckets::<Test>::get(bucket.layer0_bucket_id).is_some());

        // Agreement should have been requested
        let l0_bucket =
            pallet_storage_provider::Buckets::<Test>::get(bucket.layer0_bucket_id).unwrap();
        assert_eq!(l0_bucket.min_providers, 1);
    });
}

#[test]
fn create_s3_bucket_with_storage_fails_no_providers() {
    new_test_ext().execute_with(|| {
        // No providers registered
        assert_noop!(
            S3Registry::create_s3_bucket_with_storage(
                RuntimeOrigin::signed(1),
                b"my-bucket".to_vec(),
                100,
                500,
                1000,
            ),
            Error::<Test>::NoProvidersAvailable
        );
    });
}

#[test]
fn create_s3_bucket_with_storage_fails_invalid_name() {
    new_test_ext().execute_with(|| {
        setup_provider();

        assert_noop!(
            S3Registry::create_s3_bucket_with_storage(
                RuntimeOrigin::signed(1),
                b"ab".to_vec(), // too short
                100,
                500,
                1000,
            ),
            Error::<Test>::InvalidBucketName
        );
    });
}

#[test]
fn create_s3_bucket_with_storage_fails_duplicate_name() {
    new_test_ext().execute_with(|| {
        setup_provider();

        assert_ok!(S3Registry::create_s3_bucket_with_storage(
            RuntimeOrigin::signed(1),
            b"my-bucket".to_vec(),
            100,
            500,
            1000,
        ));

        assert_noop!(
            S3Registry::create_s3_bucket_with_storage(
                RuntimeOrigin::signed(1),
                b"my-bucket".to_vec(),
                100,
                500,
                1000,
            ),
            Error::<Test>::BucketNameExists
        );
    });
}
