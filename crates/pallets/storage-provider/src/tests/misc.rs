// SPDX-License-Identifier: Apache-2.0

use super::*;
use storage_primitives::{ChunkLocation, Commitment};

#[test]
fn update_provider_multiaddr_works() {
    new_test_ext().execute_with(|| {
        frame_system::Pallet::<Test>::set_block_number(1);
        register_provider(1, 200);

        let new_multiaddr = b"/ip4/192.168.0.1/tcp/4000".to_vec();
        assert_ok!(StorageProvider::update_provider_multiaddr(
            RuntimeOrigin::signed(1),
            new_multiaddr.clone().try_into().unwrap(),
        ));

        let provider = Providers::<Test>::get(1).unwrap();
        assert_eq!(provider.multiaddr.to_vec(), new_multiaddr.clone());

        // Verify event emitted
        let expected = RuntimeEvent::StorageProvider(crate::Event::ProviderMultiaddrUpdated {
            provider: 1,
            multiaddr: new_multiaddr.try_into().unwrap(),
        });
        assert!(frame_system::Pallet::<Test>::events()
            .iter()
            .any(|r| r.event == expected));
    });
}

#[test]
fn update_provider_multiaddr_fails_not_registered() {
    new_test_ext().execute_with(|| {
        let new_multiaddr = b"/ip4/192.168.0.1/tcp/4000".to_vec();
        assert_noop!(
            StorageProvider::update_provider_multiaddr(
                RuntimeOrigin::signed(1),
                new_multiaddr.try_into().unwrap(),
            ),
            Error::<Test>::ProviderNotFound
        );
    });
}

#[test]
fn remove_slashed_works() {
    new_test_ext().execute_with(|| {
        register_provider(2, 200);
        let bucket_id = setup_agreement(2, 1, 50, 200);

        // Slash provider's entire reserved stake (mirrors production slash_provider_for_failed_challenge)
        Providers::<Test>::mutate(2, |maybe_provider| {
            if let Some(provider) = maybe_provider {
                let stake = provider.stake;
                let (_, remaining) =
                    <Balances as frame_support::traits::ReservableCurrency<u64>>::slash_reserved(
                        &2, stake,
                    );
                assert_eq!(remaining, 0, "entire stake should have been slashed");
                provider.stake = 0;
            }
        });

        let owner_balance_before = Balances::free_balance(1);
        let agreement = StorageAgreements::<Test>::get(bucket_id, 2).unwrap();
        let payment_locked = agreement.payment_locked;

        // Anyone can call remove_slashed
        assert_ok!(StorageProvider::remove_slashed(
            RuntimeOrigin::signed(3),
            bucket_id,
            2
        ));

        // Agreement removed
        assert!(StorageAgreements::<Test>::get(bucket_id, 2).is_none());
        // Payment returned to owner
        assert_eq!(
            Balances::free_balance(1),
            owner_balance_before + payment_locked
        );
        // Provider removed from bucket
        let bucket = Buckets::<Test>::get(bucket_id).unwrap();
        assert!(!bucket.primary_providers.contains(&2));
    });
}

#[test]
fn remove_slashed_fails_not_slashed() {
    new_test_ext().execute_with(|| {
        register_provider(2, 200);
        let bucket_id = setup_agreement(2, 1, 50, 200);

        // Provider has stake > 0
        assert_noop!(
            StorageProvider::remove_slashed(RuntimeOrigin::signed(3), bucket_id, 2),
            Error::<Test>::ProviderNotSlashed
        );
    });
}

#[test]
fn remove_slashed_fails_no_agreement() {
    new_test_ext().execute_with(|| {
        register_provider(2, 200);
        create_bucket(1, 1);

        // Zero out stake
        Providers::<Test>::mutate(2, |maybe_provider| {
            if let Some(provider) = maybe_provider {
                <Balances as frame_support::traits::ReservableCurrency<u64>>::unreserve(
                    &2,
                    provider.stake,
                );
                provider.stake = 0;
            }
        });

        assert_noop!(
            StorageProvider::remove_slashed(RuntimeOrigin::signed(3), 0, 2),
            Error::<Test>::AgreementNotFound
        );
    });
}

#[test]
fn remove_slashed_fails_provider_not_found() {
    new_test_ext().execute_with(|| {
        create_bucket(1, 1);

        assert_noop!(
            StorageProvider::remove_slashed(RuntimeOrigin::signed(3), 0, 99),
            Error::<Test>::ProviderNotFound
        );
    });
}

