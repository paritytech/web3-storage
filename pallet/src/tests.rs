//! Tests for the storage provider pallet.

use crate::{mock::*, *};
use codec::Encode;
use frame_support::{assert_noop, assert_ok};
use sp_core::crypto::KeyTypeId;
use storage_primitives::{
    AgreementTerms, BucketId, ProviderRole, ReplicaTerms, Role, REPLAY_WINDOW_BITS,
};

/// Key type used by the keystore in tests for provider signing material.
const PROVIDER_KEY_TYPE: KeyTypeId = KeyTypeId(*b"prov");

/// Static test public key for tests that never exercise signature verification
/// (e.g. provider register/settings flows).
fn test_public_key() -> frame_support::BoundedVec<u8, frame_support::traits::ConstU32<64>> {
    vec![1u8; 32].try_into().unwrap()
}

/// Generate a provider sr25519 keypair inside the runtime keystore.
///
/// The returned public key bytes are what should be stored in
/// `register_provider`'s `public_key` argument so the pallet can verify
/// signatures produced by [`sign_terms`].
fn generate_provider_public_key(
    seed: &str,
) -> (
    sp_core::sr25519::Public,
    frame_support::BoundedVec<u8, frame_support::traits::ConstU32<64>>,
) {
    let public = sp_io::crypto::sr25519_generate(PROVIDER_KEY_TYPE, Some(seed.as_bytes().to_vec()));
    let bounded = public.0.to_vec().try_into().unwrap();
    (public, bounded)
}

/// Sign agreement terms with the provider's keystore key.
fn sign_terms(
    public: &sp_core::sr25519::Public,
    terms: &AgreementTermsOf<Test>,
) -> sp_runtime::MultiSignature {
    let hash = sp_io::hashing::blake2_256(&terms.encode());
    let sig = sp_io::crypto::sr25519_sign(PROVIDER_KEY_TYPE, public, &hash)
        .expect("keystore should sign with a key it generated");
    sp_runtime::MultiSignature::Sr25519(sig)
}

/// Build [`AgreementTermsOf<Test>`] for a primary agreement (no replica params).
fn primary_terms(
    owner: u64,
    max_bytes: u64,
    duration: u64,
    price_per_byte: u64,
    valid_until: u64,
    nonce: u64,
) -> AgreementTermsOf<Test> {
    AgreementTerms {
        owner,
        max_bytes,
        duration,
        price_per_byte,
        valid_until,
        nonce,
        replica_params: None,
    }
}

/// Build [`AgreementTermsOf<Test>`] for a replica agreement.
#[allow(clippy::too_many_arguments)]
fn replica_terms(
    owner: u64,
    max_bytes: u64,
    duration: u64,
    price_per_byte: u64,
    valid_until: u64,
    nonce: u64,
    sync_balance: u64,
    min_sync_interval: u64,
) -> AgreementTermsOf<Test> {
    AgreementTerms {
        owner,
        max_bytes,
        duration,
        price_per_byte,
        valid_until,
        nonce,
        replica_params: Some(ReplicaTerms {
            sync_balance,
            min_sync_interval,
        }),
    }
}

/// Register a provider and apply common settings used by establish-agreement
/// tests. Returns the sr25519 public key the provider was registered with.
fn register_signing_provider(
    provider: u64,
    seed: &str,
    stake: u64,
    settings: ProviderSettings<Test>,
) -> sp_core::sr25519::Public {
    let multiaddr = format!("/ip4/127.0.0.1/tcp/300{provider}");
    let (public, bounded) = generate_provider_public_key(seed);
    assert_ok!(StorageProvider::register_provider(
        RuntimeOrigin::signed(provider),
        multiaddr.as_bytes().to_vec().try_into().unwrap(),
        bounded,
        stake,
    ));
    assert_ok!(StorageProvider::update_provider_settings(
        RuntimeOrigin::signed(provider),
        settings,
    ));
    public
}

/// Sensible default provider settings for tests that just need to accept
/// primaries/replicas.
fn default_test_settings(
    price_per_byte: u64,
    replica_sync_price: Option<u64>,
) -> ProviderSettings<Test> {
    ProviderSettings {
        min_duration: 10,
        max_duration: 1000,
        price_per_byte,
        accepting_primary: true,
        replica_sync_price,
        accepting_extensions: true,
        max_capacity: 0,
    }
}

mod provider_tests {
    use super::*;

    #[test]
    fn register_provider_works() {
        new_test_ext().execute_with(|| {
            let multiaddr = b"/ip4/127.0.0.1/tcp/3000".to_vec();

            assert_ok!(StorageProvider::register_provider(
                RuntimeOrigin::signed(1),
                multiaddr.clone().try_into().unwrap(),
                test_public_key(),
                200
            ));

            let provider = Providers::<Test>::get(1).unwrap();
            assert_eq!(provider.stake, 200);
            assert_eq!(provider.multiaddr.to_vec(), multiaddr);
            assert_eq!(provider.committed_bytes, 0);
        });
    }

    #[test]
    fn register_provider_fails_with_insufficient_stake() {
        new_test_ext().execute_with(|| {
            let multiaddr = b"/ip4/127.0.0.1/tcp/3000".to_vec();

            assert_noop!(
                StorageProvider::register_provider(
                    RuntimeOrigin::signed(1),
                    multiaddr.try_into().unwrap(),
                    test_public_key(),
                    50 // Below minimum of 100
                ),
                Error::<Test>::InsufficientStake
            );
        });
    }

    #[test]
    fn register_provider_fails_if_already_registered() {
        new_test_ext().execute_with(|| {
            let multiaddr = b"/ip4/127.0.0.1/tcp/3000".to_vec();

            assert_ok!(StorageProvider::register_provider(
                RuntimeOrigin::signed(1),
                multiaddr.clone().try_into().unwrap(),
                test_public_key(),
                200
            ));

            assert_noop!(
                StorageProvider::register_provider(
                    RuntimeOrigin::signed(1),
                    multiaddr.try_into().unwrap(),
                    test_public_key(),
                    200
                ),
                Error::<Test>::ProviderAlreadyRegistered
            );
        });
    }

    #[test]
    fn add_stake_works() {
        new_test_ext().execute_with(|| {
            let multiaddr = b"/ip4/127.0.0.1/tcp/3000".to_vec();

            assert_ok!(StorageProvider::register_provider(
                RuntimeOrigin::signed(1),
                multiaddr.try_into().unwrap(),
                test_public_key(),
                200
            ));

            assert_ok!(StorageProvider::add_stake(RuntimeOrigin::signed(1), 100));

            let provider = Providers::<Test>::get(1).unwrap();
            assert_eq!(provider.stake, 300);
        });
    }

    #[test]
    fn add_stake_fails_if_not_registered() {
        new_test_ext().execute_with(|| {
            assert_noop!(
                StorageProvider::add_stake(RuntimeOrigin::signed(1), 100),
                Error::<Test>::ProviderNotFound
            );
        });
    }

    #[test]
    fn deregister_provider_full_flow_announce_then_complete() {
        new_test_ext().execute_with(|| {
            System::set_block_number(1);
            let settings = default_test_settings(0, None);
            let provider_pk = register_signing_provider(1, "//Provider", 200, settings);

            // Open a primary agreement so the provider lifecycle isn't
            // trivial: deregister must wait for committed_bytes to drop
            // back to zero, which only happens once the agreement is
            // settled (via claim_expired_agreement after expiry).
            let terms = primary_terms(2, 50, 100, 0, 10_000, 1);
            let sig = sign_terms(&provider_pk, &terms);
            assert_ok!(StorageProvider::establish_storage_agreement(
                RuntimeOrigin::signed(2),
                1,
                terms,
                sig,
            ));
            assert_eq!(Providers::<Test>::get(1).unwrap().committed_bytes, 50);
            let bucket_id = NextBucketId::<Test>::get() - 1;
            let agreement = StorageAgreements::<Test>::get(bucket_id, 1).unwrap();

            // While the agreement is live, deregister must refuse to
            // even start the announcement.
            assert_noop!(
                StorageProvider::deregister_provider(RuntimeOrigin::signed(1)),
                Error::<Test>::ProviderHasActiveAgreements
            );

            // Wait past expires_at + SettlementTimeout so the provider
            // can claim payment and release its committed_bytes.
            run_to_block(agreement.expires_at + 51);
            assert_ok!(StorageProvider::claim_expired_agreement(
                RuntimeOrigin::signed(1),
                bucket_id,
            ));
            assert_eq!(Providers::<Test>::get(1).unwrap().committed_bytes, 0);

            let balance_before = Balances::free_balance(1);

            // Announce step: provider record stays, stake stays reserved,
            // acceptance flags are forced false, deregister_at is stamped.
            assert_ok!(StorageProvider::deregister_provider(RuntimeOrigin::signed(
                1
            )));
            let provider = Providers::<Test>::get(1).unwrap();
            assert_eq!(
                provider.deregister_at,
                Some(System::block_number() + 100) // DeregisterAnnouncementPeriod in mock
            );
            assert!(!provider.settings.accepting_primary);
            assert!(!provider.settings.accepting_extensions);
            assert_eq!(Balances::free_balance(1), balance_before); // not yet refunded

            // Premature completion is rejected.
            assert_noop!(
                StorageProvider::complete_deregister(RuntimeOrigin::signed(1)),
                Error::<Test>::DeregisterPeriodNotElapsed
            );

            // After the period, complete succeeds and stake comes back.
            let deregister_at = provider.deregister_at.unwrap();
            run_to_block(deregister_at);
            assert_ok!(StorageProvider::complete_deregister(RuntimeOrigin::signed(
                1
            )));
            assert!(Providers::<Test>::get(1).is_none());
            assert_eq!(Balances::free_balance(1), balance_before + 200);
        });
    }

