//! Tests for the storage provider pallet.

use crate::{mock::*, *};
use frame_support::{assert_noop, assert_ok};
use storage_primitives::{ProviderRole, Role};

/// Helper function to create a test public key (32 bytes).
fn test_public_key() -> frame_support::BoundedVec<u8, frame_support::traits::ConstU32<64>> {
    vec![1u8; 32].try_into().unwrap()
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
            let multiaddr = b"/ip4/127.0.0.1/tcp/3000".to_vec();

            assert_ok!(StorageProvider::register_provider(
                RuntimeOrigin::signed(1),
                multiaddr.try_into().unwrap(),
                test_public_key(),
                200
            ));

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
    fn withdraw_agreement_request_still_works_during_announcement() {
        // Defensive: if a request was created BEFORE announce and the
        // provider is now exiting, the owner must still be able to recover
        // their locked funds via withdraw_agreement_request. Otherwise the
        // owner's payment would be stuck until the request expires
        // (RequestTimeout) and even then there's no automatic refund path.
        new_test_ext().execute_with(|| {
            let multiaddr = b"/ip4/127.0.0.1/tcp/3000".to_vec();
            assert_ok!(StorageProvider::register_provider(
                RuntimeOrigin::signed(2),
                multiaddr.try_into().unwrap(),
                test_public_key(),
                200
            ));
            assert_ok!(StorageProvider::create_bucket(RuntimeOrigin::signed(1), 1));

            // Owner creates request → payment locked.
            assert_ok!(StorageProvider::request_primary_agreement(
                RuntimeOrigin::signed(1),
                0,
                2,
                50,
                100,
                1000
            ));

            // Provider announces deregister (without accepting).
            // committed_bytes is 0 because the request was never accepted.
            assert_ok!(StorageProvider::deregister_provider(RuntimeOrigin::signed(
                2
            )));

            // Owner can still withdraw their pending request.
            assert_ok!(StorageProvider::withdraw_agreement_request(
                RuntimeOrigin::signed(1),
                0,
                2,
            ));
            assert!(AgreementRequests::<Test>::get(0, 2).is_none());
        });
    }

    #[test]
    fn agreement_entry_points_reject_deregistering_provider() {
        new_test_ext().execute_with(|| {
            let multiaddr = b"/ip4/127.0.0.1/tcp/3000".to_vec();
            assert_ok!(StorageProvider::register_provider(
                RuntimeOrigin::signed(2),
                multiaddr.try_into().unwrap(),
                test_public_key(),
                200
            ));
            assert_ok!(StorageProvider::create_bucket(RuntimeOrigin::signed(1), 1));

            // Set up an existing agreement so top_up_agreement has something
            // to top up — this happens BEFORE announce.
            assert_ok!(StorageProvider::request_primary_agreement(
                RuntimeOrigin::signed(1),
                0,
                2,
                50,
                100,
                1000
            ));
            assert_ok!(StorageProvider::accept_agreement(
                RuntimeOrigin::signed(2),
                0
            ));

            // End the agreement so committed_bytes drops back to 0 and
            // announce is allowed. Wait past expires_at + SettlementTimeout
            // so claim_expired_agreement succeeds.
            run_to_block(200);
            assert_ok!(StorageProvider::claim_expired_agreement(
                RuntimeOrigin::signed(2),
                0
            ));

            // Now announce.
            assert_ok!(StorageProvider::deregister_provider(RuntimeOrigin::signed(
                2
            )));

            // Every agreement-creating entry point now rejects with
            // DeregisterAnnounced.
            assert_noop!(
                StorageProvider::request_primary_agreement(
                    RuntimeOrigin::signed(1),
                    0,
                    2,
                    50,
                    100,
                    1000
                ),
                Error::<Test>::DeregisterAnnounced
            );
            assert_noop!(
                StorageProvider::request_agreement(
                    RuntimeOrigin::signed(1),
                    0,
                    2,
                    50,
                    100,
                    1000,
                    storage_primitives::ReplicaRequestParams {
                        sync_balance: 0,
                        min_sync_interval: 10,
                    }
                ),
                Error::<Test>::DeregisterAnnounced
            );

            // accept_agreement: there's no pending request now, but if there
            // were, the deregister check would fire before the request
            // lookup. We simulate by inserting a dummy request via storage.
            crate::AgreementRequests::<Test>::insert(
                0,
                2,
                crate::AgreementRequest {
                    requester: 1,
                    max_bytes: 50,
                    payment_locked: 0,
                    duration: 100,
                    expires_at: 10_000,
                    replica_params: None,
                },
            );
            assert_noop!(
                StorageProvider::accept_agreement(RuntimeOrigin::signed(2), 0),
                Error::<Test>::DeregisterAnnounced
            );

            // Auto-match (find_matching_provider) skips deregistering
            // providers — request_storage finds no candidate.
            // (We don't have a public entry point that calls find_matching_provider
            // directly without other setup; the unit-level guarantee is the
            // skip branch we added at top of the loop.)
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
            // Setup provider and bucket
            let multiaddr = b"/ip4/127.0.0.1/tcp/3000".to_vec();
            assert_ok!(StorageProvider::register_provider(
                RuntimeOrigin::signed(2),
                multiaddr.try_into().unwrap(),
                test_public_key(),
                200
            ));
            assert_ok!(StorageProvider::create_bucket(RuntimeOrigin::signed(1), 1));

            // Create agreement (max_bytes = 100 fits within stake of 200)
            // payment = price_per_byte(0) * max_bytes * duration = 0
            assert_ok!(StorageProvider::request_primary_agreement(
                RuntimeOrigin::signed(1),
                0,
                2,
                100,
                100,
                1000
            ));
            assert_ok!(StorageProvider::accept_agreement(
                RuntimeOrigin::signed(2),
                0
            ));

            // Try to deregister
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
            // Setup provider and bucket
            let multiaddr = b"/ip4/127.0.0.1/tcp/3000".to_vec();
            assert_ok!(StorageProvider::register_provider(
                RuntimeOrigin::signed(2),
                multiaddr.try_into().unwrap(),
                test_public_key(),
                200
            ));
            assert_ok!(StorageProvider::create_bucket(RuntimeOrigin::signed(1), 1));

            // Create agreement for 100 bytes
            assert_ok!(StorageProvider::request_primary_agreement(
                RuntimeOrigin::signed(1),
                0,
                2,
                100, // max_bytes
                100,
                1000
            ));
            assert_ok!(StorageProvider::accept_agreement(
                RuntimeOrigin::signed(2),
                0
            ));

            // Try to set max_capacity below committed_bytes
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

            // min_duration > max_duration would silently brick the provider
            // out of `find_matching_provider`; reject it at the entry point.
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
            let multiaddr = b"/ip4/127.0.0.1/tcp/3000".to_vec();
            assert_ok!(StorageProvider::register_provider(
                RuntimeOrigin::signed(2),
                multiaddr.try_into().unwrap(),
                test_public_key(),
                200
            ));
            assert_ok!(StorageProvider::create_bucket(RuntimeOrigin::signed(1), 1));
            assert_ok!(StorageProvider::request_primary_agreement(
                RuntimeOrigin::signed(1),
                0,
                2,
                100,
                100,
                1000
            ));
            assert_ok!(StorageProvider::accept_agreement(
                RuntimeOrigin::signed(2),
                0
            ));

            assert_ok!(StorageProvider::set_extensions_blocked(
                RuntimeOrigin::signed(2),
                0,
                true
            ));
            let agreement = StorageAgreements::<Test>::get(0, 2).unwrap();
            assert!(agreement.extensions_blocked);

            assert_ok!(StorageProvider::set_extensions_blocked(
                RuntimeOrigin::signed(2),
                0,
                false
            ));
            let agreement = StorageAgreements::<Test>::get(0, 2).unwrap();
            assert!(!agreement.extensions_blocked);
        });
    }

    #[test]
    fn set_extensions_blocked_fails_after_agreement_expires() {
        new_test_ext().execute_with(|| {
            let multiaddr = b"/ip4/127.0.0.1/tcp/3000".to_vec();
            assert_ok!(StorageProvider::register_provider(
                RuntimeOrigin::signed(2),
                multiaddr.try_into().unwrap(),
                test_public_key(),
                200
            ));
            assert_ok!(StorageProvider::create_bucket(RuntimeOrigin::signed(1), 1));
            assert_ok!(StorageProvider::request_primary_agreement(
                RuntimeOrigin::signed(1),
                0,
                2,
                100,
                100, // duration = 100 → expires_at = current_block + 100
                1000
            ));
            assert_ok!(StorageProvider::accept_agreement(
                RuntimeOrigin::signed(2),
                0
            ));

            let agreement = StorageAgreements::<Test>::get(0, 2).unwrap();

            // At expires_at exactly, the agreement is no longer extendable
            // (strict `<` in the pallet guard).
            run_to_block(agreement.expires_at);
            assert_noop!(
                StorageProvider::set_extensions_blocked(RuntimeOrigin::signed(2), 0, true),
                Error::<Test>::AgreementExpired
            );

            // Past expiry, same rejection.
            run_to_block(agreement.expires_at + 1);
            assert_noop!(
                StorageProvider::set_extensions_blocked(RuntimeOrigin::signed(2), 0, true),
                Error::<Test>::AgreementExpired
            );
        });
    }

    #[test]
    fn accept_agreement_fails_when_capacity_exceeded() {
        new_test_ext().execute_with(|| {
            // Setup provider with limited capacity
            let multiaddr = b"/ip4/127.0.0.1/tcp/3000".to_vec();
            assert_ok!(StorageProvider::register_provider(
                RuntimeOrigin::signed(2),
                multiaddr.try_into().unwrap(),
                test_public_key(),
                200
            ));

            // Set max_capacity to 50 bytes (stake of 200 can back this)
            let settings = ProviderSettings {
                min_duration: 0u64,
                max_duration: 1000u64,
                price_per_byte: 1u64,
                accepting_primary: true,
                replica_sync_price: None,
                accepting_extensions: true,
                max_capacity: 50,
            };
            assert_ok!(StorageProvider::update_provider_settings(
                RuntimeOrigin::signed(2),
                settings
            ));

            // Create bucket
            assert_ok!(StorageProvider::create_bucket(RuntimeOrigin::signed(1), 1));

            // Request agreement for 60 bytes (exceeds max_capacity of 50)
            // payment = price_per_byte * max_bytes * duration = 1 * 60 * 10 = 600
            assert_ok!(StorageProvider::request_primary_agreement(
                RuntimeOrigin::signed(1),
                0,
                2,
                60, // Exceeds max_capacity of 50
                10,
                600
            ));

            // Accept should fail due to capacity exceeded
            assert_noop!(
                StorageProvider::accept_agreement(RuntimeOrigin::signed(2), 0),
                Error::<Test>::CapacityExceeded
            );
        });
    }

    #[test]
    fn accept_agreement_works_with_unlimited_capacity() {
        new_test_ext().execute_with(|| {
            // Setup provider with unlimited capacity (max_capacity = 0)
            let multiaddr = b"/ip4/127.0.0.1/tcp/3000".to_vec();
            assert_ok!(StorageProvider::register_provider(
                RuntimeOrigin::signed(2),
                multiaddr.try_into().unwrap(),
                test_public_key(),
                200
            ));

            // Settings with unlimited capacity (default)
            // Default max_capacity is 0 which means unlimited

            // Create bucket
            assert_ok!(StorageProvider::create_bucket(RuntimeOrigin::signed(1), 1));

            // Request agreement for 100 bytes
            // payment = 1 * 100 * 10 = 1000
            assert_ok!(StorageProvider::request_primary_agreement(
                RuntimeOrigin::signed(1),
                0,
                2,
                100,
                10,
                1000
            ));

            // Accept should succeed (capacity is unlimited, stake of 200 covers 100 bytes)
            assert_ok!(StorageProvider::accept_agreement(
                RuntimeOrigin::signed(2),
                0
            ));

            let provider = Providers::<Test>::get(2).unwrap();
            assert_eq!(provider.committed_bytes, 100);
        });
    }

    #[test]
    fn accept_agreement_works_within_capacity() {
        new_test_ext().execute_with(|| {
            // Setup provider with limited capacity
            let multiaddr = b"/ip4/127.0.0.1/tcp/3000".to_vec();
            assert_ok!(StorageProvider::register_provider(
                RuntimeOrigin::signed(2),
                multiaddr.try_into().unwrap(),
                test_public_key(),
                200
            ));

            // Set max_capacity to 150 bytes (stake of 200 covers this)
            let settings = ProviderSettings {
                min_duration: 0u64,
                max_duration: 1000u64,
                price_per_byte: 1u64,
                accepting_primary: true,
                replica_sync_price: None,
                accepting_extensions: true,
                max_capacity: 150,
            };
            assert_ok!(StorageProvider::update_provider_settings(
                RuntimeOrigin::signed(2),
                settings
            ));

            // Create bucket
            assert_ok!(StorageProvider::create_bucket(RuntimeOrigin::signed(1), 1));

            // Request agreement for 100 bytes (within capacity)
            // payment = 1 * 100 * 10 = 1000
            assert_ok!(StorageProvider::request_primary_agreement(
                RuntimeOrigin::signed(1),
                0,
                2,
                100,
                10,
                1000
            ));

            // Accept should succeed
            assert_ok!(StorageProvider::accept_agreement(
                RuntimeOrigin::signed(2),
                0
            ));

            let provider = Providers::<Test>::get(2).unwrap();
            assert_eq!(provider.committed_bytes, 100);
            assert_eq!(provider.settings.max_capacity, 150);
        });
    }
}