#[test]
fn set_extensions_blocked_fails_no_agreement() {
    new_test_ext().execute_with(|| {
        register_provider(2, 200);
        create_bucket(1, 1);

        assert_noop!(
            StorageProvider::set_extensions_blocked(RuntimeOrigin::signed(2), 0, true),
            Error::<Test>::AgreementNotFound
        );
    });
}

#[test]
fn set_extensions_blocked_emits_event() {
    new_test_ext().execute_with(|| {
        frame_system::Pallet::<Test>::set_block_number(1);
        register_provider(2, 200);
        let bucket_id = setup_agreement(2, 1, 50, 100);

        assert_ok!(StorageProvider::set_extensions_blocked(
            RuntimeOrigin::signed(2),
            bucket_id,
            true
        ));

        let expected = RuntimeEvent::StorageProvider(crate::Event::ExtensionsBlocked {
            bucket_id,
            provider: 2,
            blocked: true,
        });
        assert!(frame_system::Pallet::<Test>::events()
            .iter()
            .any(|r| r.event == expected));
    });
}

#[test]
fn register_provider_emits_event() {
    new_test_ext().execute_with(|| {
        frame_system::Pallet::<Test>::set_block_number(1);
        register_provider(2, 200);

        let expected = RuntimeEvent::StorageProvider(crate::Event::ProviderRegistered {
            provider: 2,
            stake: 200,
        });
        assert!(frame_system::Pallet::<Test>::events()
            .iter()
            .any(|r| r.event == expected));
    });
}

#[test]
fn create_bucket_emits_event() {
    new_test_ext().execute_with(|| {
        frame_system::Pallet::<Test>::set_block_number(1);
        let bucket_id = create_bucket(1, 1);

        let expected = RuntimeEvent::StorageProvider(crate::Event::BucketCreated {
            bucket_id,
            admin: 1,
        });
        assert!(frame_system::Pallet::<Test>::events()
            .iter()
            .any(|r| r.event == expected));
    });
}

#[test]
fn establish_agreement_emits_event() {
    new_test_ext().execute_with(|| {
        frame_system::Pallet::<Test>::set_block_number(1);
        register_provider(2, 200);
        let bucket_id = setup_agreement(2, 1, 50, 100);

        // `setup_agreement` redeems provider-signed terms via
        // `establish_storage_agreement`, which emits StorageAgreementEstablished.
        let events = frame_system::Pallet::<Test>::events();
        let found = events.iter().any(|r| {
            matches!(
                &r.event,
                RuntimeEvent::StorageProvider(crate::Event::StorageAgreementEstablished {
                    bucket_id: bid,
                    provider: 2,
                    owner: 1,
                    expires_at: 101, // block 1 + duration 100
                    ..
                }) if *bid == bucket_id
            )
        });
        assert!(found);
    });
}

#[test]
fn challenge_checkpoint_emits_event() {
    new_test_ext().execute_with(|| {
        frame_system::Pallet::<Test>::set_block_number(1);
        register_provider(2, 200);
        let bucket_id = setup_agreement(2, 1, 50, 200);

        // Insert snapshot
        Buckets::<Test>::mutate(bucket_id, |maybe_bucket| {
            if let Some(bucket) = maybe_bucket {
                bucket.snapshot = Some(storage_primitives::BucketSnapshot {
                    commitment: storage_primitives::Commitment {
                        mmr_root: sp_core::H256::repeat_byte(0xAB),
                        start_seq: 0,
                        leaf_count: 10,
                    },
                    checkpoint_block: 1,
                    primary_signers: vec![0x01],
                });
            }
        });

        assert_ok!(StorageProvider::challenge_checkpoint(
            RuntimeOrigin::signed(3),
            bucket_id,
            2,
            ChunkLocation {
                leaf_index: 0,
                chunk_index: 0,
            },
        ));

        let expected = RuntimeEvent::StorageProvider(crate::Event::ChallengeCreated {
            challenge_id: storage_primitives::ChallengeId {
                deadline: 101,
                index: 0,
            },
            bucket_id,
            provider: 2,
            challenger: 3,
            respond_by: 101,
        });
        assert!(frame_system::Pallet::<Test>::events()
            .iter()
            .any(|r| r.event == expected));
    });
}