    #[test]
    fn deregister_provider_announcement_is_one_shot() {
        new_test_ext().execute_with(|| {
            let multiaddr = b"/ip4/127.0.0.1/tcp/3000".to_vec();
            assert_ok!(StorageProvider::register_provider(
                RuntimeOrigin::signed(1),
                multiaddr.try_into().unwrap(),
                test_public_key(),
                200
            ));
            assert_ok!(StorageProvider::deregister_provider(RuntimeOrigin::signed(
                1
            )));
            assert_noop!(
                StorageProvider::deregister_provider(RuntimeOrigin::signed(1)),
                Error::<Test>::DeregisterAnnounced
            );
        });
    }

    #[test]
    fn cancel_deregister_clears_announcement() {
        new_test_ext().execute_with(|| {
            let multiaddr = b"/ip4/127.0.0.1/tcp/3000".to_vec();
            assert_ok!(StorageProvider::register_provider(
                RuntimeOrigin::signed(1),
                multiaddr.try_into().unwrap(),
                test_public_key(),
                200
            ));
            assert_ok!(StorageProvider::deregister_provider(RuntimeOrigin::signed(
                1
            )));
            assert!(Providers::<Test>::get(1).unwrap().deregister_at.is_some());

            assert_ok!(StorageProvider::cancel_deregister(RuntimeOrigin::signed(1)));
            let restored = Providers::<Test>::get(1).unwrap();
            assert!(restored.deregister_at.is_none());
            // Cancel mirrors announce: flags that announce forced to false
            // are restored to true.
            assert!(restored.settings.accepting_primary);
            assert!(restored.settings.accepting_extensions);

            // And settings updates work again post-cancel.
            let tweak = ProviderSettings {
                min_duration: 10u64,
                max_duration: 1000u64,
                price_per_byte: 5u64,
                accepting_primary: true,
                replica_sync_price: None,
                accepting_extensions: true,
                max_capacity: 0,
            };
            assert_ok!(StorageProvider::update_provider_settings(
                RuntimeOrigin::signed(1),
                tweak
            ));
        });
    }

    #[test]
    fn cancel_deregister_fails_without_announcement() {
        new_test_ext().execute_with(|| {
            let multiaddr = b"/ip4/127.0.0.1/tcp/3000".to_vec();
            assert_ok!(StorageProvider::register_provider(
                RuntimeOrigin::signed(1),
                multiaddr.try_into().unwrap(),
                test_public_key(),
                200
            ));
            assert_noop!(
                StorageProvider::cancel_deregister(RuntimeOrigin::signed(1)),
                Error::<Test>::DeregisterNotAnnounced
            );
        });
    }

    #[test]
    fn complete_deregister_fails_without_announcement() {
        new_test_ext().execute_with(|| {
            let multiaddr = b"/ip4/127.0.0.1/tcp/3000".to_vec();
            assert_ok!(StorageProvider::register_provider(
                RuntimeOrigin::signed(1),
                multiaddr.try_into().unwrap(),
                test_public_key(),
                200
            ));
            assert_noop!(
                StorageProvider::complete_deregister(RuntimeOrigin::signed(1)),
                Error::<Test>::DeregisterNotAnnounced
            );
        });
    }

    #[test]
    fn update_provider_settings_blocked_while_announcement_pending() {
        new_test_ext().execute_with(|| {
            let multiaddr = b"/ip4/127.0.0.1/tcp/3000".to_vec();
            assert_ok!(StorageProvider::register_provider(
                RuntimeOrigin::signed(1),
                multiaddr.try_into().unwrap(),
                test_public_key(),
                200
            ));
            assert_ok!(StorageProvider::deregister_provider(RuntimeOrigin::signed(
                1
            )));

            let resumed = ProviderSettings {
                min_duration: 10u64,
                max_duration: 1000u64,
                price_per_byte: 5u64,
                accepting_primary: true, // attempts to un-freeze
                replica_sync_price: None,
                accepting_extensions: true,
                max_capacity: 0,
            };
            assert_noop!(
                StorageProvider::update_provider_settings(RuntimeOrigin::signed(1), resumed),
                Error::<Test>::DeregisterAnnounced
            );
        });
    }

    #[test]
    fn establish_storage_agreement_rejects_due_to_deregistering_provider() {
        // Once a provider has announced deregistration,
        // `establish_storage_agreement` (and `establish_replica_agreement`)
        // must reject otherwise-valid signed terms with `DeregisterAnnounced`.
        new_test_ext().execute_with(|| {
            System::set_block_number(1);
            let settings = default_test_settings(0, Some(1));
            let provider_pk = register_signing_provider(2, "//Provider", 200, settings);

            assert_ok!(StorageProvider::deregister_provider(RuntimeOrigin::signed(
                2
            )));

            let terms = primary_terms(1, 50, 100, 0, 1_000, 1);
            let sig = sign_terms(&provider_pk, &terms);
            assert_noop!(
                StorageProvider::establish_storage_agreement(
                    RuntimeOrigin::signed(1),
                    2,
                    terms,
                    sig,
                ),
                Error::<Test>::DeregisterAnnounced
            );
        });
    }

    #[test]
    fn complete_deregister_drains_checkpoint_rewards() {
        new_test_ext().execute_with(|| {
            let multiaddr = b"/ip4/127.0.0.1/tcp/3000".to_vec();
            assert_ok!(StorageProvider::register_provider(
                RuntimeOrigin::signed(1),
                multiaddr.try_into().unwrap(),
                test_public_key(),
                200
            ));

            // Seed pending rewards across two buckets for this provider. We
            // poke storage directly because the on-chain reward-credit path
            // requires a full checkpoint setup that's orthogonal to this
            // test.
            CheckpointRewards::<Test>::insert(1, 100u64, 30u64);
            CheckpointRewards::<Test>::insert(1, 200u64, 70u64);
            // Unrelated provider's reward in another bucket — must survive.
            CheckpointRewards::<Test>::insert(2, 100u64, 999u64);

            let free_before = Balances::free_balance(1);

            assert_ok!(StorageProvider::deregister_provider(RuntimeOrigin::signed(
                1
            )));
            let deregister_at = Providers::<Test>::get(1).unwrap().deregister_at.unwrap();
            run_to_block(deregister_at);
            assert_ok!(StorageProvider::complete_deregister(RuntimeOrigin::signed(
                1
            )));

            // 200 (stake) + 30 + 70 (drained rewards) = 300 added to free balance.
            assert_eq!(Balances::free_balance(1), free_before + 300);
            // Provider's reward entries are gone.
            assert_eq!(CheckpointRewards::<Test>::iter_prefix(1u64).count(), 0);
            // Unrelated provider's reward is untouched.
            assert_eq!(CheckpointRewards::<Test>::get(2u64, 100u64), 999);
        });
    }

