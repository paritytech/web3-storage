//! Tests for S3 Registry pallet.

use crate::{mock::*, Error, S3Buckets};
use codec::Encode;
use frame_support::{assert_noop, assert_ok, traits::ConstU32, BoundedVec};
use pallet_storage_provider::{AgreementTermsOf, ProviderSettings};
use sp_core::crypto::KeyTypeId;
use storage_primitives::{AgreementTerms, BucketId};

const PROVIDER_KEY_TYPE: KeyTypeId = KeyTypeId(*b"prov");

#[allow(dead_code)]
fn test_public_key() -> BoundedVec<u8, ConstU32<64>> {
    vec![1u8; 32].try_into().unwrap()
}

/// Generate a provider sr25519 keypair via the runtime keystore.
fn generate_provider_public_key(
    seed: &str,
) -> (sp_core::sr25519::Public, BoundedVec<u8, ConstU32<64>>) {
    let public = sp_io::crypto::sr25519_generate(PROVIDER_KEY_TYPE, Some(seed.as_bytes().to_vec()));
    let bounded = public.0.to_vec().try_into().unwrap();
    (public, bounded)
}

/// Sign SCALE-encoded terms with the provider's keystore key.
fn sign_terms(
    public: &sp_core::sr25519::Public,
    terms: &AgreementTermsOf<Test>,
) -> sp_runtime::MultiSignature {
    let hash = sp_io::hashing::blake2_256(&terms.encode());
    let sig = sp_io::crypto::sr25519_sign(PROVIDER_KEY_TYPE, public, &hash)
        .expect("keystore signs with a key it generated");
    sp_runtime::MultiSignature::Sr25519(sig)
}

/// Build primary terms for the standard test provider.
fn primary_terms(owner: u64, max_bytes: u64, duration: u64, nonce: u64) -> AgreementTermsOf<Test> {
    AgreementTerms {
        owner,
        max_bytes,
        duration,
        price_per_byte: 0u128,
        valid_until: 1_000_000u64,
        nonce,
        bucket_id: None,
        replica_params: None,
    }
}

