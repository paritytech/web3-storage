// SPDX-License-Identifier: Apache-2.0

//! `create_bucket` and `add_primary_provider`: bucket creation and provider
//! assignment as separate operations.

use super::*;
use frame_support::traits::Get;
use sp_core::H256;
use storage_primitives::{ChunkLocation, Commitment, EndAction, Visibility};

/// Signed primary terms for an existing bucket. Build the quote before
/// `assert_noop!`: `provider_signer` writes the provider's key into storage.
fn quote_for(
    provider: u64,
    admin: u64,
    bucket_id: u64,
) -> (crate::AgreementTermsOf<Test>, sp_runtime::MultiSignature) {
    signed_primary_terms(provider, admin, BucketTarget::Existing(bucket_id), 50, 100)
}

/// `add_primary_provider` signed by `admin`.
fn add_primary(
    admin: u64,
    bucket_id: u64,
    provider: u64,
    (terms, sig): (crate::AgreementTermsOf<Test>, sp_runtime::MultiSignature),
) -> frame_support::dispatch::DispatchResult {
    StorageProvider::add_primary_provider(
        RuntimeOrigin::signed(admin),
        bucket_id,
        provider,
        terms,
        sig,
    )
}

#[test]
fn create_bucket_rejects_min_providers_above_the_cap() {
    new_test_ext().execute_with(|| {
        let cap = <Test as Config>::MaxPrimaryProviders::get();
        assert_noop!(
            StorageProvider::create_bucket(RuntimeOrigin::signed(1), cap + 1, Visibility::Private),
            Error::<Test>::InvalidMinProviders
        );

        // The cap itself is allowed.
        assert_ok!(StorageProvider::create_bucket(
            RuntimeOrigin::signed(1),
            cap,
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

        setup_added_primary(2, 1, bucket_id, 50, 100);

        let bucket = Buckets::<Test>::get(bucket_id).unwrap();
        assert_eq!(bucket.primary_providers.to_vec(), vec![2]);

        let agreement = StorageAgreements::<Test>::get(bucket_id, 2).unwrap();
        assert_eq!(agreement.owner, 1);
        assert_eq!(agreement.max_bytes, 50);
        assert!(matches!(agreement.role, ProviderRole::Primary));

        assert_eq!(Providers::<Test>::get(2).unwrap().committed_bytes, 50);

        // `StorageAgreementEstablished` follows `ProviderAddedToBucket`.
        let added =
            event_position(|e| matches!(e, Event::ProviderAddedToBucket { provider: 2, .. }));
        let established =
            event_position(|e| matches!(e, Event::StorageAgreementEstablished { provider: 2, .. }));
        assert!(added < established);
    });
}

#[test]
fn add_primary_provider_internal_writes_nothing_for_a_rejected_quote() {
    new_test_ext().execute_with(|| {
        register_provider(2, 200);
        register_provider_with_settings(
            3,
            200,
            ProviderSettings {
                accepting_primary: false,
                ..Default::default()
            },
        );
        let bucket_id = setup_agreement(2, 1, 50, 100);

        let (terms, sig) = quote_for(3, 1, bucket_id);
        assert_err!(
            StorageProvider::add_primary_provider_internal(&1, bucket_id, &3, terms, &sig),
            Error::<Test>::ProviderNotAcceptingPrimary
        );
        let bucket = Buckets::<Test>::get(bucket_id).unwrap();
        assert_eq!(bucket.primary_providers.to_vec(), vec![2]);
        assert!(StorageAgreements::<Test>::get(bucket_id, 3).is_none());
    });
}

#[test]
fn add_primary_provider_rejects_non_admin() {
    new_test_ext().execute_with(|| {
        register_provider(2, 200);
        register_provider(3, 200);
        let bucket_id = setup_agreement(2, 1, 50, 100);

        let quote = quote_for(3, 4, bucket_id);
        assert_noop!(
            add_primary(4, bucket_id, 3, quote),
            Error::<Test>::NotBucketAdmin
        );
    });
}

#[test]
fn add_primary_provider_rejects_unknown_bucket() {
    new_test_ext().execute_with(|| {
        register_provider(2, 200);

        let quote = quote_for(2, 1, 999);

        assert_noop!(add_primary(1, 999, 2, quote), Error::<Test>::BucketNotFound);
    });
}

#[test]
fn add_primary_provider_rejects_quote_for_another_bucket() {
    new_test_ext().execute_with(|| {
        register_provider(2, 200);
        let bucket_id = create_bucket(1, 0);
        let other = create_bucket(1, 0);

        let quote = quote_for(2, 1, other);

        assert_noop!(
            add_primary(1, bucket_id, 2, quote),
            Error::<Test>::TermsBucketMismatch
        );
    });
}

#[test]
fn add_primary_provider_rejects_new_bucket_quote() {
    new_test_ext().execute_with(|| {
        register_provider(2, 200);
        let bucket_id = create_bucket(1, 0);

        let quote = signed_primary_terms(2, 1, BucketTarget::New, 50, 100);
        assert_noop!(
            add_primary(1, bucket_id, 2, quote),
            Error::<Test>::TermsBucketMismatch
        );
    });
}

#[test]
fn add_primary_provider_rejects_replica_quote() {
    new_test_ext().execute_with(|| {
        register_provider(2, 200);
        let bucket_id = create_bucket(1, 0);

        let quote = signed_replica_terms(2, 1, bucket_id, 50, 100, replica_params());
        assert_noop!(
            add_primary(1, bucket_id, 2, quote),
            Error::<Test>::UnexpectedReplicaTerms
        );
    });
}

#[test]
fn create_bucket_with_primary_rejects_replica_quote() {
    new_test_ext().execute_with(|| {
        register_provider(2, 200);

        // Replica params on a primary redemption would leave the sync funding
        // unheld, so the shared primary path rejects them.
        let pair = provider_signer(2);
        let mut terms = primary_terms(1, BucketTarget::New, 50, 100, 0);
        terms.replica_params = Some(replica_params());
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
    });
}

#[test]
fn add_primary_provider_rejects_duplicate_agreement() {
    new_test_ext().execute_with(|| {
        register_provider(2, 200);
        let bucket_id = setup_agreement(2, 1, 50, 100);

        let quote = quote_for(2, 1, bucket_id);

        assert_noop!(
            add_primary(1, bucket_id, 2, quote),
            Error::<Test>::AgreementAlreadyExists
        );
    });
}

#[test]
fn add_primary_provider_rejects_full_primary_set() {
    new_test_ext().execute_with(|| {
        let bucket_id = create_bucket(1, 0);
        let cap = u64::from(<<Test as Config>::MaxPrimaryProviders as Get<u32>>::get());
        for provider in 2..2 + cap {
            register_provider(provider, 200);
            setup_added_primary(provider, 1, bucket_id, 50, 100);
        }

        let extra = 2 + cap;
        register_provider(extra, 200);
        let quote = quote_for(extra, 1, bucket_id);
        assert_noop!(
            add_primary(1, bucket_id, extra, quote),
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
        assert_ok!(add_primary(1, bucket_id, 2, (terms.clone(), sig)));

        // Same nonce: the owner's nonce has advanced past it, so it is
        // rejected even though the agreement slot is free on a different
        // bucket.
        let other = create_bucket(1, 0);
        let mut replay = terms;
        replay.bucket = BucketTarget::Existing(other);
        let replay_sig = sign_terms(&provider_signer(2), &replay);
        assert_noop!(
            add_primary(1, other, 2, (replay, replay_sig)),
            Error::<Test>::NonceMismatch
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
        register_provider(3, 200);
        // Provider 2 at index 0 signed the snapshot.
        let bucket_id = super::challenge::setup_with_snapshot(2, 1);

        setup_added_primary(3, 1, bucket_id, 50, 100);

        let bucket = Buckets::<Test>::get(bucket_id).unwrap();
        // `primary_signers` is unchanged: the new provider at index 1 has no
        // bit until it signs a checkpoint.
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

#[test]
fn added_primary_is_challengeable_only_after_it_signs_the_snapshot() {
    new_test_ext().execute_with(|| {
        run_to_block(1);
        register_provider(2, 200);
        register_provider(3, 200);
        let bucket_id = setup_agreement(2, 1, 50, 100);

        let commitment = Commitment {
            mmr_root: H256::repeat_byte(0xAB),
            start_seq: 0,
            leaf_count: 10,
        };
        assert_ok!(StorageProvider::checkpoint(
            RuntimeOrigin::signed(1),
            bucket_id,
            commitment,
            vec![(2, sign_commitment(2, bucket_id, commitment))]
                .try_into()
                .unwrap(),
        ));

        // Provider 3 is added after the checkpoint and did not sign it, so
        // its bit at index 1 is clear.
        setup_added_primary(3, 1, bucket_id, 50, 100);
        let bucket = Buckets::<Test>::get(bucket_id).unwrap();
        assert_eq!(bucket.primary_providers.to_vec(), vec![2, 3]);
        assert!(!bucket.snapshot.unwrap().has_provider_signed(1));

        let target = ChunkLocation {
            leaf_index: 0,
            chunk_index: 0,
        };
        assert_noop!(
            StorageProvider::challenge_checkpoint(RuntimeOrigin::signed(4), bucket_id, 3, target),
            Error::<Test>::ProviderNotInSnapshot
        );

        assert_ok!(StorageProvider::extend_checkpoint(
            RuntimeOrigin::signed(1),
            bucket_id,
            vec![(3, sign_commitment(3, bucket_id, commitment))]
                .try_into()
                .unwrap(),
        ));
        assert_ok!(StorageProvider::challenge_checkpoint(
            RuntimeOrigin::signed(4),
            bucket_id,
            3,
            target
        ));
        assert!(System::events().iter().any(|r| matches!(
            r.event,
            RuntimeEvent::StorageProvider(Event::ChallengeCreated { provider: 3, .. })
        )));
    });
}