    #[test]
    fn deregister_provider_fails_with_active_agreements() {
        new_test_ext().execute_with(|| {
            System::set_block_number(1);
            let settings = default_test_settings(0, None);
            let provider_pk = register_signing_provider(2, "//Provider", 200, settings);

            // Open a primary agreement via the signed-terms flow so the
            // provider has live committed bytes.
            let terms = primary_terms(1, 100, 100, 0, 1_000, 1);
            let sig = sign_terms(&provider_pk, &terms);
            assert_ok!(StorageProvider::establish_storage_agreement(
                RuntimeOrigin::signed(1),
                2,
                terms,
                sig,
            ));

            assert_noop!(
                StorageProvider::deregister_provider(RuntimeOrigin::signed(2)),
                Error::<Test>::ProviderHasActiveAgreements
            );
        });
    }

    #[test]
    fn update_provider_settings_works() {
        new_test_ext().execute_with(|| {
            let multiaddr = b"/ip4/127.0.0.1/tcp/3000".to_vec();

            assert_ok!(StorageProvider::register_provider(
                RuntimeOrigin::signed(1),
                multiaddr.try_into().unwrap(),
                test_public_key(),
                200
            ));

            let new_settings = ProviderSettings {
                min_duration: 10u64,
                max_duration: 1000u64,
                price_per_byte: 5u64,
                accepting_primary: true,
                replica_sync_price: Some(10u64),
                accepting_extensions: true,
                max_capacity: 0, // Unlimited
            };

            assert_ok!(StorageProvider::update_provider_settings(
                RuntimeOrigin::signed(1),
                new_settings.clone()
            ));

            let provider = Providers::<Test>::get(1).unwrap();
            assert_eq!(provider.settings.price_per_byte, 5);
            assert_eq!(provider.settings.replica_sync_price, Some(10));
            assert_eq!(provider.settings.max_capacity, 0);
        });
    }

    #[test]
    fn update_provider_settings_with_max_capacity_works() {
        new_test_ext().execute_with(|| {
            let multiaddr = b"/ip4/127.0.0.1/tcp/3000".to_vec();

            // Register with enough stake for 10000 bytes (stake >= bytes * MinStakePerByte)
            // MinStakePerByte = 1 in mock, so stake of 200 covers 200 bytes
            assert_ok!(StorageProvider::register_provider(
                RuntimeOrigin::signed(1),
                multiaddr.try_into().unwrap(),
                test_public_key(),
                200
            ));

            let new_settings = ProviderSettings {
                min_duration: 10u64,
                max_duration: 1000u64,
                price_per_byte: 5u64,
                accepting_primary: true,
                replica_sync_price: None,
                accepting_extensions: true,
                max_capacity: 200, // Up to 200 bytes (within stake limit)
            };

            assert_ok!(StorageProvider::update_provider_settings(
                RuntimeOrigin::signed(1),
                new_settings.clone()
            ));

            let provider = Providers::<Test>::get(1).unwrap();
            assert_eq!(provider.settings.max_capacity, 200);
        });
    }

    #[test]
    fn update_provider_settings_fails_with_capacity_below_committed() {
        new_test_ext().execute_with(|| {
            System::set_block_number(1);
            let settings = default_test_settings(0, None);
            let provider_pk = register_signing_provider(2, "//Provider", 200, settings);

            // Open a 100-byte primary agreement so committed_bytes = 100.
            let terms = primary_terms(1, 100, 100, 0, 1_000, 1);
            let sig = sign_terms(&provider_pk, &terms);
            assert_ok!(StorageProvider::establish_storage_agreement(
                RuntimeOrigin::signed(1),
                2,
                terms,
                sig,
            ));

            let new_settings = ProviderSettings {
                min_duration: 10u64,
                max_duration: 1000u64,
                price_per_byte: 5u64,
                accepting_primary: true,
                replica_sync_price: None,
                accepting_extensions: true,
                max_capacity: 50, // Below committed 100 bytes
            };

            assert_noop!(
                StorageProvider::update_provider_settings(RuntimeOrigin::signed(2), new_settings),
                Error::<Test>::CapacityBelowCommitted
            );
        });
    }

    #[test]
    fn update_provider_settings_fails_with_insufficient_stake_for_capacity() {
        new_test_ext().execute_with(|| {
            let multiaddr = b"/ip4/127.0.0.1/tcp/3000".to_vec();

            // Register with stake of 200
            assert_ok!(StorageProvider::register_provider(
                RuntimeOrigin::signed(1),
                multiaddr.try_into().unwrap(),
                test_public_key(),
                200
            ));

            // Try to set capacity that requires more stake than available
            // MinStakePerByte = 1 in mock, so 200 stake only covers 200 bytes
            let new_settings = ProviderSettings {
                min_duration: 10u64,
                max_duration: 1000u64,
                price_per_byte: 5u64,
                accepting_primary: true,
                replica_sync_price: None,
                accepting_extensions: true,
                max_capacity: 1000, // Requires 1000 stake, but only have 200
            };

            assert_noop!(
                StorageProvider::update_provider_settings(RuntimeOrigin::signed(1), new_settings),
                Error::<Test>::InsufficientStakeForCapacity
            );
        });
    }

    #[test]
    fn update_provider_settings_fails_when_min_duration_above_max() {
        new_test_ext().execute_with(|| {
            let multiaddr = b"/ip4/127.0.0.1/tcp/3000".to_vec();

            assert_ok!(StorageProvider::register_provider(
                RuntimeOrigin::signed(1),
                multiaddr.try_into().unwrap(),
                test_public_key(),
                200
            ));

            // min_duration > max_duration would make the provider impossible to
            // match against any duration; reject it at the entry point.
            let bad_settings = ProviderSettings {
                min_duration: 1000u64,
                max_duration: 10u64,
                price_per_byte: 5u64,
                accepting_primary: true,
                replica_sync_price: None,
                accepting_extensions: true,
                max_capacity: 0,
            };

            assert_noop!(
                StorageProvider::update_provider_settings(RuntimeOrigin::signed(1), bad_settings),
                Error::<Test>::MinDurationExceedsMaxDuration
            );

            // Equal endpoints are allowed (single-duration providers).
            let edge_settings = ProviderSettings {
                min_duration: 100u64,
                max_duration: 100u64,
                price_per_byte: 5u64,
                accepting_primary: true,
                replica_sync_price: None,
                accepting_extensions: true,
                max_capacity: 0,
            };
            assert_ok!(StorageProvider::update_provider_settings(
                RuntimeOrigin::signed(1),
                edge_settings
            ));
        });
    }

    #[test]
    fn update_provider_settings_emits_event_with_new_settings() {
        new_test_ext().execute_with(|| {
            // System events are only collected after block 0.
            frame_system::Pallet::<Test>::set_block_number(1);

            let multiaddr = b"/ip4/127.0.0.1/tcp/3000".to_vec();
            assert_ok!(StorageProvider::register_provider(
                RuntimeOrigin::signed(1),
                multiaddr.try_into().unwrap(),
                test_public_key(),
                200
            ));

            let new_settings = ProviderSettings {
                min_duration: 10u64,
                max_duration: 1000u64,
                price_per_byte: 5u64,
                accepting_primary: true,
                replica_sync_price: Some(10u64),
                accepting_extensions: true,
                max_capacity: 0,
            };

            assert_ok!(StorageProvider::update_provider_settings(
                RuntimeOrigin::signed(1),
                new_settings.clone()
            ));

            // Indexers should not need a follow-up storage read — the event
            // carries the full new settings payload.
            let expected = RuntimeEvent::StorageProvider(crate::Event::ProviderSettingsUpdated {
                provider: 1,
                settings: new_settings,
            });
            assert!(
                frame_system::Pallet::<Test>::events()
                    .iter()
                    .any(|r| r.event == expected),
                "ProviderSettingsUpdated event with full settings was not emitted"
            );
        });
    }

    #[test]
    fn set_extensions_blocked_works_on_active_agreement() {
        new_test_ext().execute_with(|| {
            System::set_block_number(1);
            let settings = default_test_settings(0, None);
            let provider_pk = register_signing_provider(2, "//Provider", 200, settings);

            let terms = primary_terms(1, 100, 100, 0, 1_000, 1);
            let sig = sign_terms(&provider_pk, &terms);
            assert_ok!(StorageProvider::establish_storage_agreement(
                RuntimeOrigin::signed(1),
                2,
                terms,
                sig,
            ));
            let bucket_id = NextBucketId::<Test>::get() - 1;

            assert_ok!(StorageProvider::set_extensions_blocked(
                RuntimeOrigin::signed(2),
                bucket_id,
                true,
            ));
            let agreement = StorageAgreements::<Test>::get(bucket_id, 2).unwrap();
            assert!(agreement.extensions_blocked);

            assert_ok!(StorageProvider::set_extensions_blocked(
                RuntimeOrigin::signed(2),
                bucket_id,
                false,
            ));
            let agreement = StorageAgreements::<Test>::get(bucket_id, 2).unwrap();
            assert!(!agreement.extensions_blocked);
        });
    }