mod bucket_tests {
    use super::*;

    #[test]
    fn create_bucket_works() {
        new_test_ext().execute_with(|| {
            assert_ok!(StorageProvider::create_bucket(RuntimeOrigin::signed(1), 2));

            let bucket = Buckets::<Test>::get(0).unwrap();
            assert_eq!(bucket.min_providers, 2);
            assert_eq!(bucket.members.len(), 1);
            assert_eq!(bucket.members[0].account, 1);
            assert_eq!(bucket.members[0].role, Role::Admin);
            assert!(bucket.snapshot.is_none());
            assert!(bucket.frozen_start_seq.is_none());

            // Check bucket ID incremented
            assert_eq!(NextBucketId::<Test>::get(), 1);
        });
    }

    #[test]
    fn create_multiple_buckets_increments_id() {
        new_test_ext().execute_with(|| {
            assert_ok!(StorageProvider::create_bucket(RuntimeOrigin::signed(1), 1));
            assert_ok!(StorageProvider::create_bucket(RuntimeOrigin::signed(2), 2));
            assert_ok!(StorageProvider::create_bucket(RuntimeOrigin::signed(1), 3));

            assert_eq!(NextBucketId::<Test>::get(), 3);
            assert!(Buckets::<Test>::get(0).is_some());
            assert!(Buckets::<Test>::get(1).is_some());
            assert!(Buckets::<Test>::get(2).is_some());
        });
    }