/// Register provider (account 3) with accepting_primary = true. Returns the
/// sr25519 public key it was registered with so callers can sign terms.
fn setup_provider() -> sp_core::sr25519::Public {
    let multiaddr: BoundedVec<u8, ConstU32<128>> =
        b"/ip4/127.0.0.1/tcp/3000".to_vec().try_into().unwrap();
    let (public, public_key_bytes) = generate_provider_public_key("//Provider");
    assert_ok!(StorageProvider::register_provider(
        RuntimeOrigin::signed(3),
        multiaddr,
        public_key_bytes,
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
    public
}

/// Register provider (account 3) and open an S3 bucket "my-bucket" owned by
/// `owner` via the signed-terms path. Returns the S3 bucket id.
///
/// Used by tests that just need *a* bucket to operate on (put/delete object,
/// delete bucket) and don't care about the agreement details.
fn setup_provider_and_s3_bucket(owner: u64, nonce: u64) -> u64 {
    let provider_pk = setup_provider();
    let terms = primary_terms(owner, 100, 500, nonce);
    let sig = sign_terms(&provider_pk, &terms);
    assert_ok!(S3Registry::create_s3_bucket(
        RuntimeOrigin::signed(owner),
        b"my-bucket".to_vec(),
        3,
        terms,
        sig,
    ));
    0
}

#[test]
fn create_s3_bucket_works() {
    new_test_ext().execute_with(|| {
        let provider_pk = setup_provider();
        let terms = primary_terms(1, 100, 500, 1);
        let sig = sign_terms(&provider_pk, &terms);

        assert_ok!(S3Registry::create_s3_bucket(
            RuntimeOrigin::signed(1),
            b"my-bucket".to_vec(),
            3,
            terms,
            sig,
        ));

        let bucket = S3Buckets::<Test>::get(0).unwrap();
        assert_eq!(bucket.name.as_slice(), b"my-bucket");
        assert_eq!(bucket.owner, 1);
        assert_eq!(bucket.object_count, 0);
        assert_eq!(bucket.total_size, 0);

        // Layer 0 bucket should exist with the named provider as the lone
        // primary, and the agreement was opened atomically.
        let l0_bucket =
            pallet_storage_provider::Buckets::<Test>::get(bucket.layer0_bucket_id).unwrap();
        assert_eq!(l0_bucket.min_providers, 1);
        assert_eq!(l0_bucket.primary_providers.to_vec(), vec![3]);
        let bid: BucketId = bucket.layer0_bucket_id;
        assert!(pallet_storage_provider::StorageAgreements::<Test>::contains_key(bid, 3));
    });
}

#[test]
fn create_s3_bucket_surfaces_layer0_signature_errors() {
    // If the named provider isn't registered, signature verification fails
    // at Layer 0 and that error surfaces directly through the S3 registry —
    // no custom NoProvidersAvailable / AgreementRequestFailed shim wraps it.
    new_test_ext().execute_with(|| {
        let (unregistered_pk, _) = generate_provider_public_key("//Ghost");
        let terms = primary_terms(1, 100, 500, 1);
        let sig = sign_terms(&unregistered_pk, &terms);
        assert_noop!(
            S3Registry::create_s3_bucket(
                RuntimeOrigin::signed(1),
                b"my-bucket".to_vec(),
                3,
                terms,
                sig,
            ),
            pallet_storage_provider::Error::<Test>::ProviderNotFound
        );
    });
}

#[test]
fn create_s3_bucket_fails_invalid_name() {
    new_test_ext().execute_with(|| {
        let provider_pk = setup_provider();
        let terms = primary_terms(1, 100, 500, 1);
        let sig = sign_terms(&provider_pk, &terms);

        // Too short — the S3 layer's name validation runs before Layer 0
        // so the nonce isn't consumed.
        assert_noop!(
            S3Registry::create_s3_bucket(
                RuntimeOrigin::signed(1),
                b"ab".to_vec(),
                3,
                terms.clone(),
                sig.clone(),
            ),
            Error::<Test>::InvalidBucketName
        );

        // Uppercase letters aren't allowed in S3 bucket names.
        assert_noop!(
            S3Registry::create_s3_bucket(
                RuntimeOrigin::signed(1),
                b"MyBucket".to_vec(),
                3,
                terms,
                sig,
            ),
            Error::<Test>::InvalidBucketName
        );
    });
}

#[test]
fn create_s3_bucket_fails_duplicate_name() {
    new_test_ext().execute_with(|| {
        let provider_pk = setup_provider();
        let terms = primary_terms(1, 100, 500, 1);
        let sig = sign_terms(&provider_pk, &terms);
        assert_ok!(S3Registry::create_s3_bucket(
            RuntimeOrigin::signed(1),
            b"my-bucket".to_vec(),
            3,
            terms,
            sig,
        ));

        // Second attempt uses a fresh nonce so Layer 0 *would* accept it,
        // but the S3 layer rejects the duplicate bucket name first.
        let terms2 = primary_terms(1, 100, 500, 2);
        let sig2 = sign_terms(&provider_pk, &terms2);
        assert_noop!(
            S3Registry::create_s3_bucket(
                RuntimeOrigin::signed(1),
                b"my-bucket".to_vec(),
                3,
                terms2,
                sig2,
            ),
            Error::<Test>::BucketNameExists
        );
    });
}

#[test]
fn delete_s3_bucket_works() {
    new_test_ext().execute_with(|| {
        let s3_bucket_id = setup_provider_and_s3_bucket(1, 1);
        assert_ok!(S3Registry::delete_s3_bucket(
            RuntimeOrigin::signed(1),
            s3_bucket_id
        ));
        assert!(S3Buckets::<Test>::get(s3_bucket_id).is_none());
    });
}

#[test]
fn delete_s3_bucket_fails_not_owner() {
    new_test_ext().execute_with(|| {
        let s3_bucket_id = setup_provider_and_s3_bucket(1, 1);
        assert_noop!(
            S3Registry::delete_s3_bucket(RuntimeOrigin::signed(2), s3_bucket_id),
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
        let s3_bucket_id = setup_provider_and_s3_bucket(1, 1);
        let cid = sp_core::H256::repeat_byte(0xAB);
        assert_ok!(S3Registry::put_object_metadata(
            RuntimeOrigin::signed(1),
            s3_bucket_id,
            b"photos/cat.jpg".to_vec(),
            cid,
            1024,
            b"image/jpeg".to_vec(),
            vec![],
        ));

        let bucket = S3Buckets::<Test>::get(s3_bucket_id).unwrap();
        assert_eq!(bucket.object_count, 1);
        assert_eq!(bucket.total_size, 1024);

        let obj = S3Registry::get_object(s3_bucket_id, b"photos/cat.jpg").unwrap();
        assert_eq!(obj.cid, cid);
        assert_eq!(obj.size, 1024);
    });
}

#[test]
fn delete_object_metadata_works() {
    new_test_ext().execute_with(|| {
        let s3_bucket_id = setup_provider_and_s3_bucket(1, 1);
        let cid = sp_core::H256::repeat_byte(0xAB);
        assert_ok!(S3Registry::put_object_metadata(
            RuntimeOrigin::signed(1),
            s3_bucket_id,
            b"photos/cat.jpg".to_vec(),
            cid,
            1024,
            b"image/jpeg".to_vec(),
            vec![],
        ));

        assert_ok!(S3Registry::delete_object_metadata(
            RuntimeOrigin::signed(1),
            s3_bucket_id,
            b"photos/cat.jpg".to_vec(),
        ));

        let bucket = S3Buckets::<Test>::get(s3_bucket_id).unwrap();
        assert_eq!(bucket.object_count, 0);
        assert_eq!(bucket.total_size, 0);
        assert!(S3Registry::get_object(s3_bucket_id, b"photos/cat.jpg").is_none());
    });
}

#[test]
fn delete_nonempty_bucket_fails() {
    new_test_ext().execute_with(|| {
        let s3_bucket_id = setup_provider_and_s3_bucket(1, 1);
        let cid = sp_core::H256::repeat_byte(0xAB);
        assert_ok!(S3Registry::put_object_metadata(
            RuntimeOrigin::signed(1),
            s3_bucket_id,
            b"file.txt".to_vec(),
            cid,
            100,
            b"text/plain".to_vec(),
            vec![],
        ));

        assert_noop!(
            S3Registry::delete_s3_bucket(RuntimeOrigin::signed(1), s3_bucket_id),
            Error::<Test>::BucketNotEmpty
        );
    });
}