    #[test]
    fn set_extensions_blocked_fails_after_agreement_expires() {
        new_test_ext().execute_with(|| {
            System::set_block_number(1);
            let settings = default_test_settings(0, None);
            let provider_pk = register_signing_provider(2, "//Provider", 200, settings);

            let terms = primary_terms(1, 100, 100, 0, 1_000, 1);
            let sig = sign_terms(&provider_pk, &terms);
            assert_ok!(StorageProvider::establish_storage_agreement(
                RuntimeOrigin::signed(1),
                2,
                terms,
                sig,
            ));
            let bucket_id = NextBucketId::<Test>::get() - 1;
            let agreement = StorageAgreements::<Test>::get(bucket_id, 2).unwrap();

            // At expires_at exactly, the agreement is no longer extendable
            // (strict `<` in the pallet guard).
            run_to_block(agreement.expires_at);
            assert_noop!(
                StorageProvider::set_extensions_blocked(RuntimeOrigin::signed(2), bucket_id, true),
                Error::<Test>::AgreementExpired
            );

            // Past expiry, same rejection.
            run_to_block(agreement.expires_at + 1);
            assert_noop!(
                StorageProvider::set_extensions_blocked(RuntimeOrigin::signed(2), bucket_id, true),
                Error::<Test>::AgreementExpired
            );
        });
    }

    #[test]
    fn establish_storage_agreement_fails_when_capacity_exceeded() {
        new_test_ext().execute_with(|| {
            System::set_block_number(1);
            let settings = ProviderSettings {
                min_duration: 0u64,
                max_duration: 1000u64,
                price_per_byte: 1u64,
                accepting_primary: true,
                replica_sync_price: None,
                accepting_extensions: true,
                max_capacity: 50,
            };
            let provider_pk = register_signing_provider(2, "//Provider", 200, settings);

            // 60 bytes exceeds the provider's 50-byte cap.
            let terms = primary_terms(1, 60, 10, 1, 1_000, 1);
            let sig = sign_terms(&provider_pk, &terms);
            assert_noop!(
                StorageProvider::establish_storage_agreement(
                    RuntimeOrigin::signed(1),
                    2,
                    terms,
                    sig,
                ),
                Error::<Test>::CapacityExceeded
            );
        });
    }

    #[test]
    fn establish_storage_agreement_works_with_unlimited_capacity() {
        new_test_ext().execute_with(|| {
            System::set_block_number(1);
            let settings = ProviderSettings {
                min_duration: 0u64,
                max_duration: 1000u64,
                price_per_byte: 1u64,
                accepting_primary: true,
                replica_sync_price: None,
                accepting_extensions: true,
                max_capacity: 0, // Unlimited
            };
            let provider_pk = register_signing_provider(2, "//Provider", 200, settings);

            let terms = primary_terms(1, 100, 10, 1, 1_000, 1);
            let sig = sign_terms(&provider_pk, &terms);
            assert_ok!(StorageProvider::establish_storage_agreement(
                RuntimeOrigin::signed(1),
                2,
                terms,
                sig,
            ));

            let provider = Providers::<Test>::get(2).unwrap();
            assert_eq!(provider.committed_bytes, 100);
        });
    }

    #[test]
    fn establish_storage_agreement_works_within_capacity() {
        new_test_ext().execute_with(|| {
            System::set_block_number(1);
            let settings = ProviderSettings {
                min_duration: 0u64,
                max_duration: 1000u64,
                price_per_byte: 1u64,
                accepting_primary: true,
                replica_sync_price: None,
                accepting_extensions: true,
                max_capacity: 150,
            };
            let provider_pk = register_signing_provider(2, "//Provider", 200, settings);

            let terms = primary_terms(1, 100, 10, 1, 1_000, 1);
            let sig = sign_terms(&provider_pk, &terms);
            assert_ok!(StorageProvider::establish_storage_agreement(
                RuntimeOrigin::signed(1),
                2,
                terms,
                sig,
            ));

            let provider = Providers::<Test>::get(2).unwrap();
            assert_eq!(provider.committed_bytes, 100);
            assert_eq!(provider.settings.max_capacity, 150);
        });
    }
}

mod establish_storage_agreement_tests {
    use super::*;

    /// Happy path: signed terms produce a bucket + primary agreement
    /// atomically, the provider's `committed_bytes` advances, and
    /// `ProviderReplayState` records the nonce.
    #[test]
    fn establishes_bucket_and_primary_agreement() {
        new_test_ext().execute_with(|| {
            System::set_block_number(1);
            let settings = default_test_settings(0, None);
            let provider_pk = register_signing_provider(2, "//Provider", 200, settings);

            let owner_balance_before = Balances::free_balance(1);
            let terms = primary_terms(1, 100, 100, 0, 1_000, 7);
            let sig = sign_terms(&provider_pk, &terms);

            assert_ok!(StorageProvider::establish_storage_agreement(
                RuntimeOrigin::signed(1),
                2,
                terms.clone(),
                sig,
            ));

            // Bucket created with owner as sole admin and provider as primary.
            let bucket_id = NextBucketId::<Test>::get() - 1;
            let bucket = Buckets::<Test>::get(bucket_id).unwrap();
            assert_eq!(bucket.primary_providers.to_vec(), vec![2]);
            assert_eq!(bucket.members[0].account, 1);
            assert_eq!(bucket.members[0].role, Role::Admin);

            // Primary agreement opened, with terms reflected in storage.
            let agreement = StorageAgreements::<Test>::get(bucket_id, 2).unwrap();
            assert_eq!(agreement.owner, 1);
            assert_eq!(agreement.max_bytes, 100);
            assert!(matches!(agreement.role, ProviderRole::Primary));

            // Provider commitments updated.
            let provider = Providers::<Test>::get(2).unwrap();
            assert_eq!(provider.committed_bytes, 100);
            assert_eq!(provider.stats.agreements_total, 1);

            // No payment since price_per_byte = 0.
            assert_eq!(Balances::free_balance(1), owner_balance_before);

            // Replay window now anchored at nonce 7.
            let window = ProviderReplayStates::<Test>::get(2);
            assert_eq!(window.hwm, 7);
            assert_eq!(window.bitmap[0] & 1, 1);
        });
    }

    #[test]
    fn reserves_payment_at_signed_price() {
        new_test_ext().execute_with(|| {
            System::set_block_number(1);
            // Provider signs terms at price 1; pallet should reserve that
            // amount even if the on-chain advertised price later drops.
            let settings = ProviderSettings {
                min_duration: 0u64,
                max_duration: 1000u64,
                price_per_byte: 1u64,
                accepting_primary: true,
                replica_sync_price: None,
                accepting_extensions: true,
                max_capacity: 0,
            };
            let provider_pk = register_signing_provider(2, "//Provider", 200, settings);

            let before = Balances::free_balance(1);
            // payment = 1 * 100 * 10 = 1000
            let terms = primary_terms(1, 100, 10, 1, 1_000, 1);
            let sig = sign_terms(&provider_pk, &terms);
            assert_ok!(StorageProvider::establish_storage_agreement(
                RuntimeOrigin::signed(1),
                2,
                terms,
                sig,
            ));
            assert_eq!(Balances::free_balance(1), before - 1000);
        });
    }

    #[test]
    fn rejects_when_terms_owner_does_not_match_origin() {
        new_test_ext().execute_with(|| {
            System::set_block_number(1);
            let settings = default_test_settings(0, None);
            let provider_pk = register_signing_provider(2, "//Provider", 200, settings);

            // Terms signed for owner = 1, but origin = 3.
            let terms = primary_terms(1, 100, 100, 0, 1_000, 1);
            let sig = sign_terms(&provider_pk, &terms);

            assert_noop!(
                StorageProvider::establish_storage_agreement(
                    RuntimeOrigin::signed(3),
                    2,
                    terms,
                    sig,
                ),
                Error::<Test>::TermsOwnerMismatch
            );
        });
    }

    #[test]
    fn rejects_expired_terms() {
        new_test_ext().execute_with(|| {
            System::set_block_number(50);
            let settings = default_test_settings(0, None);
            let provider_pk = register_signing_provider(2, "//Provider", 200, settings);

            // valid_until is in the past.
            let terms = primary_terms(1, 100, 100, 0, 10, 1);
            let sig = sign_terms(&provider_pk, &terms);
            assert_noop!(
                StorageProvider::establish_storage_agreement(
                    RuntimeOrigin::signed(1),
                    2,
                    terms,
                    sig,
                ),
                Error::<Test>::TermsExpired
            );
        });
    }

