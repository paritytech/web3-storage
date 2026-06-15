//! Tests for S3 Registry pallet.

use crate::{mock::*, Error, S3Buckets};
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
    let hash = sp_io::hashing::blake2_256(&terms.signing_payload());
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
        price_per_byte: 1u128,
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

// ────────────────────────────────────────────────────────────────────────────
// Helpers shared by the copy / query tests below
// ────────────────────────────────────────────────────────────────────────────

/// Create an S3 bucket for `owner` using an already-registered provider at
/// account 3. `nonce` must be unique per (provider, terms-hash) pair.
fn create_bucket(owner: u64, name: &[u8], nonce: u64, pk: &sp_core::sr25519::Public) -> u64 {
    let terms = primary_terms(owner, 100, 500, nonce);
    let sig = sign_terms(pk, &terms);
    assert_ok!(S3Registry::create_s3_bucket(
        RuntimeOrigin::signed(owner),
        name.to_vec(),
        3,
        terms,
        sig,
    ));
    S3Registry::next_s3_bucket_id() - 1
}

/// Store a small test object at `key` in `bucket_id` owned by `owner`.
fn put_test_object(owner: u64, bucket_id: u64, key: &[u8], size: u64) {
    assert_ok!(S3Registry::put_object_metadata(
        RuntimeOrigin::signed(owner),
        bucket_id,
        key.to_vec(),
        sp_core::H256::repeat_byte(0xAB),
        size,
        b"application/octet-stream".to_vec(),
        vec![],
    ));
}

// ────────────────────────────────────────────────────────────────────────────
// T1 — copy_object_metadata
// ────────────────────────────────────────────────────────────────────────────

#[test]
fn copy_object_metadata_works() {
    new_test_ext().execute_with(|| {
        let pk = setup_provider();
        let src = create_bucket(1, b"src-bucket", 1, &pk);
        let dst = create_bucket(1, b"dst-bucket", 2, &pk);
        put_test_object(1, src, b"file.txt", 512);

        assert_ok!(S3Registry::copy_object_metadata(
            RuntimeOrigin::signed(1),
            src,
            b"file.txt".to_vec(),
            dst,
            b"copy.txt".to_vec(),
        ));

        // Destination has the object with the original size.
        let obj = S3Registry::get_object(dst, b"copy.txt").unwrap();
        assert_eq!(obj.size, 512);
        let dst_info = S3Buckets::<Test>::get(dst).unwrap();
        assert_eq!(dst_info.object_count, 1);
        assert_eq!(dst_info.total_size, 512);

        System::assert_has_event(RuntimeEvent::S3Registry(crate::Event::ObjectCopied {
            src_bucket_id: src,
            src_key: b"file.txt".to_vec(),
            dst_bucket_id: dst,
            dst_key: b"copy.txt".to_vec(),
        }));
    });
}

#[test]
fn copy_object_metadata_within_same_bucket() {
    new_test_ext().execute_with(|| {
        let pk = setup_provider();
        let bucket = create_bucket(1, b"my-bucket", 1, &pk);
        put_test_object(1, bucket, b"original.txt", 256);

        assert_ok!(S3Registry::copy_object_metadata(
            RuntimeOrigin::signed(1),
            bucket,
            b"original.txt".to_vec(),
            bucket,
            b"duplicate.txt".to_vec(),
        ));

        assert!(S3Registry::get_object(bucket, b"duplicate.txt").is_some());
        // Both keys exist in the same bucket.
        let info = S3Buckets::<Test>::get(bucket).unwrap();
        assert_eq!(info.object_count, 2);
        assert_eq!(info.total_size, 512); // 256 original + 256 copy
    });
}

#[test]
fn copy_object_metadata_overwrites_existing_dst() {
    // Copying onto an existing dst key: old size is removed, new size added.
    new_test_ext().execute_with(|| {
        let pk = setup_provider();
        let src = create_bucket(1, b"src-bucket", 1, &pk);
        let dst = create_bucket(1, b"dst-bucket", 2, &pk);
        put_test_object(1, src, b"a.txt", 100);
        put_test_object(1, dst, b"b.txt", 400); // existing dst object

        assert_ok!(S3Registry::copy_object_metadata(
            RuntimeOrigin::signed(1),
            src,
            b"a.txt".to_vec(),
            dst,
            b"b.txt".to_vec(), // overwrite
        ));

        let info = S3Buckets::<Test>::get(dst).unwrap();
        // Count unchanged (overwrite, not new), size replaced: 400 → 100.
        assert_eq!(info.object_count, 1);
        assert_eq!(info.total_size, 100);
    });
}

