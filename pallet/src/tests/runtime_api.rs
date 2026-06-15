use super::*;
use codec::Encode;
use sp_core::H256;
use storage_primitives::BucketSnapshot;

#[test]
fn query_provider_info_returns_data() {
    new_test_ext().execute_with(|| {
        register_provider(2, 200);

        let info = StorageProvider::query_provider_info(&2);
        assert!(info.is_some());
        let info = info.unwrap();
        assert_eq!(info.stake, 200);
        assert_eq!(info.committed_bytes, 0);
        assert!(info.accepting_primary);
    });
}

#[test]
fn query_provider_info_none_for_unknown() {
    new_test_ext().execute_with(|| {
        let info = StorageProvider::query_provider_info(&99);
        assert!(info.is_none());
    });
}

#[test]
fn query_bucket_info_returns_data() {
    new_test_ext().execute_with(|| {
        register_provider(2, 200);
        let bucket_id = setup_agreement(2, 1, 50, 200);

        // Insert snapshot
        Buckets::<Test>::mutate(bucket_id, |maybe_bucket| {
            if let Some(bucket) = maybe_bucket {
                bucket.snapshot = Some(BucketSnapshot {
                    mmr_root: H256::repeat_byte(0xAB),
                    start_seq: 0,
                    leaf_count: 10,
                    checkpoint_block: 1,
                    primary_signers: vec![0x01],
                });
            }
        });

        let response = StorageProvider::query_bucket_info(bucket_id);
        assert!(response.is_some());
        let response = response.unwrap();
        assert_eq!(response.bucket_id, bucket_id);
        assert_eq!(response.min_providers, 1);
        assert!(!response.members.is_empty());
        assert!(response.snapshot.is_some());
        let snapshot = response.snapshot.unwrap();
        assert_eq!(snapshot.mmr_root, H256::repeat_byte(0xAB));
        assert_eq!(snapshot.leaf_count, 10);

        // Primary providers should include provider 2
        assert!(!response.primary_providers.is_empty());
        assert!(response.primary_providers.contains(&2u64.encode()));
    });
}

#[test]
fn query_bucket_info_none_for_unknown() {
    new_test_ext().execute_with(|| {
        let response = StorageProvider::query_bucket_info(999);
        assert!(response.is_none());
    });
}

#[test]
fn query_agreement_info_returns_data() {
    new_test_ext().execute_with(|| {
        register_provider(2, 200);
        let bucket_id = setup_agreement(2, 1, 50, 200);

        let response = StorageProvider::query_agreement_info(bucket_id, &2);
        assert!(response.is_some());
        let response = response.unwrap();
        assert_eq!(response.max_bytes, 50);
        assert_eq!(response.owner, 1u64.encode());
        assert_eq!(response.provider, 2u64.encode());
        assert!(!response.extensions_blocked);
        assert!(matches!(
            response.role,
            storage_primitives::ProviderRole::Primary
        ));
    });
}

#[test]
fn query_agreement_info_none_for_unknown() {
    new_test_ext().execute_with(|| {
        let response = StorageProvider::query_agreement_info(999, &2);
        assert!(response.is_none());
    });
}

#[test]
fn can_accept_bytes_checks_capacity() {
    new_test_ext().execute_with(|| {
        // Provider with max_capacity set
        register_provider_with_settings(
            2,
            200,
            ProviderSettings {
                accepting_primary: true,
                max_capacity: 100,
                ..Default::default()
            },
        );

        // Can accept 50 bytes (well within capacity and stake covers it)
        assert!(StorageProvider::query_can_accept_bytes(&2, 50));

        // Can accept 100 bytes (at capacity limit)
        assert!(StorageProvider::query_can_accept_bytes(&2, 100));

        // Cannot accept 101 bytes (exceeds capacity)
        assert!(!StorageProvider::query_can_accept_bytes(&2, 101));

        // Non-existent provider
        assert!(!StorageProvider::query_can_accept_bytes(&99, 10));
    });
}