    #[test]
    fn rejects_signature_from_wrong_signer() {
        new_test_ext().execute_with(|| {
            System::set_block_number(1);
            let settings = default_test_settings(0, None);
            let _provider_pk = register_signing_provider(2, "//Provider", 200, settings.clone());
            // A second, unrelated keypair the pallet has never heard of.
            let (other_pk, _) = generate_provider_public_key("//Imposter");

            let terms = primary_terms(1, 100, 100, 0, 1_000, 1);
            let sig = sign_terms(&other_pk, &terms);
            assert_noop!(
                StorageProvider::establish_storage_agreement(
                    RuntimeOrigin::signed(1),
                    2,
                    terms,
                    sig,
                ),
                Error::<Test>::InvalidProviderSignature
            );
        });
    }

    #[test]
    fn rejects_tampered_terms() {
        new_test_ext().execute_with(|| {
            System::set_block_number(1);
            let settings = default_test_settings(0, None);
            let provider_pk = register_signing_provider(2, "//Provider", 200, settings);

            // Sign one set of terms, then submit a different set with the
            // same signature: signature won't verify over the new encoding.
            let original = primary_terms(1, 100, 100, 0, 1_000, 1);
            let sig = sign_terms(&provider_pk, &original);

            let mut tampered = original.clone();
            tampered.max_bytes = 999;

            assert_noop!(
                StorageProvider::establish_storage_agreement(
                    RuntimeOrigin::signed(1),
                    2,
                    tampered,
                    sig,
                ),
                Error::<Test>::InvalidProviderSignature
            );
        });
    }

    #[test]
    fn rejects_unregistered_provider() {
        new_test_ext().execute_with(|| {
            System::set_block_number(1);
            // Generate a key but never register the provider.
            let (provider_pk, _) = generate_provider_public_key("//Ghost");
            let terms = primary_terms(1, 100, 100, 0, 1_000, 1);
            let sig = sign_terms(&provider_pk, &terms);
            assert_noop!(
                StorageProvider::establish_storage_agreement(
                    RuntimeOrigin::signed(1),
                    2,
                    terms,
                    sig,
                ),
                Error::<Test>::ProviderNotFound
            );
        });
    }

    #[test]
    fn rejects_provider_not_accepting_primary() {
        new_test_ext().execute_with(|| {
            System::set_block_number(1);
            let mut settings = default_test_settings(0, None);
            settings.accepting_primary = false;
            let provider_pk = register_signing_provider(2, "//Provider", 200, settings);

            let terms = primary_terms(1, 100, 100, 0, 1_000, 1);
            let sig = sign_terms(&provider_pk, &terms);
            assert_noop!(
                StorageProvider::establish_storage_agreement(
                    RuntimeOrigin::signed(1),
                    2,
                    terms,
                    sig,
                ),
                Error::<Test>::ProviderNotAcceptingPrimary
            );
        });
    }

    #[test]
    fn rejects_duration_below_provider_minimum() {
        new_test_ext().execute_with(|| {
            System::set_block_number(1);
            let settings = ProviderSettings {
                min_duration: 500,
                max_duration: 1000,
                price_per_byte: 0,
                accepting_primary: true,
                replica_sync_price: None,
                accepting_extensions: true,
                max_capacity: 0,
            };
            let provider_pk = register_signing_provider(2, "//Provider", 200, settings);

            let terms = primary_terms(1, 100, 100, 0, 1_000, 1);
            let sig = sign_terms(&provider_pk, &terms);
            assert_noop!(
                StorageProvider::establish_storage_agreement(
                    RuntimeOrigin::signed(1),
                    2,
                    terms,
                    sig,
                ),
                Error::<Test>::DurationTooShort
            );
        });
    }

    #[test]
    fn rejects_when_signed_price_below_on_chain_price() {
        // If a provider raises their on-chain price after signing, the
        // pallet enforces `provider_info.price_per_byte <= terms.price_per_byte`
        // and rejects with `PaymentExceedsMax`.
        new_test_ext().execute_with(|| {
            System::set_block_number(1);
            let settings = ProviderSettings {
                min_duration: 0,
                max_duration: 1000,
                price_per_byte: 5, // Current on-chain price.
                accepting_primary: true,
                replica_sync_price: None,
                accepting_extensions: true,
                max_capacity: 0,
            };
            let provider_pk = register_signing_provider(2, "//Provider", 200, settings);

            // Signed terms quote a stale, lower price.
            let terms = primary_terms(1, 10, 10, 1, 1_000, 1);
            let sig = sign_terms(&provider_pk, &terms);
            assert_noop!(
                StorageProvider::establish_storage_agreement(
                    RuntimeOrigin::signed(1),
                    2,
                    terms,
                    sig,
                ),
                Error::<Test>::PaymentExceedsMax
            );
        });
    }

    #[test]
    fn rejects_when_stake_insufficient_for_committed_bytes() {
        new_test_ext().execute_with(|| {
            System::set_block_number(1);
            // MinStakePerByte = 1, stake = 100 → can only back 100 bytes.
            let settings = default_test_settings(0, None);
            let provider_pk = register_signing_provider(2, "//Provider", 100, settings);

            // 200 bytes requires 200 stake; provider only has 100.
            let terms = primary_terms(1, 200, 100, 0, 1_000, 1);
            let sig = sign_terms(&provider_pk, &terms);
            assert_noop!(
                StorageProvider::establish_storage_agreement(
                    RuntimeOrigin::signed(1),
                    2,
                    terms,
                    sig,
                ),
                Error::<Test>::InsufficientStakeForBytes
            );
        });
    }

    #[test]
    fn rejects_replayed_nonce_in_window() {
        // Same nonce twice — the second submission lands inside the window
        // with its bit already set.
        new_test_ext().execute_with(|| {
            System::set_block_number(1);
            let settings = default_test_settings(0, None);
            let provider_pk = register_signing_provider(2, "//Provider", 1_000, settings);

            let terms = primary_terms(1, 10, 100, 0, 1_000, 1);
            let sig = sign_terms(&provider_pk, &terms);
            assert_ok!(StorageProvider::establish_storage_agreement(
                RuntimeOrigin::signed(1),
                2,
                terms.clone(),
                sig.clone(),
            ));
            // Same nonce, same terms → AlreadyUsed.
            assert_noop!(
                StorageProvider::establish_storage_agreement(
                    RuntimeOrigin::signed(1),
                    2,
                    terms,
                    sig,
                ),
                Error::<Test>::NonceAlreadyUsed
            );
        });
    }

    #[test]
    fn accepts_nonce_at_window_edge_and_rejects_one_past() {
        // After advancing hwm to 300, nonce 45 (distance 255) is still in
        // the window, but nonce 44 (distance 256) is one slot past it.
        new_test_ext().execute_with(|| {
            System::set_block_number(1);
            let settings = default_test_settings(0, None);
            let provider_pk = register_signing_provider(2, "//Provider", 5_000, settings);

            let advance = primary_terms(1, 1, 100, 0, 1_000, 300);
            let sig = sign_terms(&provider_pk, &advance);
            assert_ok!(StorageProvider::establish_storage_agreement(
                RuntimeOrigin::signed(1),
                2,
                advance,
                sig,
            ));
            assert_eq!(ProviderReplayStates::<Test>::get(2).hwm, 300);

            // Distance == REPLAY_WINDOW_BITS - 1 ⇒ accepted.
            let edge_nonce = 300 - (REPLAY_WINDOW_BITS as u64 - 1);
            let at_edge = primary_terms(1, 1, 100, 0, 1_000, edge_nonce);
            let sig = sign_terms(&provider_pk, &at_edge);
            assert_ok!(StorageProvider::establish_storage_agreement(
                RuntimeOrigin::signed(1),
                2,
                at_edge,
                sig,
            ));

            // Distance == REPLAY_WINDOW_BITS ⇒ rejected.
            let past_edge_nonce = 300 - REPLAY_WINDOW_BITS as u64;
            let past_edge = primary_terms(1, 1, 100, 0, 1_000, past_edge_nonce);
            let sig = sign_terms(&provider_pk, &past_edge);
            assert_noop!(
                StorageProvider::establish_storage_agreement(
                    RuntimeOrigin::signed(1),
                    2,
                    past_edge,
                    sig,
                ),
                Error::<Test>::NonceTooOld
            );
        });
    }