    #[test]
    fn set_member_works() {
        new_test_ext().execute_with(|| {
            assert_ok!(StorageProvider::create_bucket(RuntimeOrigin::signed(1), 1));

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
            assert_ok!(StorageProvider::create_bucket(RuntimeOrigin::signed(1), 1));

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
            assert_ok!(StorageProvider::create_bucket(RuntimeOrigin::signed(1), 1));

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
            assert_ok!(StorageProvider::create_bucket(RuntimeOrigin::signed(1), 1));

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
    fn admin_can_demote_self() {
        new_test_ext().execute_with(|| {
            assert_ok!(StorageProvider::create_bucket(RuntimeOrigin::signed(1), 1));

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
            assert_ok!(StorageProvider::create_bucket(RuntimeOrigin::signed(1), 1));
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
            assert_ok!(StorageProvider::create_bucket(RuntimeOrigin::signed(1), 1));

            assert_noop!(
                StorageProvider::remove_member(RuntimeOrigin::signed(1), 0, 99),
                Error::<Test>::MemberNotFound
            );
        });
    }

    #[test]
    fn set_min_providers_works() {
        new_test_ext().execute_with(|| {
            assert_ok!(StorageProvider::create_bucket(RuntimeOrigin::signed(1), 2));

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
            assert_ok!(StorageProvider::create_bucket(RuntimeOrigin::signed(1), 1));

            assert_noop!(
                StorageProvider::freeze_bucket(RuntimeOrigin::signed(1), 0),
                Error::<Test>::NoSnapshot
            );
        });
    }
}

mod agreement_tests {
    use super::*;

    fn setup_provider_and_bucket() {
        let multiaddr = b"/ip4/127.0.0.1/tcp/3000".to_vec();
        assert_ok!(StorageProvider::register_provider(
            RuntimeOrigin::signed(2),
            multiaddr.try_into().unwrap(),
            test_public_key(),
            200
        ));
        assert_ok!(StorageProvider::create_bucket(RuntimeOrigin::signed(1), 1));
    }

    #[test]
    fn request_primary_agreement_works() {
        new_test_ext().execute_with(|| {
            setup_provider_and_bucket();

            assert_ok!(StorageProvider::request_primary_agreement(
                RuntimeOrigin::signed(1),
                0,    // bucket_id
                2,    // provider
                1000, // max_bytes
                100,  // duration
                1000  // max_payment
            ));

            let request = AgreementRequests::<Test>::get(0, 2).unwrap();
            assert_eq!(request.requester, 1);
            assert_eq!(request.max_bytes, 1000);
            assert_eq!(request.duration, 100);
            assert!(request.replica_params.is_none());
        });
    }

    #[test]
    fn request_primary_agreement_fails_for_non_admin() {
        new_test_ext().execute_with(|| {
            setup_provider_and_bucket();

            assert_noop!(
                StorageProvider::request_primary_agreement(
                    RuntimeOrigin::signed(3), // Not admin
                    0,
                    2,
                    1000,
                    100,
                    1000
                ),
                Error::<Test>::NotBucketAdmin
            );
        });
    }

    #[test]
    fn accept_agreement_works() {
        new_test_ext().execute_with(|| {
            setup_provider_and_bucket();

            // max_bytes = 100 fits within stake of 200 (MinStakePerByte = 1)
            // payment = price_per_byte(0) * max_bytes * duration = 0
            assert_ok!(StorageProvider::request_primary_agreement(
                RuntimeOrigin::signed(1),
                0,
                2,
                100,
                100,
                1000
            ));

            assert_ok!(StorageProvider::accept_agreement(
                RuntimeOrigin::signed(2),
                0
            ));

            // Check agreement created
            let agreement = StorageAgreements::<Test>::get(0, 2).unwrap();
            assert_eq!(agreement.owner, 1);
            assert_eq!(agreement.max_bytes, 100);
            assert!(matches!(agreement.role, ProviderRole::Primary));

            // Check provider added to bucket
            let bucket = Buckets::<Test>::get(0).unwrap();
            assert!(bucket.primary_providers.contains(&2));

            // Check provider stats updated
            let provider = Providers::<Test>::get(2).unwrap();
            assert_eq!(provider.committed_bytes, 100);
            assert_eq!(provider.stats.agreements_total, 1);

            // Check request removed
            assert!(AgreementRequests::<Test>::get(0, 2).is_none());
        });
    }

    #[test]
    fn reject_agreement_returns_funds() {
        new_test_ext().execute_with(|| {
            // Setup provider with non-zero price so funds are actually reserved
            let multiaddr = b"/ip4/127.0.0.1/tcp/3000".to_vec();
            assert_ok!(StorageProvider::register_provider(
                RuntimeOrigin::signed(2),
                multiaddr.try_into().unwrap(),
                test_public_key(),
                200
            ));

            let settings = ProviderSettings {
                min_duration: 0u64,
                max_duration: 1000u64,
                price_per_byte: 1u64, // Non-zero price
                accepting_primary: true,
                replica_sync_price: None,
                accepting_extensions: true,
                max_capacity: 0,
            };
            assert_ok!(StorageProvider::update_provider_settings(
                RuntimeOrigin::signed(2),
                settings
            ));

            assert_ok!(StorageProvider::create_bucket(RuntimeOrigin::signed(1), 1));

            let balance_before = Balances::free_balance(1);

            // Request agreement for 100 bytes at price 1, duration 10
            // payment = 1 * 100 * 10 = 1000
            assert_ok!(StorageProvider::request_primary_agreement(
                RuntimeOrigin::signed(1),
                0,
                2,
                100,
                10,
                1000
            ));

            // Some funds should be reserved (1000)
            assert!(Balances::free_balance(1) < balance_before);
            assert_eq!(Balances::free_balance(1), balance_before - 1000);

            assert_ok!(StorageProvider::reject_agreement(
                RuntimeOrigin::signed(2),
                0
            ));

            // Funds should be returned
            assert_eq!(Balances::free_balance(1), balance_before);

            // Request should be removed
            assert!(AgreementRequests::<Test>::get(0, 2).is_none());
        });
    }

    #[test]
    fn withdraw_agreement_request_works() {
        new_test_ext().execute_with(|| {
            setup_provider_and_bucket();

            let balance_before = Balances::free_balance(1);

            assert_ok!(StorageProvider::request_primary_agreement(
                RuntimeOrigin::signed(1),
                0,
                2,
                1000,
                100,
                1000
            ));

            assert_ok!(StorageProvider::withdraw_agreement_request(
                RuntimeOrigin::signed(1),
                0,
                2
            ));

            // Funds returned
            assert_eq!(Balances::free_balance(1), balance_before);

            // Request removed
            assert!(AgreementRequests::<Test>::get(0, 2).is_none());
        });
    }

    #[test]
    fn withdraw_fails_for_non_requester() {
        new_test_ext().execute_with(|| {
            setup_provider_and_bucket();

            assert_ok!(StorageProvider::request_primary_agreement(
                RuntimeOrigin::signed(1),
                0,
                2,
                1000,
                100,
                1000
            ));

            assert_noop!(
                StorageProvider::withdraw_agreement_request(
                    RuntimeOrigin::signed(3), // Not the requester
                    0,
                    2
                ),
                Error::<Test>::NotAgreementOwner
            );
        });
    }

    #[test]
    fn max_primary_providers_enforced() {
        new_test_ext().execute_with(|| {
            assert_ok!(StorageProvider::create_bucket(RuntimeOrigin::signed(1), 1));

            // Register 6 providers (max is 5)
            for i in 2..=7 {
                let multiaddr = format!("/ip4/127.0.0.1/tcp/{}", 3000 + i);
                assert_ok!(StorageProvider::register_provider(
                    RuntimeOrigin::signed(i),
                    multiaddr.as_bytes().to_vec().try_into().unwrap(),
                    test_public_key(),
                    200
                ));
            }

            // Add 5 providers (should all succeed)
            for i in 2..=6 {
                assert_ok!(StorageProvider::request_primary_agreement(
                    RuntimeOrigin::signed(1),
                    0,
                    i,
                    100,
                    100,
                    1000
                ));
                assert_ok!(StorageProvider::accept_agreement(
                    RuntimeOrigin::signed(i),
                    0
                ));
            }

            // 6th provider should fail
            assert_noop!(
                StorageProvider::request_primary_agreement(
                    RuntimeOrigin::signed(1),
                    0,
                    7,
                    100,
                    100,
                    1000
                ),
                Error::<Test>::MaxPrimaryProvidersReached
            );
        });
    }
}

mod member_buckets_tests {
    use super::*;

    #[test]
    fn member_buckets_index_on_create() {
        new_test_ext().execute_with(|| {
            assert_ok!(StorageProvider::create_bucket(RuntimeOrigin::signed(1), 1));
            assert_ok!(StorageProvider::create_bucket(RuntimeOrigin::signed(1), 1));

            let member_buckets = pallet::MemberBuckets::<Test>::get(1);
            assert_eq!(member_buckets.to_vec(), vec![0, 1]);
        });
    }

    #[test]
    fn member_buckets_index_on_set_member() {
        new_test_ext().execute_with(|| {
            assert_ok!(StorageProvider::create_bucket(RuntimeOrigin::signed(1), 1));

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
            assert_ok!(StorageProvider::create_bucket(RuntimeOrigin::signed(1), 1));
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
            assert_ok!(StorageProvider::create_bucket(RuntimeOrigin::signed(1), 0));
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
            assert_ok!(StorageProvider::create_bucket(RuntimeOrigin::signed(1), 1));
            assert_ok!(StorageProvider::create_bucket(RuntimeOrigin::signed(2), 1));
            assert_ok!(StorageProvider::create_bucket(RuntimeOrigin::signed(3), 1));

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

mod auto_matching_tests {
    use super::*;

    #[test]
    fn create_bucket_with_storage_works() {
        new_test_ext().execute_with(|| {
            // Register a provider with accepting_primary: true
            let multiaddr = b"/ip4/127.0.0.1/tcp/3000".to_vec();
            assert_ok!(StorageProvider::register_provider(
                RuntimeOrigin::signed(2),
                multiaddr.try_into().unwrap(),
                test_public_key(),
                200
            ));

            // Update settings to accept primary agreements
            // Use price_per_byte: 0 like other tests to avoid balance issues
            let settings = ProviderSettings {
                min_duration: 10u64,
                max_duration: 1000u64,
                price_per_byte: 0u64, // Free storage (like other tests)
                accepting_primary: true,
                replica_sync_price: None,
                accepting_extensions: true,
                max_capacity: 200,
            };
            assert_ok!(StorageProvider::update_provider_settings(
                RuntimeOrigin::signed(2),
                settings
            ));

            // Create bucket with storage requirements
            assert_ok!(StorageProvider::create_bucket_with_storage(
                RuntimeOrigin::signed(1),
                100, // max_bytes
                100, // duration
                10   // max_price_per_byte (higher than provider's price of 0)
            ));

            // Verify bucket was created
            let bucket = Buckets::<Test>::get(0).unwrap();
            assert_eq!(bucket.min_providers, 1);
            assert_eq!(bucket.primary_providers.len(), 1);
            assert_eq!(bucket.primary_providers[0], 2);

            // Verify agreement was created
            let agreement = StorageAgreements::<Test>::get(0, 2).unwrap();
            assert_eq!(agreement.max_bytes, 100);
            assert_eq!(agreement.owner, 1);

            // Verify provider's committed_bytes was updated
            let provider = Providers::<Test>::get(2).unwrap();
            assert_eq!(provider.committed_bytes, 100);
        });
    }

    #[test]
    fn create_bucket_with_storage_fails_no_matching_provider() {
        new_test_ext().execute_with(|| {
            // No providers registered
            assert_noop!(
                StorageProvider::create_bucket_with_storage(RuntimeOrigin::signed(1), 100, 100, 10),
                Error::<Test>::NoMatchingProvider
            );
        });
    }

    #[test]
    fn create_bucket_with_storage_fails_provider_not_accepting() {
        new_test_ext().execute_with(|| {
            // Register a provider but don't set accepting_primary: true
            let multiaddr = b"/ip4/127.0.0.1/tcp/3000".to_vec();
            assert_ok!(StorageProvider::register_provider(
                RuntimeOrigin::signed(2),
                multiaddr.try_into().unwrap(),
                test_public_key(),
                200
            ));

            // Settings have accepting_primary: false by default (need to explicitly enable)
            // Since default is accepting_primary: true, let's set it to false
            let settings = ProviderSettings {
                min_duration: 10u64,
                max_duration: 1000u64,
                price_per_byte: 1u64,
                accepting_primary: false, // Not accepting
                replica_sync_price: None,
                accepting_extensions: true,
                max_capacity: 200,
            };
            assert_ok!(StorageProvider::update_provider_settings(
                RuntimeOrigin::signed(2),
                settings
            ));

            assert_noop!(
                StorageProvider::create_bucket_with_storage(RuntimeOrigin::signed(1), 100, 100, 10),
                Error::<Test>::NoMatchingProvider
            );
        });
    }

    #[test]
    fn create_bucket_with_storage_fails_price_too_high() {
        new_test_ext().execute_with(|| {
            let multiaddr = b"/ip4/127.0.0.1/tcp/3000".to_vec();
            assert_ok!(StorageProvider::register_provider(
                RuntimeOrigin::signed(2),
                multiaddr.try_into().unwrap(),
                test_public_key(),
                200
            ));

            // Provider with high price
            let settings = ProviderSettings {
                min_duration: 10u64,
                max_duration: 1000u64,
                price_per_byte: 100u64, // Very high price
                accepting_primary: true,
                replica_sync_price: None,
                accepting_extensions: true,
                max_capacity: 200,
            };
            assert_ok!(StorageProvider::update_provider_settings(
                RuntimeOrigin::signed(2),
                settings
            ));

            // User's max_price_per_byte is lower than provider's price
            assert_noop!(
                StorageProvider::create_bucket_with_storage(
                    RuntimeOrigin::signed(1),
                    100,
                    100,
                    10 // max_price_per_byte is 10, but provider charges 100
                ),
                Error::<Test>::NoMatchingProvider
            );
        });
    }

    #[test]
    fn create_bucket_with_storage_fails_insufficient_capacity() {
        new_test_ext().execute_with(|| {
            let multiaddr = b"/ip4/127.0.0.1/tcp/3000".to_vec();
            assert_ok!(StorageProvider::register_provider(
                RuntimeOrigin::signed(2),
                multiaddr.try_into().unwrap(),
                test_public_key(),
                200
            ));

            let settings = ProviderSettings {
                min_duration: 10u64,
                max_duration: 1000u64,
                price_per_byte: 1u64,
                accepting_primary: true,
                replica_sync_price: None,
                accepting_extensions: true,
                max_capacity: 50, // Only 50 bytes capacity
            };
            assert_ok!(StorageProvider::update_provider_settings(
                RuntimeOrigin::signed(2),
                settings
            ));

            // Request 100 bytes, but provider only has 50
            assert_noop!(
                StorageProvider::create_bucket_with_storage(
                    RuntimeOrigin::signed(1),
                    100, // Needs 100 bytes
                    100,
                    10
                ),
                Error::<Test>::NoMatchingProvider
            );
        });
    }

    #[test]
    fn create_bucket_with_storage_fails_duration_mismatch() {
        new_test_ext().execute_with(|| {
            let multiaddr = b"/ip4/127.0.0.1/tcp/3000".to_vec();
            assert_ok!(StorageProvider::register_provider(
                RuntimeOrigin::signed(2),
                multiaddr.try_into().unwrap(),
                test_public_key(),
                200
            ));

            let settings = ProviderSettings {
                min_duration: 500u64, // Minimum 500 blocks
                max_duration: 1000u64,
                price_per_byte: 1u64,
                accepting_primary: true,
                replica_sync_price: None,
                accepting_extensions: true,
                max_capacity: 200,
            };
            assert_ok!(StorageProvider::update_provider_settings(
                RuntimeOrigin::signed(2),
                settings
            ));

            // Request only 100 blocks, but provider requires minimum 500
            assert_noop!(
                StorageProvider::create_bucket_with_storage(
                    RuntimeOrigin::signed(1),
                    100,
                    100, // Duration of 100, below provider's min of 500
                    10
                ),
                Error::<Test>::NoMatchingProvider
            );
        });
    }

    #[test]
    fn create_bucket_with_storage_selects_cheapest_provider() {
        new_test_ext().execute_with(|| {
            // Register two providers with different prices
            let multiaddr = b"/ip4/127.0.0.1/tcp/3000".to_vec();

            // Provider 2: expensive (price = 5) - but still affordable
            assert_ok!(StorageProvider::register_provider(
                RuntimeOrigin::signed(2),
                multiaddr.clone().try_into().unwrap(),
                test_public_key(),
                200
            ));
            let settings_expensive = ProviderSettings {
                min_duration: 10u64,
                max_duration: 1000u64,
                price_per_byte: 5u64,
                accepting_primary: true,
                replica_sync_price: None,
                accepting_extensions: true,
                max_capacity: 200,
            };
            assert_ok!(StorageProvider::update_provider_settings(
                RuntimeOrigin::signed(2),
                settings_expensive
            ));

            // Provider 3: cheap (price = 0)
            assert_ok!(StorageProvider::register_provider(
                RuntimeOrigin::signed(3),
                multiaddr.try_into().unwrap(),
                test_public_key(),
                200
            ));
            let settings_cheap = ProviderSettings {
                min_duration: 10u64,
                max_duration: 1000u64,
                price_per_byte: 0u64, // Free
                accepting_primary: true,
                replica_sync_price: None,
                accepting_extensions: true,
                max_capacity: 200,
            };
            assert_ok!(StorageProvider::update_provider_settings(
                RuntimeOrigin::signed(3),
                settings_cheap
            ));

            // Create bucket - should match with cheaper provider (3)
            // Use small values to keep payment low: 10 * 10 * 5 = 500 max
            assert_ok!(StorageProvider::create_bucket_with_storage(
                RuntimeOrigin::signed(1),
                10, // max_bytes
                10, // duration
                10  // max_price_per_byte
            ));

            // Verify matched with provider 3 (the cheaper one)
            let bucket = Buckets::<Test>::get(0).unwrap();
            assert_eq!(bucket.primary_providers[0], 3);
        });
    }
}