#[test]
fn can_accept_bytes_unlimited_capacity() {
    new_test_ext().execute_with(|| {
        // Provider with max_capacity = 0 (unlimited)
        register_provider(2, 200);

        // Unlimited capacity — only stake constraint matters
        // stake = 200, MinStakePerByte = 1, so can accept up to 200 bytes
        assert!(StorageProvider::query_can_accept_bytes(&2, 100));
        assert!(StorageProvider::query_can_accept_bytes(&2, 200));
        assert!(!StorageProvider::query_can_accept_bytes(&2, 201));
    });
}

#[test]
fn find_matching_providers_scores() {
    new_test_ext().execute_with(|| {
        // Register a provider that matches requirements
        // stake >= max_capacity * MinStakePerByte(1) → need stake >= 1000
        register_provider_with_settings(
            2,
            1000,
            ProviderSettings {
                accepting_primary: true,
                max_capacity: 1000,
                price_per_byte: 5,
                min_duration: 10,
                max_duration: 500,
                ..Default::default()
            },
        );

        let requirements = crate::runtime_api::StorageRequirements {
            bytes_needed: 100,
            min_duration: 50,
            max_price_per_byte: 10,
            primary_only: true,
        };

        let results = StorageProvider::query_find_matching_providers(requirements, 10);
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].match_score, 100); // Perfect match
        assert!(results[0].partial_reason.is_none());
    });
}

#[test]
fn find_matching_providers_partial_score() {
    new_test_ext().execute_with(|| {
        // Register a provider with price too high
        // stake >= max_capacity * MinStakePerByte(1) → need stake >= 1000
        register_provider_with_settings(
            2,
            1000,
            ProviderSettings {
                accepting_primary: true,
                max_capacity: 1000,
                price_per_byte: 20,
                min_duration: 10,
                max_duration: 500,
                ..Default::default()
            },
        );

        let requirements = crate::runtime_api::StorageRequirements {
            bytes_needed: 100,
            min_duration: 50,
            max_price_per_byte: 10, // Provider price is 20, exceeds this
            primary_only: true,
        };

        let results = StorageProvider::query_find_matching_providers(requirements, 10);
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].match_score, 70); // 100 - 30 for price
        assert_eq!(
            results[0].partial_reason,
            Some(crate::runtime_api::PartialMatchReason::PriceTooHigh)
        );
    });
}

#[test]
fn find_matching_providers_not_accepting() {
    new_test_ext().execute_with(|| {
        // Register a provider that is NOT accepting primary
        register_provider_with_settings(
            2,
            200,
            ProviderSettings {
                accepting_primary: false,
                ..Default::default()
            },
        );

        let requirements = crate::runtime_api::StorageRequirements {
            bytes_needed: 10,
            min_duration: 10,
            max_price_per_byte: 100,
            primary_only: true,
        };

        let results = StorageProvider::query_find_matching_providers(requirements, 10);
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].match_score, 0); // Not accepting = score 0
    });
}

#[test]
fn query_bucket_providers_returns_list() {
    new_test_ext().execute_with(|| {
        register_provider(2, 200);
        let bucket_id = setup_agreement(2, 1, 50, 200);

        let providers = StorageProvider::query_bucket_providers(bucket_id);
        assert_eq!(providers, vec![2]);
    });
}

// ────────────────────────────────────────────────────────────────────────────
// T4 — runtime-API query helpers previously untested
// ────────────────────────────────────────────────────────────────────────────

#[test]
fn query_providers_returns_paginated_list() {
    // Covers query_providers (lib.rs 3290-3334): pagination + available_capacity
    // for both the unlimited (None) and limited (Some) branches.
    new_test_ext().execute_with(|| {
        // Provider 2: unlimited capacity (max_capacity = 0 default)
        register_provider(2, 200);
        // Provider 3: limited capacity; stake must cover max_capacity.
        register_provider_with_settings(
            3,
            500,
            ProviderSettings {
                accepting_primary: true,
                max_capacity: 300,
                ..Default::default()
            },
        );

        let all = StorageProvider::query_providers(0, 10);
        assert_eq!(all.len(), 2);

        // Pagination: limit=1 returns exactly one result.
        let page = StorageProvider::query_providers(0, 1);
        assert_eq!(page.len(), 1);

        // Unlimited capacity → None; limited → Some(remaining).
        let p2 = all.iter().find(|(a, _)| *a == 2).map(|(_, i)| i).unwrap();
        assert!(p2.available_capacity.is_none()); // max_capacity = 0

        let p3 = all.iter().find(|(a, _)| *a == 3).map(|(_, i)| i).unwrap();
        assert_eq!(p3.available_capacity, Some(300)); // 300 capacity, 0 committed
    });
}

