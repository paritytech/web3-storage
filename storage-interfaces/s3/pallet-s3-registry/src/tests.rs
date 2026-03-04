//! Tests for S3 Registry pallet.

use crate::{mock::*, Error, Objects, S3Buckets};
use frame_support::{assert_noop, assert_ok, BoundedVec};
use s3_primitives::MaxObjectKeyLen;
use sp_core::H256;

/// Helper to register a provider.
fn register_provider(who: u64) {
    let multiaddr: BoundedVec<u8, frame_support::traits::ConstU32<128>> =
        b"/ip4/127.0.0.1/tcp/3000".to_vec().try_into().unwrap();
    let public_key: BoundedVec<u8, frame_support::traits::ConstU32<64>> =
        b"01234567890123456789012345678901"
            .to_vec()
            .try_into()
            .unwrap();
    assert_ok!(StorageProvider::register_provider(
        RuntimeOrigin::signed(who),
        multiaddr,
        public_key,
        1_000_000_000_000, // Stake
    ));
    assert_ok!(StorageProvider::update_provider_settings(
        RuntimeOrigin::signed(who),
        pallet_storage_provider::ProviderSettings {
            accepting_primary: true,
            accepting_extensions: true,
            min_duration: 10,
            max_duration: 1000,
            price_per_byte: 1,
            replica_sync_price: None, // Not accepting replicas
            max_capacity: 1_000_000_000,
        }
    ));
}

#[test]
fn create_s3_bucket_works() {
    new_test_ext().execute_with(|| {
        register_provider(1);

        assert_ok!(S3Registry::create_s3_bucket(
            RuntimeOrigin::signed(1),
            b"my-bucket".to_vec(),
            1, // min_providers
        ));

        let bucket = S3Buckets::<Test>::get(0).unwrap();
        assert_eq!(bucket.name.as_slice(), b"my-bucket");
        assert_eq!(bucket.owner, 1);
        assert_eq!(bucket.object_count, 0);
        // Layer 0 bucket should have been created automatically
        assert!(pallet_storage_provider::Buckets::<Test>::get(bucket.layer0_bucket_id).is_some());
    });
}

#[test]
fn create_s3_bucket_fails_invalid_name() {
    new_test_ext().execute_with(|| {
        register_provider(1);

        assert_noop!(
            S3Registry::create_s3_bucket(RuntimeOrigin::signed(1), b"ab".to_vec(), 1),
            Error::<Test>::InvalidBucketName
        );

        assert_noop!(
            S3Registry::create_s3_bucket(RuntimeOrigin::signed(1), b"MyBucket".to_vec(), 1),
            Error::<Test>::InvalidBucketName
        );
    });
}

#[test]
fn create_s3_bucket_fails_duplicate_name() {
    new_test_ext().execute_with(|| {
        register_provider(1);

        assert_ok!(S3Registry::create_s3_bucket(
            RuntimeOrigin::signed(1),
            b"my-bucket".to_vec(),
            1,
        ));

        assert_noop!(
            S3Registry::create_s3_bucket(RuntimeOrigin::signed(1), b"my-bucket".to_vec(), 1),
            Error::<Test>::BucketNameExists
        );
    });
}

#[test]
fn delete_s3_bucket_works() {
    new_test_ext().execute_with(|| {
        register_provider(1);

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
fn delete_s3_bucket_fails_not_empty() {
    new_test_ext().execute_with(|| {
        register_provider(1);

        assert_ok!(S3Registry::create_s3_bucket(
            RuntimeOrigin::signed(1),
            b"my-bucket".to_vec(),
            1,
        ));

        let cid = H256::repeat_byte(0x42);
        assert_ok!(S3Registry::put_object_metadata(
            RuntimeOrigin::signed(1),
            0,
            b"test.txt".to_vec(),
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

#[test]
fn put_object_metadata_works() {
    new_test_ext().execute_with(|| {
        register_provider(1);

        assert_ok!(S3Registry::create_s3_bucket(
            RuntimeOrigin::signed(1),
            b"my-bucket".to_vec(),
            1,
        ));

        let cid = H256::repeat_byte(0x42);
        assert_ok!(S3Registry::put_object_metadata(
            RuntimeOrigin::signed(1),
            0,
            b"folder/test.txt".to_vec(),
            cid,
            1234,
            b"text/plain".to_vec(),
            vec![(b"x-custom".to_vec(), b"value".to_vec())],
        ));

        let key: BoundedVec<u8, MaxObjectKeyLen> = b"folder/test.txt".to_vec().try_into().unwrap();
        let metadata = Objects::<Test>::get(0, &key).unwrap();
        assert_eq!(metadata.cid, cid);
        assert_eq!(metadata.size, 1234);

        let bucket = S3Buckets::<Test>::get(0).unwrap();
        assert_eq!(bucket.object_count, 1);
        assert_eq!(bucket.total_size, 1234);
    });
}

#[test]
fn delete_object_metadata_works() {
    new_test_ext().execute_with(|| {
        register_provider(1);

        assert_ok!(S3Registry::create_s3_bucket(
            RuntimeOrigin::signed(1),
            b"my-bucket".to_vec(),
            1,
        ));

        let cid = H256::repeat_byte(0x42);
        assert_ok!(S3Registry::put_object_metadata(
            RuntimeOrigin::signed(1),
            0,
            b"test.txt".to_vec(),
            cid,
            100,
            b"text/plain".to_vec(),
            vec![],
        ));

        assert_ok!(S3Registry::delete_object_metadata(
            RuntimeOrigin::signed(1),
            0,
            b"test.txt".to_vec(),
        ));

        let key: BoundedVec<u8, MaxObjectKeyLen> = b"test.txt".to_vec().try_into().unwrap();
        assert!(Objects::<Test>::get(0, &key).is_none());

        let bucket = S3Buckets::<Test>::get(0).unwrap();
        assert_eq!(bucket.object_count, 0);
        assert_eq!(bucket.total_size, 0);
    });
}

#[test]
fn copy_object_metadata_works() {
    new_test_ext().execute_with(|| {
        register_provider(1);

        assert_ok!(S3Registry::create_s3_bucket(
            RuntimeOrigin::signed(1),
            b"bucket1".to_vec(),
            1,
        ));

        assert_ok!(S3Registry::create_s3_bucket(
            RuntimeOrigin::signed(1),
            b"bucket2".to_vec(),
            1,
        ));

        let cid = H256::repeat_byte(0x42);
        assert_ok!(S3Registry::put_object_metadata(
            RuntimeOrigin::signed(1),
            0,
            b"source.txt".to_vec(),
            cid,
            100,
            b"text/plain".to_vec(),
            vec![],
        ));

        assert_ok!(S3Registry::copy_object_metadata(
            RuntimeOrigin::signed(1),
            0,
            b"source.txt".to_vec(),
            1,
            b"copy.txt".to_vec(),
        ));

        let key: BoundedVec<u8, MaxObjectKeyLen> = b"copy.txt".to_vec().try_into().unwrap();
        let metadata = Objects::<Test>::get(1, &key).unwrap();
        assert_eq!(metadata.cid, cid);
        assert_eq!(metadata.size, 100);
    });
}