    #[test]
    fn rejects_nonce_far_below_window() {
        new_test_ext().execute_with(|| {
            System::set_block_number(1);
            let settings = default_test_settings(0, None);
            let provider_pk = register_signing_provider(2, "//Provider", 5_000, settings);

            let advance = primary_terms(1, 1, 100, 0, 1_000, 100_000);
            let sig = sign_terms(&provider_pk, &advance);
            assert_ok!(StorageProvider::establish_storage_agreement(
                RuntimeOrigin::signed(1),
                2,
                advance,
                sig,
            ));

            let ancient = primary_terms(1, 1, 100, 0, 1_000, 5);
            let sig = sign_terms(&provider_pk, &ancient);
            assert_noop!(
                StorageProvider::establish_storage_agreement(
                    RuntimeOrigin::signed(1),
                    2,
                    ancient,
                    sig,
                ),
                Error::<Test>::NonceTooOld
            );
        });
    }

    #[test]
    fn accepts_out_of_order_nonces() {
        // Quoting concurrency: nonces issued out of order should all be
        // accepted as long as none are replayed.
        new_test_ext().execute_with(|| {
            System::set_block_number(1);
            let settings = default_test_settings(0, None);
            let provider_pk = register_signing_provider(2, "//Provider", 5_000, settings);

            for nonce in [3u64, 7, 1, 10, 2] {
                let terms = primary_terms(1, 1, 100, 0, 1_000, nonce);
                let sig = sign_terms(&provider_pk, &terms);
                assert_ok!(StorageProvider::establish_storage_agreement(
                    RuntimeOrigin::signed(1),
                    2,
                    terms,
                    sig,
                ));
            }

            // hwm follows the max nonce seen.
            let window = ProviderReplayStates::<Test>::get(2);
            assert_eq!(window.hwm, 10);

            // Replays of any of those nonces are rejected.
            for nonce in [3u64, 7, 1, 10, 2] {
                let terms = primary_terms(1, 1, 100, 0, 1_000, nonce);
                let sig = sign_terms(&provider_pk, &terms);
                assert_noop!(
                    StorageProvider::establish_storage_agreement(
                        RuntimeOrigin::signed(1),
                        2,
                        terms,
                        sig,
                    ),
                    Error::<Test>::NonceAlreadyUsed
                );
            }
        });
    }

    #[test]
    fn forward_jump_beyond_window_clears_old_bits() {
        // Bitmap shift: when hwm jumps forward by >= REPLAY_WINDOW_BITS,
        // every previously-set bit drops off the window so prior nonces
        // are now NonceTooOld, not NonceAlreadyUsed.
        new_test_ext().execute_with(|| {
            System::set_block_number(1);
            let settings = default_test_settings(0, None);
            let provider_pk = register_signing_provider(2, "//Provider", 5_000, settings);

            for nonce in [1u64, 2, 50] {
                let terms = primary_terms(1, 1, 100, 0, 1_000, nonce);
                let sig = sign_terms(&provider_pk, &terms);
                assert_ok!(StorageProvider::establish_storage_agreement(
                    RuntimeOrigin::signed(1),
                    2,
                    terms,
                    sig,
                ));
            }
            // Jump forward by >> REPLAY_WINDOW_BITS so the bitmap is fully cleared.
            let jump = primary_terms(1, 1, 100, 0, 1_000, 10_000);
            let sig = sign_terms(&provider_pk, &jump);
            assert_ok!(StorageProvider::establish_storage_agreement(
                RuntimeOrigin::signed(1),
                2,
                jump,
                sig,
            ));

            let window = ProviderReplayStates::<Test>::get(2);
            assert_eq!(window.hwm, 10_000);
            // Only the new hwm bit is set; everything else is zero.
            assert_eq!(window.bitmap[0], 0b0000_0001);
            for byte in &window.bitmap[1..] {
                assert_eq!(*byte, 0);
            }

            // The previously-used nonces now report TooOld, proving the
            // bitmap shifted them out (not AlreadyUsed).
            for nonce in [1u64, 2, 50] {
                let terms = primary_terms(1, 1, 100, 0, 1_000, nonce);
                let sig = sign_terms(&provider_pk, &terms);
                assert_noop!(
                    StorageProvider::establish_storage_agreement(
                        RuntimeOrigin::signed(1),
                        2,
                        terms,
                        sig,
                    ),
                    Error::<Test>::NonceTooOld
                );
            }
        });
    }

    #[test]
    fn max_primary_providers_enforced_via_establish() {
        // `establish_storage_agreement` always creates a fresh single-primary
        // bucket, so the limit is exercised at bucket creation: 5 buckets
        // succeed, the 6th still succeeds (each is independent). To test the
        // primary-cap path, we use `establish_replica_agreement` over the
        // same bucket — but that exercises a different code path.
        // Here we just confirm 5 sequential establishments produce 5 buckets,
        // each with their own primary.
        new_test_ext().execute_with(|| {
            System::set_block_number(1);
            for i in 2..=6u64 {
                let settings = default_test_settings(0, None);
                let seed = format!("//Provider{i}");
                let provider_pk = register_signing_provider(i, &seed, 200, settings);
                let terms = primary_terms(1, 10, 100, 0, 1_000, 1);
                let sig = sign_terms(&provider_pk, &terms);
                assert_ok!(StorageProvider::establish_storage_agreement(
                    RuntimeOrigin::signed(1),
                    i,
                    terms,
                    sig,
                ));
            }
            // Five buckets, one per provider.
            assert_eq!(NextBucketId::<Test>::get(), 5);
        });
    }

    #[test]
    fn rejects_duplicate_agreement_when_nonce_reused_across_owners() {
        // A provider can't double-sell the same nonce, even to a different owner.
        new_test_ext().execute_with(|| {
            System::set_block_number(1);
            let settings = default_test_settings(0, None);
            let provider_pk = register_signing_provider(2, "//Provider", 5_000, settings);

            let terms1 = primary_terms(1, 10, 100, 0, 1_000, 42);
            let sig1 = sign_terms(&provider_pk, &terms1);
            assert_ok!(StorageProvider::establish_storage_agreement(
                RuntimeOrigin::signed(1),
                2,
                terms1,
                sig1,
            ));

            // Provider signs new terms for a different owner but reuses nonce 42.
            let terms2 = primary_terms(3, 10, 100, 0, 1_000, 42);
            let sig2 = sign_terms(&provider_pk, &terms2);
            assert_noop!(
                StorageProvider::establish_storage_agreement(
                    RuntimeOrigin::signed(3),
                    2,
                    terms2,
                    sig2,
                ),
                Error::<Test>::NonceAlreadyUsed
            );
        });
    }
}

mod establish_replica_agreement_tests {
    use super::*;

    /// Set up a primary bucket via `establish_storage_agreement` and return
    /// `(bucket_id, primary_provider_pk)`. The bucket id is used for the
    /// subsequent replica agreement.
    fn setup_primary_bucket() -> (BucketId, sp_core::sr25519::Public) {
        System::set_block_number(1);
        let settings = default_test_settings(0, None);
        let provider_pk = register_signing_provider(2, "//Primary", 1_000, settings);
        let terms = primary_terms(1, 100, 100, 0, 10_000, 1);
        let sig = sign_terms(&provider_pk, &terms);
        assert_ok!(StorageProvider::establish_storage_agreement(
            RuntimeOrigin::signed(1),
            2,
            terms,
            sig,
        ));
        let bucket_id = NextBucketId::<Test>::get() - 1;
        (bucket_id, provider_pk)
    }

    /// Register a replica-accepting provider and return its keypair.
    fn register_replica_provider(account: u64, seed: &str) -> sp_core::sr25519::Public {
        let settings = default_test_settings(0, Some(1));
        register_signing_provider(account, seed, 1_000, settings)
    }

