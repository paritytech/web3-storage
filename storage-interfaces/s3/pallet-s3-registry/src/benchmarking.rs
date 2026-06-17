// SPDX-License-Identifier: Apache-2.0

//! Benchmarking setup for `pallet-s3-registry`.
//!
//! Each benchmark configures the pallet to exercise the most expensive
//! control flow inside the extrinsic so the generated weights are upper
//! bounds:
//!
//! * `create_s3_bucket` — max-length bucket name and `UserBuckets`
//!   pre-filled to `MaxBucketsPerUser - 1` so the bounded `try_push` runs
//!   at the capacity boundary.
//! * `delete_s3_bucket` — `UserBuckets` is at `MaxBucketsPerUser` with the
//!   target id last so `retain` scans every prior entry; bucket name is at
//!   max length to maximise the `BucketNameToId` remove.
//! * `put_object_metadata` — update path with max-length key, content type,
//!   ETag and 16 user-metadata entries at maximum key/value lengths, so
//!   both the decode of the existing metadata and the encode of the new
//!   one are at maximum size.
//! * `delete_object_metadata` — target object has the full max-size
//!   metadata payload (decode cost), key at max length.
//! * `copy_object_metadata` — distinct src and dst buckets, both keys at
//!   max length, source object carries max-size metadata, dst hits the
//!   new-object branch with `object_count` at `MaxObjectsPerBucket - 1`.

use super::*;
use alloc::{vec, vec::Vec};
use frame_benchmarking::v2::*;
use frame_support::{
    traits::{Currency, Get},
    BoundedVec,
};
use frame_system::{pallet_prelude::BlockNumberFor, RawOrigin};
use pallet_storage_provider::{AgreementTermsOf, Pallet as StorageProvider, ProviderSettings};
use s3_primitives::{
    BucketName, MaxContentTypeLen, MaxEtagLen, MaxMetadataEntries, MaxMetadataKeyLen,
    MaxMetadataValueLen, MaxObjectKeyLen, MetadataEntry, ObjectKey, ObjectMetadata, S3BucketId,
    S3BucketInfo,
};
use sp_core::H256;
use sp_runtime::traits::{Bounded, SaturatedConversion};
use storage_primitives::AgreementTerms;

const SEED: u32 = 0;

/// Key type used by the benchmarking keystore for provider signing material.
const KEY_TYPE: sp_core::crypto::KeyTypeId = sp_core::crypto::KeyTypeId(*b"s3bn");

/// Account with effectively unbounded balance.
fn funded_account<T: Config>(name: &'static str, index: u32) -> T::AccountId {
    let acc: T::AccountId = account(name, index, SEED);
    let amount = BalanceOf::<T>::max_value() / 2u32.into();
    let _ = <T as pallet_storage_provider::Config>::Currency::make_free_balance_be(&acc, amount);
    acc
}

/// Register a storage provider that accepts primary agreements with enough
/// stake/capacity to back the benchmarks that open agreements. Returns both
/// the account and the sr25519 public key registered against it so the
/// `create_s3_bucket` benchmark can sign terms with the matching keystore key.
fn create_provider<T: Config>(index: u32) -> (T::AccountId, sp_core::sr25519::Public) {
    let provider = funded_account::<T>("provider", index);
    let multiaddr = b"/ip4/127.0.0.1/tcp/3000".to_vec();

    // Generate an sr25519 key in the runtime keystore so the pallet can
    // verify signatures over agreement terms.
    let seed = alloc::format!("//S3BenchProvider{index}");
    let public_key = sp_io::crypto::sr25519_generate(KEY_TYPE, Some(seed.into_bytes()));

    let capacity: u64 = 1_000_000_000;
    let capacity_balance: BalanceOf<T> = capacity.saturated_into();
    let required_for_capacity = T::MinStakePerByte::get() * capacity_balance;
    let stake = T::MinProviderStake::get().max(required_for_capacity);

    let _ = StorageProvider::<T>::register_provider(
        RawOrigin::Signed(provider.clone()).into(),
        multiaddr.try_into().unwrap(),
        public_key.0.to_vec().try_into().unwrap(),
        stake,
    );

    let _ = StorageProvider::<T>::update_provider_settings(
        RawOrigin::Signed(provider.clone()).into(),
        ProviderSettings {
            min_duration: 1u32.into(),
            max_duration: 1_000_000u32.into(),
            price_per_byte: 1u32.into(),
            accepting_primary: true,
            replica_sync_price: Some(1u32.into()),
            accepting_extensions: true,
            max_capacity: capacity,
        },
    );

    (provider, public_key)
}

