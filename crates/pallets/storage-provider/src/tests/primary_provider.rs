// SPDX-License-Identifier: Apache-2.0

//! `create_bucket` and `add_primary_provider`: bucket creation and provider
//! assignment as separate operations.

use super::*;
use sp_core::H256;
use storage_primitives::{BucketSnapshot, Commitment, EndAction, Visibility};

/// Signed primary terms for an existing bucket.
fn quote_for(
    provider: u64,
    admin: u64,
    bucket_id: u64,
) -> (crate::AgreementTermsOf<Test>, sp_runtime::MultiSignature) {
    signed_primary_terms(provider, admin, BucketTarget::Existing(bucket_id), 50, 100)
}

#[test]
fn create_bucket_extrinsic_creates_an_empty_bucket() {
    new_test_ext().execute_with(|| {
        assert_ok!(StorageProvider::create_bucket(
            RuntimeOrigin::signed(1),
            2,
            Visibility::Private
        ));

        let bucket = Buckets::<Test>::get(0).unwrap();
        assert!(bucket.primary_providers.is_empty());
        assert_eq!(bucket.min_providers, 2);
        assert_eq!(bucket.visibility, Visibility::Private);
        assert_eq!(bucket.members.len(), 1);
        assert_eq!(bucket.members[0].account, 1);
        assert_eq!(bucket.members[0].role, Role::Admin);
        assert_eq!(MemberBuckets::<Test>::get(1).to_vec(), vec![0]);
    });
}

#[test]
fn create_bucket_rejects_min_providers_above_the_cap() {
    new_test_ext().execute_with(|| {
        // `MaxPrimaryProviders` is 5 in the mock, so 6 could never be met and
        // would leave a bucket that can never be checkpointed.
        assert_noop!(
            StorageProvider::create_bucket(RuntimeOrigin::signed(1), 6, Visibility::Private),
            Error::<Test>::InvalidMinProviders
        );
        assert_eq!(NextBucketId::<Test>::get(), 0);

        // The cap itself is allowed.
        assert_ok!(StorageProvider::create_bucket(
            RuntimeOrigin::signed(1),
            5,
            Visibility::Private
        ));
    });
}

#[test]
fn add_primary_provider_works() {
    new_test_ext().execute_with(|| {
        run_to_block(1);
        register_provider(2, 200);
        let bucket_id = create_bucket(1, 0);

        let (terms, sig) = quote_for(2, 1, bucket_id);
        assert_ok!(StorageProvider::add_primary_provider(
            RuntimeOrigin::signed(1),
            bucket_id,
            2,
            terms,
            sig
        ));

        let bucket = Buckets::<Test>::get(bucket_id).unwrap();
        assert_eq!(bucket.primary_providers.to_vec(), vec![2]);

        let agreement = StorageAgreements::<Test>::get(bucket_id, 2).unwrap();
        assert_eq!(agreement.owner, 1);
        assert_eq!(agreement.max_bytes, 50);
        assert!(matches!(agreement.role, ProviderRole::Primary));

        assert_eq!(Providers::<Test>::get(2).unwrap().committed_bytes, 50);

        System::assert_has_event(
            Event::ProviderAddedToBucket {
                bucket_id,
                provider: 2,
            }
            .into(),
        );
    });
}

#[test]
fn add_primary_provider_rejects_non_admin() {
    new_test_ext().execute_with(|| {
        register_provider(2, 200);
        register_provider(3, 200);
        let bucket_id = setup_agreement(2, 1, 50, 100);

        // Provider 3 has no agreement on the bucket, so the only thing that can
        // reject account 4 here is the admin check.
        let (terms, sig) = quote_for(3, 4, bucket_id);
        assert_noop!(
            StorageProvider::add_primary_provider(
                RuntimeOrigin::signed(4),
                bucket_id,
                3,
                terms,
                sig
            ),
            Error::<Test>::NotBucketAdmin
        );
    });
}

#[test]
fn add_primary_provider_rejects_unknown_bucket() {
    new_test_ext().execute_with(|| {
        register_provider(2, 200);

        let (terms, sig) = quote_for(2, 1, 999);
        assert_noop!(
            StorageProvider::add_primary_provider(RuntimeOrigin::signed(1), 999, 2, terms, sig),
            Error::<Test>::BucketNotFound
        );
    });
}