    #[test]
    fn establishes_replica_agreement() {
        new_test_ext().execute_with(|| {
            let (bucket_id, _) = setup_primary_bucket();
            let replica_pk = register_replica_provider(3, "//Replica");

            let owner_balance_before = Balances::free_balance(1);
            // payment = price 0 * 50 * 100 = 0; sync_balance = 25 is reserved.
            let terms = replica_terms(1, 50, 100, 0, 10_000, 1, 25, 10);
            let sig = sign_terms(&replica_pk, &terms);
            assert_ok!(StorageProvider::establish_replica_agreement(
                RuntimeOrigin::signed(1),
                bucket_id,
                3,
                terms,
                sig,
            ));

            let agreement = StorageAgreements::<Test>::get(bucket_id, 3).unwrap();
            assert_eq!(agreement.owner, 1);
            assert_eq!(agreement.max_bytes, 50);
            match agreement.role {
                ProviderRole::Replica {
                    sync_balance,
                    sync_price,
                    min_sync_interval,
                    last_sync,
                } => {
                    assert_eq!(sync_balance, 25);
                    assert_eq!(sync_price, 1);
                    assert_eq!(min_sync_interval, 10);
                    assert!(last_sync.is_none());
                }
                ProviderRole::Primary => panic!("expected replica role"),
            }

            // Only the sync_balance is reserved (payment = 0 here).
            assert_eq!(Balances::free_balance(1), owner_balance_before - 25);

            // Replica provider's committed_bytes advanced.
            let provider = Providers::<Test>::get(3).unwrap();
            assert_eq!(provider.committed_bytes, 50);
        });
    }

    #[test]
    fn rejects_when_bucket_does_not_exist() {
        new_test_ext().execute_with(|| {
            System::set_block_number(1);
            let replica_pk = register_replica_provider(3, "//Replica");
            let terms = replica_terms(1, 50, 100, 0, 10_000, 1, 25, 10);
            let sig = sign_terms(&replica_pk, &terms);
            assert_noop!(
                StorageProvider::establish_replica_agreement(
                    RuntimeOrigin::signed(1),
                    999,
                    3,
                    terms,
                    sig,
                ),
                Error::<Test>::BucketNotFound
            );
        });
    }

    #[test]
    fn rejects_when_replica_terms_missing() {
        new_test_ext().execute_with(|| {
            let (bucket_id, _) = setup_primary_bucket();
            let replica_pk = register_replica_provider(3, "//Replica");

            // Primary-shaped terms (no replica_params) cannot drive a replica.
            let terms = primary_terms(1, 50, 100, 0, 10_000, 1);
            let sig = sign_terms(&replica_pk, &terms);
            assert_noop!(
                StorageProvider::establish_replica_agreement(
                    RuntimeOrigin::signed(1),
                    bucket_id,
                    3,
                    terms,
                    sig,
                ),
                Error::<Test>::MissingReplicaTerms
            );
        });
    }

    #[test]
    fn rejects_when_agreement_already_exists() {
        new_test_ext().execute_with(|| {
            let (bucket_id, _) = setup_primary_bucket();
            let replica_pk = register_replica_provider(3, "//Replica");

            let terms = replica_terms(1, 10, 100, 0, 10_000, 1, 5, 10);
            let sig = sign_terms(&replica_pk, &terms);
            assert_ok!(StorageProvider::establish_replica_agreement(
                RuntimeOrigin::signed(1),
                bucket_id,
                3,
                terms,
                sig,
            ));

            // Same provider, same bucket → duplicate agreement.
            let terms = replica_terms(1, 10, 100, 0, 10_000, 2, 5, 10);
            let sig = sign_terms(&replica_pk, &terms);
            assert_noop!(
                StorageProvider::establish_replica_agreement(
                    RuntimeOrigin::signed(1),
                    bucket_id,
                    3,
                    terms,
                    sig,
                ),
                Error::<Test>::AgreementAlreadyExists
            );
        });
    }

    #[test]
    fn rejects_when_provider_not_accepting_replicas() {
        new_test_ext().execute_with(|| {
            let (bucket_id, _) = setup_primary_bucket();
            // No replica_sync_price set ⇒ not accepting replicas.
            let settings = default_test_settings(0, None);
            let replica_pk = register_signing_provider(3, "//Replica", 1_000, settings);

            let terms = replica_terms(1, 50, 100, 0, 10_000, 1, 25, 10);
            let sig = sign_terms(&replica_pk, &terms);
            assert_noop!(
                StorageProvider::establish_replica_agreement(
                    RuntimeOrigin::signed(1),
                    bucket_id,
                    3,
                    terms,
                    sig,
                ),
                Error::<Test>::ProviderNotAcceptingReplicas
            );
        });
    }

    #[test]
    fn rejects_owner_mismatch() {
        new_test_ext().execute_with(|| {
            let (bucket_id, _) = setup_primary_bucket();
            let replica_pk = register_replica_provider(3, "//Replica");
            // Terms signed for owner = 1, but origin = 4.
            let terms = replica_terms(1, 50, 100, 0, 10_000, 1, 25, 10);
            let sig = sign_terms(&replica_pk, &terms);
            assert_noop!(
                StorageProvider::establish_replica_agreement(
                    RuntimeOrigin::signed(4),
                    bucket_id,
                    3,
                    terms,
                    sig,
                ),
                Error::<Test>::TermsOwnerMismatch
            );
        });
    }

    #[test]
    fn rejects_expired_replica_terms() {
        new_test_ext().execute_with(|| {
            let (bucket_id, _) = setup_primary_bucket();
            let replica_pk = register_replica_provider(3, "//Replica");

            System::set_block_number(50);
            let terms = replica_terms(1, 50, 100, 0, 10, 1, 25, 10);
            let sig = sign_terms(&replica_pk, &terms);
            assert_noop!(
                StorageProvider::establish_replica_agreement(
                    RuntimeOrigin::signed(1),
                    bucket_id,
                    3,
                    terms,
                    sig,
                ),
                Error::<Test>::TermsExpired
            );
        });
    }

    #[test]
    fn rejects_invalid_replica_signature() {
        new_test_ext().execute_with(|| {
            let (bucket_id, _) = setup_primary_bucket();
            let _replica_pk = register_replica_provider(3, "//Replica");
            let (other_pk, _) = generate_provider_public_key("//Imposter");

            let terms = replica_terms(1, 50, 100, 0, 10_000, 1, 25, 10);
            let sig = sign_terms(&other_pk, &terms);
            assert_noop!(
                StorageProvider::establish_replica_agreement(
                    RuntimeOrigin::signed(1),
                    bucket_id,
                    3,
                    terms,
                    sig,
                ),
                Error::<Test>::InvalidProviderSignature
            );
        });
    }

    #[test]
    fn replica_replay_window_rejects_reuse() {
        new_test_ext().execute_with(|| {
            let (bucket_id, _) = setup_primary_bucket();
            let replica_pk = register_replica_provider(3, "//Replica");

            let terms = replica_terms(1, 10, 100, 0, 10_000, 7, 5, 10);
            let sig = sign_terms(&replica_pk, &terms);
            assert_ok!(StorageProvider::establish_replica_agreement(
                RuntimeOrigin::signed(1),
                bucket_id,
                3,
                terms.clone(),
                sig.clone(),
            ));

            // Replay same nonce → rejected (and the duplicate check would
            // also trip, but nonce comes first since the same terms hash).
            assert_noop!(
                StorageProvider::establish_replica_agreement(
                    RuntimeOrigin::signed(1),
                    bucket_id,
                    3,
                    terms,
                    sig,
                ),
                Error::<Test>::AgreementAlreadyExists
            );
        });
    }
}

mod member_buckets_tests {
    use super::*;

    #[test]
    fn set_member_works() {
        new_test_ext().execute_with(|| {
            assert_ok!(StorageProvider::create_bucket_internal(&1, 1, None));

            // Add writer
            assert_ok!(StorageProvider::set_member(
                RuntimeOrigin::signed(1),
                0,
                2,
                Role::Writer
            ));

            let bucket = Buckets::<Test>::get(0).unwrap();
            assert_eq!(bucket.members.len(), 2);

            let writer = bucket.members.iter().find(|m| m.account == 2).unwrap();
            assert_eq!(writer.role, Role::Writer);
        });
    }

    #[test]
    fn set_member_updates_existing_role() {
        new_test_ext().execute_with(|| {
            assert_ok!(StorageProvider::create_bucket_internal(&1, 1, None));

            // Add as writer
            assert_ok!(StorageProvider::set_member(
                RuntimeOrigin::signed(1),
                0,
                2,
                Role::Writer
            ));

            // Promote to admin
            assert_ok!(StorageProvider::set_member(
                RuntimeOrigin::signed(1),
                0,
                2,
                Role::Admin
            ));

            let bucket = Buckets::<Test>::get(0).unwrap();
            let member = bucket.members.iter().find(|m| m.account == 2).unwrap();
            assert_eq!(member.role, Role::Admin);
        });
    }