#[test]
fn copy_object_metadata_fails_src_bucket_not_found() {
    new_test_ext().execute_with(|| {
        let pk = setup_provider();
        let dst = create_bucket(1, b"dst-bucket", 1, &pk);
        assert_noop!(
            S3Registry::copy_object_metadata(
                RuntimeOrigin::signed(1),
                999,
                b"file.txt".to_vec(),
                dst,
                b"copy.txt".to_vec(),
            ),
            Error::<Test>::BucketNotFound
        );
    });
}

#[test]
fn copy_object_metadata_fails_not_owner_of_src() {
    // User 2 owns the src bucket; user 1 cannot copy from it.
    new_test_ext().execute_with(|| {
        let pk = setup_provider();
        let src = create_bucket(2, b"src-bucket", 10, &pk);
        let dst = create_bucket(1, b"dst-bucket", 1, &pk);
        put_test_object(2, src, b"file.txt", 100);
        assert_noop!(
            S3Registry::copy_object_metadata(
                RuntimeOrigin::signed(1),
                src,
                b"file.txt".to_vec(),
                dst,
                b"copy.txt".to_vec(),
            ),
            Error::<Test>::NotBucketOwner
        );
    });
}

#[test]
fn copy_object_metadata_fails_dst_bucket_not_found() {
    new_test_ext().execute_with(|| {
        let pk = setup_provider();
        let src = create_bucket(1, b"src-bucket", 1, &pk);
        put_test_object(1, src, b"file.txt", 100);
        assert_noop!(
            S3Registry::copy_object_metadata(
                RuntimeOrigin::signed(1),
                src,
                b"file.txt".to_vec(),
                999,
                b"copy.txt".to_vec(),
            ),
            Error::<Test>::BucketNotFound
        );
    });
}

#[test]
fn copy_object_metadata_fails_not_owner_of_dst() {
    // User 1 owns src but user 2 owns dst; the dst ownership check must fail.
    new_test_ext().execute_with(|| {
        let pk = setup_provider();
        let src = create_bucket(1, b"src-bucket", 1, &pk);
        let dst = create_bucket(2, b"dst-bucket", 10, &pk);
        put_test_object(1, src, b"file.txt", 100);
        assert_noop!(
            S3Registry::copy_object_metadata(
                RuntimeOrigin::signed(1),
                src,
                b"file.txt".to_vec(),
                dst,
                b"copy.txt".to_vec(),
            ),
            Error::<Test>::NotBucketOwner
        );
    });
}

#[test]
fn copy_object_metadata_fails_src_key_too_long() {
    new_test_ext().execute_with(|| {
        let pk = setup_provider();
        let src = create_bucket(1, b"src-bucket", 1, &pk);
        let dst = create_bucket(1, b"dst-bucket", 2, &pk);
        let long_key = vec![b'a'; 1025]; // exceeds ObjectKey bound
        assert_noop!(
            S3Registry::copy_object_metadata(
                RuntimeOrigin::signed(1),
                src,
                long_key,
                dst,
                b"copy.txt".to_vec(),
            ),
            Error::<Test>::ObjectKeyTooLong
        );
    });
}

#[test]
fn copy_object_metadata_fails_dst_key_too_long() {
    new_test_ext().execute_with(|| {
        let pk = setup_provider();
        let src = create_bucket(1, b"src-bucket", 1, &pk);
        let dst = create_bucket(1, b"dst-bucket", 2, &pk);
        put_test_object(1, src, b"file.txt", 100);
        let long_key = vec![b'a'; 1025]; // exceeds ObjectKey bound
        assert_noop!(
            S3Registry::copy_object_metadata(
                RuntimeOrigin::signed(1),
                src,
                b"file.txt".to_vec(),
                dst,
                long_key,
            ),
            Error::<Test>::ObjectKeyTooLong
        );
    });
}

