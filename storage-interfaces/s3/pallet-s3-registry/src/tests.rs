//! Tests for S3 Registry pallet.

use crate::{mock::*, Error, S3Buckets};
use frame_support::{assert_noop, assert_ok, BoundedVec};

/// Default test parameters for bucket creation.
const DEFAULT_CAPACITY: u64 = 1_000_000_000; // 1 GB
const DEFAULT_PERIOD: u64 = 100; // blocks
const DEFAULT_PAYMENT: u128 = 1_000_000_000_000_000; // generous budget

/// Helper to register a provider with a unique multiaddr.
fn register_provider(who: u64) {
    let addr = format!("/ip4/127.0.0.1/tcp/{}", 3000 + who);
    let multiaddr: BoundedVec<u8, frame_support::traits::ConstU32<128>> =
        addr.into_bytes().try_into().unwrap();
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
        register_provider(2);

        assert_ok!(S3Registry::create_s3_bucket(
            RuntimeOrigin::signed(1),
            b"my-bucket".to_vec(),
            1, // min_providers
            DEFAULT_CAPACITY,
            DEFAULT_PERIOD,
            DEFAULT_PAYMENT,
        ));

        let bucket = S3Buckets::<Test>::get(0).unwrap();
        assert_eq!(bucket.name.as_slice(), b"my-bucket");
        assert_eq!(bucket.owner, 1);
        // Layer 0 bucket should have been created automatically
        assert!(pallet_storage_provider::Buckets::<Test>::get(bucket.layer0_bucket_id).is_some());
    });
}

#[test]
fn create_s3_bucket_auto_requests_agreement() {
    new_test_ext().execute_with(|| {
        register_provider(2);

        assert_ok!(S3Registry::create_s3_bucket(
            RuntimeOrigin::signed(1),
            b"my-bucket".to_vec(),
            1,
            DEFAULT_CAPACITY,
            DEFAULT_PERIOD,
            DEFAULT_PAYMENT,
        ));

        let bucket = S3Buckets::<Test>::get(0).unwrap();
        // Provider 2 should have a pending agreement request for this bucket
        assert!(pallet_storage_provider::AgreementRequests::<Test>::get(
            2,
            bucket.layer0_bucket_id
        )
        .is_some());
    });
}

#[test]
fn create_s3_bucket_with_multiple_providers() {
    new_test_ext().execute_with(|| {
        // Register 2 providers on accounts that have balance (2 and 3)
        register_provider(2);
        register_provider(3);

        assert_ok!(S3Registry::create_s3_bucket(
            RuntimeOrigin::signed(1),
            b"multi-provider-bucket".to_vec(),
            2, // min_providers
            DEFAULT_CAPACITY,
            DEFAULT_PERIOD,
            DEFAULT_PAYMENT,
        ));

        let bucket = S3Buckets::<Test>::get(0).unwrap();
        // Both providers should have pending agreement requests for this bucket
        assert!(pallet_storage_provider::AgreementRequests::<Test>::get(
            2,
            bucket.layer0_bucket_id
        )
        .is_some());
        assert!(pallet_storage_provider::AgreementRequests::<Test>::get(
            3,
            bucket.layer0_bucket_id
        )
        .is_some());
    });
}

#[test]
fn create_s3_bucket_fails_no_providers() {
    new_test_ext().execute_with(|| {
        // No providers registered
        assert_noop!(
            S3Registry::create_s3_bucket(
                RuntimeOrigin::signed(1),
                b"my-bucket".to_vec(),
                1,
                DEFAULT_CAPACITY,
                DEFAULT_PERIOD,
                DEFAULT_PAYMENT,
            ),
            Error::<Test>::NoProvidersAvailable
        );
    });
}

#[test]
fn create_s3_bucket_fails_insufficient_providers() {
    new_test_ext().execute_with(|| {
        // Only 1 provider registered, but requesting 3
        register_provider(2);

        assert_noop!(
            S3Registry::create_s3_bucket(
                RuntimeOrigin::signed(1),
                b"my-bucket".to_vec(),
                3, // requesting 3 providers
                DEFAULT_CAPACITY,
                DEFAULT_PERIOD,
                DEFAULT_PAYMENT,
            ),
            Error::<Test>::InsufficientProviders
        );
    });
}

#[test]
fn create_s3_bucket_fails_invalid_name() {
    new_test_ext().execute_with(|| {
        register_provider(2);

        assert_noop!(
            S3Registry::create_s3_bucket(
                RuntimeOrigin::signed(1),
                b"ab".to_vec(),
                1,
                DEFAULT_CAPACITY,
                DEFAULT_PERIOD,
                DEFAULT_PAYMENT,
            ),
            Error::<Test>::InvalidBucketName
        );

        assert_noop!(
            S3Registry::create_s3_bucket(
                RuntimeOrigin::signed(1),
                b"MyBucket".to_vec(),
                1,
                DEFAULT_CAPACITY,
                DEFAULT_PERIOD,
                DEFAULT_PAYMENT,
            ),
            Error::<Test>::InvalidBucketName
        );
    });
}

#[test]
fn create_s3_bucket_fails_duplicate_name() {
    new_test_ext().execute_with(|| {
        register_provider(2);

        assert_ok!(S3Registry::create_s3_bucket(
            RuntimeOrigin::signed(1),
            b"my-bucket".to_vec(),
            1,
            DEFAULT_CAPACITY,
            DEFAULT_PERIOD,
            DEFAULT_PAYMENT,
        ));

        assert_noop!(
            S3Registry::create_s3_bucket(
                RuntimeOrigin::signed(1),
                b"my-bucket".to_vec(),
                1,
                DEFAULT_CAPACITY,
                DEFAULT_PERIOD,
                DEFAULT_PAYMENT,
            ),
            Error::<Test>::BucketNameExists
        );
    });
}

#[test]
fn delete_s3_bucket_works() {
    new_test_ext().execute_with(|| {
        register_provider(2);

        assert_ok!(S3Registry::create_s3_bucket(
            RuntimeOrigin::signed(1),
            b"my-bucket".to_vec(),
            1,
            DEFAULT_CAPACITY,
            DEFAULT_PERIOD,
            DEFAULT_PAYMENT,
        ));

        assert_ok!(S3Registry::delete_s3_bucket(RuntimeOrigin::signed(1), 0));

        assert!(S3Buckets::<Test>::get(0).is_none());
    });
}