    #[test]
    fn set_member_fails_for_non_admin() {
        new_test_ext().execute_with(|| {
            assert_ok!(StorageProvider::create_bucket_internal(&1, 1, None));

            // Non-admin tries to add member
            assert_noop!(
                StorageProvider::set_member(RuntimeOrigin::signed(2), 0, 3, Role::Writer),
                Error::<Test>::NotBucketAdmin
            );
        });
    }

    #[test]
    fn cannot_demote_other_admin() {
        new_test_ext().execute_with(|| {
            assert_ok!(StorageProvider::create_bucket_internal(&1, 1, None));

            // Add second admin
            assert_ok!(StorageProvider::set_member(
                RuntimeOrigin::signed(1),
                0,
                2,
                Role::Admin
            ));

            // Admin 1 tries to demote admin 2
            assert_noop!(
                StorageProvider::set_member(RuntimeOrigin::signed(1), 0, 2, Role::Writer),
                Error::<Test>::CannotDemoteAdmin
            );
        });
    }

    #[test]
    fn last_admin_cannot_self_demote() {
        new_test_ext().execute_with(|| {
            assert_ok!(StorageProvider::create_bucket_internal(&1, 1, None));

            // Admin 1 is the sole admin and cannot demote themselves.
            assert_noop!(
                StorageProvider::set_member(RuntimeOrigin::signed(1), 0, 1, Role::Writer),
                Error::<Test>::LastAdminCannotBeRemoved
            );
        });
    }

    #[test]
    fn last_admin_cannot_be_removed() {
        new_test_ext().execute_with(|| {
            assert_ok!(StorageProvider::create_bucket_internal(&1, 1, None));

            assert_noop!(
                StorageProvider::remove_member(RuntimeOrigin::signed(1), 0, 1),
                Error::<Test>::LastAdminCannotBeRemoved
            );
        });
    }

    #[test]
    fn admin_can_demote_self() {
        new_test_ext().execute_with(|| {
            assert_ok!(StorageProvider::create_bucket_internal(&1, 1, None));

            // Add second admin
            assert_ok!(StorageProvider::set_member(
                RuntimeOrigin::signed(1),
                0,
                2,
                Role::Admin
            ));

            // Admin 1 demotes self
            assert_ok!(StorageProvider::set_member(
                RuntimeOrigin::signed(1),
                0,
                1,
                Role::Writer
            ));

            let bucket = Buckets::<Test>::get(0).unwrap();
            let member = bucket.members.iter().find(|m| m.account == 1).unwrap();
            assert_eq!(member.role, Role::Writer);
        });
    }

    #[test]
    fn remove_member_works() {
        new_test_ext().execute_with(|| {
            assert_ok!(StorageProvider::create_bucket_internal(&1, 1, None));
            assert_ok!(StorageProvider::set_member(
                RuntimeOrigin::signed(1),
                0,
                2,
                Role::Writer
            ));

            assert_ok!(StorageProvider::remove_member(
                RuntimeOrigin::signed(1),
                0,
                2
            ));

            let bucket = Buckets::<Test>::get(0).unwrap();
            assert_eq!(bucket.members.len(), 1);
            assert!(!bucket.members.iter().any(|m| m.account == 2));
        });
    }

    #[test]
    fn remove_member_fails_for_non_existent() {
        new_test_ext().execute_with(|| {
            assert_ok!(StorageProvider::create_bucket_internal(&1, 1, None));

            assert_noop!(
                StorageProvider::remove_member(RuntimeOrigin::signed(1), 0, 99),
                Error::<Test>::MemberNotFound
            );
        });
    }

    #[test]
    fn set_min_providers_works() {
        new_test_ext().execute_with(|| {
            assert_ok!(StorageProvider::create_bucket_internal(&1, 2, None));

            // Can set to 0 (no minimum)
            assert_ok!(StorageProvider::set_min_providers(
                RuntimeOrigin::signed(1),
                0,
                0
            ));

            let bucket = Buckets::<Test>::get(0).unwrap();
            assert_eq!(bucket.min_providers, 0);
        });
    }

    #[test]
    fn freeze_bucket_requires_snapshot() {
        new_test_ext().execute_with(|| {
            assert_ok!(StorageProvider::create_bucket_internal(&1, 1, None));

            assert_noop!(
                StorageProvider::freeze_bucket(RuntimeOrigin::signed(1), 0),
                Error::<Test>::NoSnapshot
            );
        });
    }

    #[test]
    fn member_buckets_index_on_create() {
        new_test_ext().execute_with(|| {
            assert_ok!(StorageProvider::create_bucket_internal(&1, 1, None));
            assert_ok!(StorageProvider::create_bucket_internal(&1, 1, None));

            let member_buckets = pallet::MemberBuckets::<Test>::get(1);
            assert_eq!(member_buckets.to_vec(), vec![0, 1]);
        });
    }

    #[test]
    fn member_buckets_index_on_set_member() {
        new_test_ext().execute_with(|| {
            assert_ok!(StorageProvider::create_bucket_internal(&1, 1, None));

            // Add account 2 as writer
            assert_ok!(StorageProvider::set_member(
                RuntimeOrigin::signed(1),
                0,
                2,
                Role::Writer
            ));

            let member_buckets = pallet::MemberBuckets::<Test>::get(2);
            assert_eq!(member_buckets.to_vec(), vec![0]);

            // Updating role (not a new member) should not duplicate entry
            assert_ok!(StorageProvider::set_member(
                RuntimeOrigin::signed(1),
                0,
                2,
                Role::Reader
            ));

            let member_buckets = pallet::MemberBuckets::<Test>::get(2);
            assert_eq!(member_buckets.to_vec(), vec![0]);
        });
    }

    #[test]
    fn member_buckets_index_on_remove_member() {
        new_test_ext().execute_with(|| {
            assert_ok!(StorageProvider::create_bucket_internal(&1, 1, None));
            assert_ok!(StorageProvider::set_member(
                RuntimeOrigin::signed(1),
                0,
                2,
                Role::Writer
            ));

            // Remove account 2
            assert_ok!(StorageProvider::remove_member(
                RuntimeOrigin::signed(1),
                0,
                2
            ));

            let member_buckets = pallet::MemberBuckets::<Test>::get(2);
            assert!(member_buckets.is_empty());
        });
    }

    #[test]
    fn member_buckets_index_on_bucket_delete() {
        new_test_ext().execute_with(|| {
            assert_ok!(StorageProvider::create_bucket_internal(&1, 0, None));
            assert_ok!(StorageProvider::set_member(
                RuntimeOrigin::signed(1),
                0,
                2,
                Role::Writer
            ));
            assert_ok!(StorageProvider::set_member(
                RuntimeOrigin::signed(1),
                0,
                3,
                Role::Reader
            ));

            // Delete the bucket via internal function
            assert_ok!(StorageProvider::cleanup_bucket_internal(0, &1));

            // All members should have the bucket removed from their index
            assert!(pallet::MemberBuckets::<Test>::get(1).is_empty());
            assert!(pallet::MemberBuckets::<Test>::get(2).is_empty());
            assert!(pallet::MemberBuckets::<Test>::get(3).is_empty());
        });
    }

    #[test]
    fn member_buckets_multi_membership() {
        new_test_ext().execute_with(|| {
            // Create 3 buckets owned by different accounts
            assert_ok!(StorageProvider::create_bucket_internal(&1, 1, None));
            assert_ok!(StorageProvider::create_bucket_internal(&2, 1, None));
            assert_ok!(StorageProvider::create_bucket_internal(&3, 1, None));

            // Add account 4 to all 3 buckets
            assert_ok!(StorageProvider::set_member(
                RuntimeOrigin::signed(1),
                0,
                4,
                Role::Writer
            ));
            assert_ok!(StorageProvider::set_member(
                RuntimeOrigin::signed(2),
                1,
                4,
                Role::Reader
            ));
            assert_ok!(StorageProvider::set_member(
                RuntimeOrigin::signed(3),
                2,
                4,
                Role::Admin
            ));

            let member_buckets = pallet::MemberBuckets::<Test>::get(4);
            assert_eq!(member_buckets.to_vec(), vec![0, 1, 2]);

            // Remove from bucket 1 only
            assert_ok!(StorageProvider::remove_member(
                RuntimeOrigin::signed(2),
                1,
                4
            ));

            let member_buckets = pallet::MemberBuckets::<Test>::get(4);
            assert_eq!(member_buckets.to_vec(), vec![0, 2]);
        });
    }
}