#[test]
fn checkpoint_emits_event() {
    new_test_ext().execute_with(|| {
        frame_system::Pallet::<Test>::set_block_number(1);
        let bucket_id = create_bucket(1, 0);

        assert_ok!(StorageProvider::checkpoint(
            RuntimeOrigin::signed(1),
            bucket_id,
            Commitment {
                mmr_root: sp_core::H256::repeat_byte(0xAA),
                start_seq: 0,
                leaf_count: 10,
            },
            Default::default(),
        ));

        let expected = RuntimeEvent::StorageProvider(crate::Event::BucketCheckpointed {
            bucket_id,
            commitment: Commitment {
                mmr_root: sp_core::H256::repeat_byte(0xAA),
                start_seq: 0,
                leaf_count: 10,
            },
            providers: vec![],
        });
        assert!(frame_system::Pallet::<Test>::events()
            .iter()
            .any(|r| r.event == expected));
    });
}

#[test]
fn extend_checkpoint_emits_event() {
    new_test_ext().execute_with(|| {
        frame_system::Pallet::<Test>::set_block_number(1);
        let bucket_id = create_bucket(1, 0);

        let commitment = Commitment {
            mmr_root: sp_core::H256::repeat_byte(0xAA),
            start_seq: 0,
            leaf_count: 10,
        };

        assert_ok!(StorageProvider::checkpoint(
            RuntimeOrigin::signed(1),
            bucket_id,
            commitment,
            Default::default(),
        ));

        assert_ok!(StorageProvider::extend_checkpoint(
            RuntimeOrigin::signed(1),
            bucket_id,
            Default::default(),
        ));

        // `extend_checkpoint` only adds signatures — it must re-emit the same
        // commitment the initial checkpoint carried, not a stale or default one.
        let expected = RuntimeEvent::StorageProvider(crate::Event::BucketCheckpointed {
            bucket_id,
            commitment,
            providers: vec![],
        });
        assert!(frame_system::Pallet::<Test>::events()
            .iter()
            .any(|r| r.event == expected));
    });
}

#[test]
fn end_agreement_emits_event() {
    new_test_ext().execute_with(|| {
        frame_system::Pallet::<Test>::set_block_number(1);
        register_provider(2, 200);
        let bucket_id = setup_agreement(2, 1, 50, 100);

        // Advance past agreement expiry (expires_at = 100)
        run_to_block(101);

        // End agreement after expiry (owner can end within settlement window)
        assert_ok!(StorageProvider::end_agreement(
            RuntimeOrigin::signed(1),
            bucket_id,
            2,
            storage_primitives::EndAction::Pay,
        ));

        // Verify AgreementEnded event emitted
        let events = frame_system::Pallet::<Test>::events();
        let found = events.iter().any(|r| {
            matches!(
                &r.event,
                RuntimeEvent::StorageProvider(crate::Event::AgreementEnded {
                    bucket_id: bid,
                    provider: 2,
                    ..
                }) if *bid == bucket_id
            )
        });
        assert!(found);
    });
}

#[test]
fn plain_account_signature_verifies_against_the_account_key_bytes() {
    use sp_core::Pair as _;

    // The `Deleted` defense's signer is the bucket admin — a plain,
    // unregistered account. On AccountId32 runtimes the account's SCALE
    // encoding IS its public key, so a signature by the account's own
    // key must verify against exactly those bytes.
    let pair = sp_core::sr25519::Pair::from_seed(&[42u8; 32]);
    let message = b"deletion payload";
    let sig = sp_runtime::MultiSignature::Sr25519(pair.sign(message));
    let key_bytes = pair.public().0; // == AccountId32::from(pair.public()).encode()

    use crate::impls::signatures::plain_account_verifies;
    assert_eq!(
        plain_account_verifies(&sig, message, &key_bytes),
        Some(true)
    );
    assert_eq!(
        plain_account_verifies(&sig, b"tampered", &key_bytes),
        Some(false)
    );
    // An account whose encoding is not 32 bytes has no plain-account
    // identity to verify against (e.g. this mock's u64 AccountId, whose
    // SCALE encoding is its 8 LE bytes).
    assert_eq!(
        plain_account_verifies(&sig, message, &999u64.to_le_bytes()),
        None
    );
}

#[test]
fn verify_signature_unregistered_signer_maps_plain_account_errors() {
    new_test_ext().execute_with(|| {
        let sig = sp_runtime::MultiSignature::Sr25519(sp_core::sr25519::Signature::from([0u8; 64]));
        // An unregistered signer must fall through to the plain-account
        // path (not error with ProviderNotFound). The mock's u64
        // AccountId has no 32-byte identity, so the fallback reports
        // InvalidPublicKey.
        assert_noop!(
            StorageProvider::verify_signature(&sig, b"msg", &999),
            Error::<Test>::InvalidPublicKey
        );
    });
}