#[test]
fn add_primary_provider_rejects_quote_for_another_bucket() {
    new_test_ext().execute_with(|| {
        register_provider(2, 200);
        let bucket_id = create_bucket(1, 0);
        let other = create_bucket(1, 0);

        let (terms, sig) = quote_for(2, 1, other);
        assert_noop!(
            StorageProvider::add_primary_provider(
                RuntimeOrigin::signed(1),
                bucket_id,
                2,
                terms,
                sig
            ),
            Error::<Test>::TermsBucketMismatch
        );
    });
}

#[test]
fn add_primary_provider_rejects_new_bucket_quote() {
    new_test_ext().execute_with(|| {
        register_provider(2, 200);
        let bucket_id = create_bucket(1, 0);

        let (terms, sig) = signed_primary_terms(2, 1, BucketTarget::New, 50, 100);
        assert_noop!(
            StorageProvider::add_primary_provider(
                RuntimeOrigin::signed(1),
                bucket_id,
                2,
                terms,
                sig
            ),
            Error::<Test>::TermsBucketMismatch
        );
    });
}

#[test]
fn add_primary_provider_rejects_replica_quote() {
    new_test_ext().execute_with(|| {
        register_provider(2, 200);
        let bucket_id = create_bucket(1, 0);

        let (terms, sig) = signed_replica_terms(
            2,
            1,
            bucket_id,
            50,
            100,
            storage_primitives::ReplicaTerms {
                sync_balance: 100,
                min_sync_interval: 10,
                sync_price: 10,
            },
        );
        assert_noop!(
            StorageProvider::add_primary_provider(
                RuntimeOrigin::signed(1),
                bucket_id,
                2,
                terms,
                sig
            ),
            Error::<Test>::UnexpectedReplicaTerms
        );
    });
}

#[test]
fn create_bucket_with_primary_rejects_replica_quote() {
    new_test_ext().execute_with(|| {
        register_provider(2, 200);

        // Replica params on a primary redemption would leave the sync funding
        // unheld, so the shared primary path rejects them outright.
        let (terms, sig) = signed_replica_terms(
            2,
            1,
            0,
            50,
            100,
            storage_primitives::ReplicaTerms {
                sync_balance: 100,
                min_sync_interval: 10,
                sync_price: 10,
            },
        );
        assert_noop!(
            StorageProvider::create_bucket_with_primary(
                RuntimeOrigin::signed(1),
                2,
                terms,
                sig,
                Visibility::Private
            ),
            Error::<Test>::TermsBucketMismatch
        );

        // Same quote flavour, but naming a new bucket: now only the replica
        // params can reject it.
        let pair = provider_signer(2);
        let mut terms = primary_terms(1, BucketTarget::New, 50, 100, 0);
        terms.replica_params = Some(storage_primitives::ReplicaTerms {
            sync_balance: 100,
            min_sync_interval: 10,
            sync_price: 10,
        });
        let sig = sign_terms(&pair, &terms);
        assert_noop!(
            StorageProvider::create_bucket_with_primary(
                RuntimeOrigin::signed(1),
                2,
                terms,
                sig,
                Visibility::Private
            ),
            Error::<Test>::UnexpectedReplicaTerms
        );

        // Nothing was written on either rejection.
        assert_eq!(NextBucketId::<Test>::get(), 0);
    });
}

#[test]
fn add_primary_provider_rejects_duplicate_agreement() {
    new_test_ext().execute_with(|| {
        register_provider(2, 200);
        let bucket_id = setup_agreement(2, 1, 50, 100);

        let (terms, sig) = quote_for(2, 1, bucket_id);
        assert_noop!(
            StorageProvider::add_primary_provider(
                RuntimeOrigin::signed(1),
                bucket_id,
                2,
                terms,
                sig
            ),
            Error::<Test>::AgreementAlreadyExists
        );
    });
}