#[test]
fn copy_object_metadata_fails_object_not_found() {
    new_test_ext().execute_with(|| {
        let pk = setup_provider();
        let src = create_bucket(1, b"src-bucket", 1, &pk);
        let dst = create_bucket(1, b"dst-bucket", 2, &pk);
        // No object put in src.
        assert_noop!(
            S3Registry::copy_object_metadata(
                RuntimeOrigin::signed(1),
                src,
                b"missing.txt".to_vec(),
                dst,
                b"copy.txt".to_vec(),
            ),
            Error::<Test>::ObjectNotFound
        );
    });
}

// ────────────────────────────────────────────────────────────────────────────
// T2 — query helpers and uncovered put/delete branches
// ────────────────────────────────────────────────────────────────────────────

#[test]
fn get_bucket_by_name_found() {
    new_test_ext().execute_with(|| {
        setup_provider_and_s3_bucket(1, 1);
        let info = S3Registry::get_bucket_by_name(b"my-bucket").unwrap();
        assert_eq!(info.owner, 1);
    });
}

#[test]
fn get_bucket_by_name_not_found() {
    new_test_ext().execute_with(|| {
        assert!(S3Registry::get_bucket_by_name(b"no-such-bucket").is_none());
    });
}

#[test]
fn is_bucket_owner_returns_true_for_owner() {
    new_test_ext().execute_with(|| {
        let id = setup_provider_and_s3_bucket(1, 1);
        assert!(S3Registry::is_bucket_owner(id, &1u64));
    });
}

#[test]
fn is_bucket_owner_returns_false_for_non_owner() {
    new_test_ext().execute_with(|| {
        let id = setup_provider_and_s3_bucket(1, 1);
        assert!(!S3Registry::is_bucket_owner(id, &2u64));
    });
}

#[test]
fn is_bucket_owner_returns_false_for_missing_bucket() {
    new_test_ext().execute_with(|| {
        assert!(!S3Registry::is_bucket_owner(999, &1u64));
    });
}

#[test]
fn get_layer0_bucket_id_returns_id() {
    new_test_ext().execute_with(|| {
        let id = setup_provider_and_s3_bucket(1, 1);
        // The Layer 0 bucket ID is assigned by the storage-provider pallet
        // (starts at 0); just confirm it is Some.
        assert!(S3Registry::get_layer0_bucket_id(id).is_some());
    });
}

#[test]
fn get_layer0_bucket_id_returns_none_for_unknown() {
    new_test_ext().execute_with(|| {
        assert!(S3Registry::get_layer0_bucket_id(999).is_none());
    });
}

#[test]
fn put_object_metadata_updates_existing_object() {
    // Putting the same key twice: old size is removed, new size added; count stays 1.
    new_test_ext().execute_with(|| {
        let id = setup_provider_and_s3_bucket(1, 1);
        put_test_object(1, id, b"data.bin", 1000);

        let cid2 = sp_core::H256::repeat_byte(0xCD);
        assert_ok!(S3Registry::put_object_metadata(
            RuntimeOrigin::signed(1),
            id,
            b"data.bin".to_vec(),
            cid2,
            200,
            b"application/octet-stream".to_vec(),
            vec![],
        ));

        let info = S3Buckets::<Test>::get(id).unwrap();
        assert_eq!(info.object_count, 1);
        assert_eq!(info.total_size, 200); // 1000 removed, 200 added
        let obj = S3Registry::get_object(id, b"data.bin").unwrap();
        assert_eq!(obj.cid, cid2);
    });
}

#[test]
fn put_object_metadata_stores_user_metadata() {
    // Non-empty user_metadata exercises the filter_map conversion path.
    new_test_ext().execute_with(|| {
        let id = setup_provider_and_s3_bucket(1, 1);
        assert_ok!(S3Registry::put_object_metadata(
            RuntimeOrigin::signed(1),
            id,
            b"tagged.txt".to_vec(),
            sp_core::H256::repeat_byte(0x01),
            64,
            b"text/plain".to_vec(),
            vec![
                (b"author".to_vec(), b"alice".to_vec()),
                (b"project".to_vec(), b"web3".to_vec()),
            ],
        ));
        let obj = S3Registry::get_object(id, b"tagged.txt").unwrap();
        assert_eq!(obj.user_metadata.len(), 2);
    });
}