/// Sign agreement terms with the provider's keystore key.
fn sign_terms<T: Config>(
    public_key: &sp_core::sr25519::Public,
    terms: &AgreementTermsOf<T>,
) -> sp_runtime::MultiSignature {
    let hash = sp_io::hashing::blake2_256(&terms.signing_payload());
    let sig = sp_io::crypto::sr25519_sign(KEY_TYPE, public_key, &hash)
        .expect("benchmarking keystore signs with a key it generated");
    sp_runtime::MultiSignature::Sr25519(sig)
}

/// Generate a valid S3 bucket name of `len` bytes (3 ≤ len ≤ 63), made of
/// `'a'` filler with a hyphen mixed in so the result is still
/// `validate_bucket_name`-compatible (lowercase / digit / hyphen, with
/// first/last lowercase). Each `salt` produces a distinct name when
/// `len >= 10` by overwriting some interior bytes with the salt's hex.
fn make_bucket_name(len: usize, salt: u32) -> Vec<u8> {
    let len = len.clamp(3, 63);
    let mut name = vec![b'a'; len];
    let salt_hex = alloc::format!("{salt:08x}");
    let salt_bytes = salt_hex.as_bytes();
    // Place the salt in the middle (not first/last) so validation still passes.
    if len >= salt_bytes.len() + 2 {
        let start = 1;
        name[start..start + salt_bytes.len()].copy_from_slice(salt_bytes);
    }
    name
}

/// Build a maximum-size `ObjectMetadata` payload.
fn max_object_metadata() -> ObjectMetadata {
    let content_type: BoundedVec<u8, MaxContentTypeLen> =
        vec![b'c'; MaxContentTypeLen::get() as usize]
            .try_into()
            .unwrap();
    let etag: BoundedVec<u8, MaxEtagLen> =
        vec![b'e'; MaxEtagLen::get() as usize].try_into().unwrap();

    let entry_count = MaxMetadataEntries::get() as usize;
    let mut entries: Vec<MetadataEntry> = Vec::with_capacity(entry_count);
    for i in 0..entry_count {
        // Keys must be distinct — vary the last byte by the index.
        let mut k = vec![b'k'; MaxMetadataKeyLen::get() as usize];
        let last = k.len() - 1;
        k[last] = b'a' + (i as u8 % 26);
        let key: BoundedVec<u8, MaxMetadataKeyLen> = k.try_into().unwrap();
        let value: BoundedVec<u8, MaxMetadataValueLen> =
            vec![b'v'; MaxMetadataValueLen::get() as usize]
                .try_into()
                .unwrap();
        entries.push(MetadataEntry { key, value });
    }
    let user_metadata: BoundedVec<MetadataEntry, MaxMetadataEntries> = entries.try_into().unwrap();

    ObjectMetadata {
        cid: H256::repeat_byte(0xAB),
        size: 1_000_000,
        last_modified: 0,
        content_type,
        etag,
        user_metadata,
    }
}

/// Max-size object key.
fn max_object_key() -> Vec<u8> {
    vec![b'x'; MaxObjectKeyLen::get() as usize]
}

/// Max-size object key as a `BoundedVec`.
fn max_object_key_bounded() -> ObjectKey {
    max_object_key().try_into().unwrap()
}