#[test]
fn query_providers_with_capacity_filters_and_available_capacity() {
    // Covers query_providers_with_capacity (lib.rs 4034-4106): all three filter
    // exit points (not accepting, capacity too low, stake too low) and the
    // available_capacity Some/None branches in the output mapping.
    new_test_ext().execute_with(|| {
        // 2: not accepting primary, no replica_sync_price → excluded
        register_provider_with_settings(
            2,
            200,
            ProviderSettings {
                accepting_primary: false,
                replica_sync_price: None,
                max_capacity: 200,
                ..Default::default()
            },
        );
        // 3: accepting but max_capacity (100) < bytes_needed (150) → excluded
        register_provider_with_settings(
            3,
            200,
            ProviderSettings {
                accepting_primary: true,
                max_capacity: 100,
                ..Default::default()
            },
        );
        // 4: accepting, unlimited capacity, but stake (100) < required (150) → excluded
        register_provider_with_settings(
            4,
            100,
            ProviderSettings {
                accepting_primary: true,
                max_capacity: 0,
                ..Default::default()
            },
        );
        // 5: accepting, limited capacity (200), stake (200) → included; Some(200)
        register_provider_with_settings(
            5,
            200,
            ProviderSettings {
                accepting_primary: true,
                max_capacity: 200,
                ..Default::default()
            },
        );
        // 6: accepting, unlimited capacity, stake (500) → included; None
        register_provider_with_settings(
            6,
            500,
            ProviderSettings {
                accepting_primary: true,
                max_capacity: 0,
                ..Default::default()
            },
        );

        let results = StorageProvider::query_providers_with_capacity(150, 0, 10);
        assert_eq!(results.len(), 2);

        let p5 = results.iter().find(|(a, _)| *a == 5).map(|(_, i)| i).unwrap();
        assert_eq!(p5.available_capacity, Some(200)); // limited: 200 - 0 committed

        let p6 = results.iter().find(|(a, _)| *a == 6).map(|(_, i)| i).unwrap();
        assert!(p6.available_capacity.is_none()); // unlimited

        // Pagination: offset=1 → exactly one result remains.
        let paged = StorageProvider::query_providers_with_capacity(150, 1, 10);
        assert_eq!(paged.len(), 1);
    });
}

#[test]
fn query_agreement_info_returns_replica_role() {
    // Covers the Replica arm of query_agreement_info (lib.rs 3392-3401).
    use storage_primitives::agreement_term::ReplicaTerms;
    new_test_ext().execute_with(|| {
        register_provider(2, 200);
        let bucket_id = setup_agreement(2, 1, 50, 200);

        register_provider_with_settings(
            3,
            200,
            ProviderSettings {
                accepting_primary: false,
                replica_sync_price: Some(5),
                ..Default::default()
            },
        );
        setup_replica_agreement(
            3,
            1,
            bucket_id,
            50,
            200,
            ReplicaTerms { sync_balance: 100, min_sync_interval: 10, sync_price: 5 },
        );

        let response = StorageProvider::query_agreement_info(bucket_id, &3).unwrap();
        if let ProviderRole::Replica { sync_balance, sync_price, min_sync_interval, last_sync } =
            response.role
        {
            assert_eq!(sync_balance, 100u128);
            assert_eq!(sync_price, 5u128);
            assert_eq!(min_sync_interval, 10u32);
            assert!(last_sync.is_none());
        } else {
            panic!("expected Replica role");
        }
    });
}