#[test]
fn add_primary_provider_rejects_full_primary_set() {
    new_test_ext().execute_with(|| {
        let bucket_id = create_bucket(1, 0);
        // `MaxPrimaryProviders` is 5 in the mock.
        for provider in 2..7u64 {
            register_provider(provider, 200);
            setup_added_primary(provider, 1, bucket_id, 50, 100);
        }

        register_provider(7, 200);
        let (terms, sig) = quote_for(7, 1, bucket_id);
        assert_noop!(
            StorageProvider::add_primary_provider(
                RuntimeOrigin::signed(1),
                bucket_id,
                7,
                terms,
                sig
            ),
            Error::<Test>::MaxPrimaryProvidersReached
        );
    });
}

#[test]
fn add_primary_provider_rejects_replayed_quote() {
    new_test_ext().execute_with(|| {
        register_provider(2, 200);
        let bucket_id = create_bucket(1, 0);

        let (terms, sig) = quote_for(2, 1, bucket_id);
        assert_ok!(StorageProvider::add_primary_provider(
            RuntimeOrigin::signed(1),
            bucket_id,
            2,
            terms.clone(),
            sig
        ));

        // Same nonce, same provider: the replay window rejects it even though
        // the agreement slot is free on a different bucket.
        let other = create_bucket(1, 0);
        let mut replay = terms;
        replay.bucket = BucketTarget::Existing(other);
        let replay_sig = sign_terms(&provider_signer(2), &replay);
        assert_noop!(
            StorageProvider::add_primary_provider(
                RuntimeOrigin::signed(1),
                other,
                2,
                replay,
                replay_sig
            ),
            Error::<Test>::NonceAlreadyUsed
        );
    });
}

#[test]
fn bucket_outlives_its_last_agreement_and_takes_a_new_primary() {
    new_test_ext().execute_with(|| {
        register_provider(2, 200);
        register_provider(3, 200);
        let bucket_id = setup_agreement(2, 1, 50, 100);

        let agreement = StorageAgreements::<Test>::get(bucket_id, 2).unwrap();
        run_to_block(agreement.expires_at + 1);
        assert_ok!(StorageProvider::end_agreement(
            RuntimeOrigin::signed(1),
            bucket_id,
            2,
            EndAction::Pay
        ));

        let bucket = Buckets::<Test>::get(bucket_id).unwrap();
        assert!(bucket.primary_providers.is_empty());

        setup_added_primary(3, 1, bucket_id, 50, 100);

        let bucket = Buckets::<Test>::get(bucket_id).unwrap();
        assert_eq!(bucket.primary_providers.to_vec(), vec![3]);
    });
}

#[test]
fn add_primary_provider_appends_without_disturbing_snapshot_bits() {
    new_test_ext().execute_with(|| {
        register_provider(2, 200);
        register_provider(3, 200);
        let bucket_id = setup_agreement(2, 1, 50, 100);

        Buckets::<Test>::mutate(bucket_id, |maybe_bucket| {
            if let Some(bucket) = maybe_bucket {
                bucket.snapshot = Some(BucketSnapshot {
                    commitment: Commitment {
                        mmr_root: H256::repeat_byte(0xAB),
                        start_seq: 0,
                        leaf_count: 10,
                    },
                    checkpoint_block: 1,
                    primary_signers: vec![0x01],
                });
            }
        });

        setup_added_primary(3, 1, bucket_id, 50, 100);

        let bucket = Buckets::<Test>::get(bucket_id).unwrap();
        // The new primary is appended, so the signer at index 0 keeps its bit
        // and the newcomer's bit stays clear until it signs a checkpoint.
        assert_eq!(bucket.primary_providers.to_vec(), vec![2, 3]);
        assert_eq!(bucket.snapshot.unwrap().primary_signers, vec![0x01]);
    });
}

#[test]
fn add_primary_provider_works_on_a_frozen_bucket() {
    new_test_ext().execute_with(|| {
        register_provider(2, 200);
        register_provider(3, 200);
        let bucket_id = setup_agreement(2, 1, 50, 100);

        Buckets::<Test>::mutate(bucket_id, |maybe_bucket| {
            if let Some(bucket) = maybe_bucket {
                bucket.frozen_start_seq = Some(0);
            }
        });

        setup_added_primary(3, 1, bucket_id, 50, 100);

        assert!(StorageAgreements::<Test>::get(bucket_id, 3).is_some());
    });
}