/// Insert an S3 bucket directly into storage without going through
/// `create_s3_bucket` (avoids the Layer 0 bucket setup cost).
fn insert_s3_bucket<T: Config>(
    owner: &T::AccountId,
    s3_bucket_id: S3BucketId,
    bucket_name: &[u8],
    object_count: u64,
) {
    let bounded_name: BucketName = bucket_name.to_vec().try_into().unwrap();
    let info: S3BucketInfoOf<T> = S3BucketInfo {
        s3_bucket_id,
        name: bounded_name.clone(),
        layer0_bucket_id: 0,
        owner: owner.clone(),
        created_at: frame_system::Pallet::<T>::block_number(),
        object_count,
        total_size: 0,
    };
    S3Buckets::<T>::insert(s3_bucket_id, info);
    BucketNameToId::<T>::insert(&bounded_name, s3_bucket_id);
}

/// Pre-fill `UserBuckets` for `user` with `count` synthetic bucket IDs.
fn prefill_user_buckets<T: Config>(user: &T::AccountId, count: u32) {
    let ids: Vec<S3BucketId> = (10_000..10_000u64 + count as u64).collect();
    let bounded: BoundedVec<S3BucketId, T::MaxBucketsPerUser> = ids.try_into().unwrap();
    UserBuckets::<T>::insert(user, bounded);
}

#[benchmarks]
mod benchmarks {
    use super::*;

    // ─────────────────────────────────────────────────────────────────────────
    // Bucket lifecycle
    // ─────────────────────────────────────────────────────────────────────────

    /// Worst case:
    /// - Provider registered with full max-capacity stake so the Layer 0
    ///   signature / replay / capacity / stake / duration / price checks
    ///   all run.
    /// - Name is at the max 63 bytes that `validate_bucket_name` accepts.
    /// - `UserBuckets` for the caller is pre-filled to `MaxBucketsPerUser - 1`
    ///   so the bounded `try_push` runs right at the capacity boundary.
    #[benchmark]
    fn create_s3_bucket() {
        let (provider, provider_pk) = create_provider::<T>(0);

        let user = funded_account::<T>("user", 0);
        let cap = T::MaxBucketsPerUser::get();
        if cap > 1 {
            prefill_user_buckets::<T>(&user, cap - 1);
        }

        let name = make_bucket_name(63, 0);
        let max_bytes: u64 = 1_000;
        let duration: BlockNumberFor<T> = 100u32.into();
        let terms: AgreementTermsOf<T> = AgreementTerms {
            owner: user.clone(),
            max_bytes,
            duration,
            price_per_byte: 1u32.into(),
            valid_until: BlockNumberFor::<T>::max_value(),
            nonce: 1,
            bucket_id: None,
            replica_params: None,
        };
        let sig = sign_terms::<T>(&provider_pk, &terms);

        #[extrinsic_call]
        create_s3_bucket(RawOrigin::Signed(user), name, provider, terms, sig);
    }

    /// Worst case:
    /// - `UserBuckets` is at `MaxBucketsPerUser` with the target ID inserted
    ///   last, so `retain` walks every other entry before removing it.
    /// - The bucket name being removed from `BucketNameToId` is at max length.
    #[benchmark]
    fn delete_s3_bucket() {
        let user = funded_account::<T>("user", 0);
        let name = make_bucket_name(63, 1);
        let s3_bucket_id: S3BucketId = 42;
        insert_s3_bucket::<T>(&user, s3_bucket_id, &name, 0);

        // Fill UserBuckets to MaxBucketsPerUser with the target id last.
        let cap = T::MaxBucketsPerUser::get();
        if cap > 1 {
            let mut ids: Vec<S3BucketId> = (10_000..10_000u64 + (cap - 1) as u64).collect();
            ids.push(s3_bucket_id);
            let bounded: BoundedVec<S3BucketId, T::MaxBucketsPerUser> = ids.try_into().unwrap();
            UserBuckets::<T>::insert(&user, bounded);
        } else {
            let bounded: BoundedVec<S3BucketId, T::MaxBucketsPerUser> =
                vec![s3_bucket_id].try_into().unwrap();
            UserBuckets::<T>::insert(&user, bounded);
        }

        #[extrinsic_call]
        delete_s3_bucket(RawOrigin::Signed(user), s3_bucket_id);
    }

    // ─────────────────────────────────────────────────────────────────────────
    // Object metadata
    // ─────────────────────────────────────────────────────────────────────────