#[test]
fn query_bucket_agreements_includes_replica() {
    // Covers query_bucket_agreements (lib.rs 3410-3442) including the Replica
    // arm of the role match inside that function.
    use storage_primitives::agreement_term::ReplicaTerms;
    new_test_ext().execute_with(|| {
        register_provider(2, 200);
        let bucket_id = setup_agreement(2, 1, 50, 200);

        register_provider_with_settings(
            3,
            200,
            ProviderSettings {
                accepting_primary: false,
                replica_sync_price: Some(5),
                ..Default::default()
            },
        );
        setup_replica_agreement(
            3,
            1,
            bucket_id,
            50,
            200,
            ReplicaTerms { sync_balance: 100, min_sync_interval: 10, sync_price: 5 },
        );

        let agreements = StorageProvider::query_bucket_agreements(bucket_id);
        assert_eq!(agreements.len(), 2);

        let has_primary = agreements.iter().any(|a| matches!(a.role, ProviderRole::Primary));
        let has_replica =
            agreements.iter().any(|a| matches!(a.role, ProviderRole::Replica { .. }));
        assert!(has_primary);
        assert!(has_replica);
    });
}

#[test]
fn query_bucket_ids_paginates() {
    // Covers query_bucket_ids (lib.rs 3445-3450).
    new_test_ext().execute_with(|| {
        register_provider(2, 200);
        setup_agreement(2, 1, 50, 200); // bucket 0
        setup_agreement(2, 1, 50, 200); // bucket 1

        let all = StorageProvider::query_bucket_ids(0, 10);
        assert_eq!(all.len(), 2);

        let paged = StorageProvider::query_bucket_ids(1, 1);
        assert_eq!(paged.len(), 1);
    });
}

#[test]
fn query_provider_agreements_includes_replica() {
    // Covers query_provider_agreements (lib.rs 3452-3486) including the Replica
    // arm of the role match.
    use storage_primitives::agreement_term::ReplicaTerms;
    new_test_ext().execute_with(|| {
        register_provider(2, 200);
        let bucket_id = setup_agreement(2, 1, 50, 200);

        register_provider_with_settings(
            3,
            200,
            ProviderSettings {
                accepting_primary: false,
                replica_sync_price: Some(5),
                ..Default::default()
            },
        );
        setup_replica_agreement(
            3,
            1,
            bucket_id,
            50,
            200,
            ReplicaTerms { sync_balance: 100, min_sync_interval: 10, sync_price: 5 },
        );

        // Provider 2: one Primary agreement.
        let p2 = StorageProvider::query_provider_agreements(&2);
        assert_eq!(p2.len(), 1);
        assert!(matches!(p2[0].role, ProviderRole::Primary));

        // Provider 3: one Replica agreement.
        let p3 = StorageProvider::query_provider_agreements(&3);
        assert_eq!(p3.len(), 1);
        assert!(matches!(p3[0].role, ProviderRole::Replica { .. }));

        // Non-existent provider returns empty list.
        assert!(StorageProvider::query_provider_agreements(&99).is_empty());
    });
}

#[test]
fn query_challenges_at_returns_data() {
    new_test_ext().execute_with(|| {
        frame_system::Pallet::<Test>::set_block_number(1);
        register_provider(2, 200);
        let bucket_id = setup_agreement(2, 1, 50, 200);

        // Insert snapshot
        Buckets::<Test>::mutate(bucket_id, |maybe_bucket| {
            if let Some(bucket) = maybe_bucket {
                bucket.snapshot = Some(BucketSnapshot {
                    mmr_root: H256::repeat_byte(0xAB),
                    start_seq: 0,
                    leaf_count: 10,
                    checkpoint_block: 1,
                    primary_signers: vec![0x01],
                });
            }
        });

        assert_ok!(StorageProvider::challenge_checkpoint(
            RuntimeOrigin::signed(3),
            bucket_id,
            2,
            0,
            0,
        ));

        let challenges = StorageProvider::query_challenges_at(101);
        assert_eq!(challenges.len(), 1);
        assert_eq!(challenges[0].bucket_id, bucket_id);
        assert_eq!(challenges[0].provider, 2u64.encode());
        assert_eq!(challenges[0].challenger, 3u64.encode());
        assert_eq!(challenges[0].deadline, 101);
        assert_eq!(challenges[0].deposit, 100);
    });
}