    /// Worst case: the *update* branch is taken with max-size old metadata
    /// (full decode) and max-size new metadata (full encode + write). Key,
    /// content type, ETag and all 16 user-metadata entries are at their
    /// upper bounds.
    #[benchmark]
    fn put_object_metadata() {
        let user = funded_account::<T>("user", 0);
        let s3_bucket_id: S3BucketId = 42;
        let bucket_name = make_bucket_name(63, 2);
        insert_s3_bucket::<T>(&user, s3_bucket_id, &bucket_name, 1);

        // Pre-insert an object with full-size metadata at the target key.
        let key_bounded = max_object_key_bounded();
        Objects::<T>::insert(s3_bucket_id, &key_bounded, max_object_metadata());

        // Build the max-size input arguments for the call itself.
        let key = max_object_key();
        let content_type = vec![b'c'; MaxContentTypeLen::get() as usize];
        let metadata_pairs: Vec<(Vec<u8>, Vec<u8>)> = (0..MaxMetadataEntries::get())
            .map(|i| {
                let mut k = vec![b'k'; MaxMetadataKeyLen::get() as usize];
                let last = k.len() - 1;
                k[last] = b'a' + (i as u8 % 26);
                let v = vec![b'v'; MaxMetadataValueLen::get() as usize];
                (k, v)
            })
            .collect();

        #[extrinsic_call]
        put_object_metadata(
            RawOrigin::Signed(user),
            s3_bucket_id,
            key,
            H256::repeat_byte(0xCD),
            2_000_000,
            content_type,
            metadata_pairs,
        );
    }

    /// Worst case: stored object carries the full max-size metadata payload,
    /// so the `Objects::take` decode is at its maximum cost. The key being
    /// deleted is also at max length.
    #[benchmark]
    fn delete_object_metadata() {
        let user = funded_account::<T>("user", 0);
        let s3_bucket_id: S3BucketId = 42;
        let bucket_name = make_bucket_name(63, 3);
        insert_s3_bucket::<T>(&user, s3_bucket_id, &bucket_name, 1);

        let key_bounded = max_object_key_bounded();
        Objects::<T>::insert(s3_bucket_id, &key_bounded, max_object_metadata());

        let key = max_object_key();

        #[extrinsic_call]
        delete_object_metadata(RawOrigin::Signed(user), s3_bucket_id, key);
    }

    /// Worst case:
    /// - Different src and dst buckets (two `S3Buckets` reads + 1 write).
    /// - Both keys at max length.
    /// - Source object carries max-size metadata (full decode + re-encode
    ///   at the destination).
    /// - Destination hits the *new-object* branch with `object_count` at
    ///   `MaxObjectsPerBucket - 1` so the bounded counter check matters.
    #[benchmark]
    fn copy_object_metadata() {
        let user = funded_account::<T>("user", 0);

        let src_bucket_id: S3BucketId = 42;
        let dst_bucket_id: S3BucketId = 43;
        let src_name = make_bucket_name(63, 4);
        let dst_name = make_bucket_name(63, 5);
        insert_s3_bucket::<T>(&user, src_bucket_id, &src_name, 1);

        // Destination is one object away from its bucket-level cap to exercise
        // the boundary check before the increment.
        let dst_initial = T::MaxObjectsPerBucket::get().saturating_sub(1) as u64;
        insert_s3_bucket::<T>(&user, dst_bucket_id, &dst_name, dst_initial);

        let src_key_bounded = max_object_key_bounded();
        Objects::<T>::insert(src_bucket_id, &src_key_bounded, max_object_metadata());

        let src_key = max_object_key();
        // Dst key must be distinct enough that the dst lookup hits the
        // `None` (new object) branch — flip the first byte.
        let mut dst_key = max_object_key();
        dst_key[0] = b'd';

        #[extrinsic_call]
        copy_object_metadata(
            RawOrigin::Signed(user),
            src_bucket_id,
            src_key,
            dst_bucket_id,
            dst_key,
        );
    }

    impl_benchmark_test_suite!(Pallet, crate::mock::new_test_ext(), crate::mock::Test);
}
