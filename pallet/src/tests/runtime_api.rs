// SPDX-License-Identifier: Apache-2.0

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
                    commitment_nonce: 0,
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
fn query_bucket_agreements_returns_data() {
    new_test_ext().execute_with(|| {
        register_provider(2, 200);
        register_provider(4, 200);
        let bucket_id = setup_agreement(2, 1, 50, 200);
        add_primary_to_bucket(4, 1, bucket_id, 30);

        let agreements = StorageProvider::query_bucket_agreements(bucket_id);
        assert_eq!(agreements.len(), 2);
        assert!(agreements.iter().all(|a| a.bucket_id == bucket_id));
        assert!(agreements.iter().any(|a| a.provider == 2u64.encode()));
        assert!(agreements.iter().any(|a| a.provider == 4u64.encode()));
    });
}

#[test]
fn query_bucket_agreements_empty_for_unknown() {
    new_test_ext().execute_with(|| {
        let agreements = StorageProvider::query_bucket_agreements(999);
        assert!(agreements.is_empty());
    });
}

#[test]
fn query_provider_agreements_returns_data() {
    new_test_ext().execute_with(|| {
        register_provider(2, 200);
        let bucket_a = setup_agreement(2, 1, 50, 200);
        let bucket_b = setup_agreement(2, 3, 50, 200);

        let agreements = StorageProvider::query_provider_agreements(&2);
        assert_eq!(agreements.len(), 2);
        assert!(agreements.iter().all(|a| a.provider == 2u64.encode()));
        assert!(agreements.iter().any(|a| a.bucket_id == bucket_a));
        assert!(agreements.iter().any(|a| a.bucket_id == bucket_b));
    });
}

#[test]
fn query_provider_agreements_empty_for_unknown() {
    new_test_ext().execute_with(|| {
        register_provider(2, 200);
        setup_agreement(2, 1, 50, 200);

        let agreements = StorageProvider::query_provider_agreements(&99);
        assert!(agreements.is_empty());
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

#[test]
fn query_challenges_at_returns_data() {
    new_test_ext().execute_with(|| {
        frame_system::Pallet::<Test>::set_block_number(1);
        register_provider(2, 200);
        register_provider(4, 200);
        let bucket_id = setup_agreement(2, 1, 50, 200);
        add_primary_to_bucket(4, 1, bucket_id, 50);

        // Insert snapshot signed by both primaries (bits 0 and 1).
        Buckets::<Test>::mutate(bucket_id, |maybe_bucket| {
            if let Some(bucket) = maybe_bucket {
                bucket.snapshot = Some(BucketSnapshot {
                    mmr_root: H256::repeat_byte(0xAB),
                    start_seq: 0,
                    leaf_count: 10,
                    checkpoint_block: 1,
                    primary_signers: vec![0x03],
                    commitment_nonce: 0,
                });
            }
        });

        // Two challenges at the same deadline: index 0 -> provider 2,
        // index 1 -> provider 4.
        assert_ok!(StorageProvider::challenge_checkpoint(
            RuntimeOrigin::signed(3),
            bucket_id,
            2,
            0,
            0,
        ));
        assert_ok!(StorageProvider::challenge_checkpoint(
            RuntimeOrigin::signed(5),
            bucket_id,
            4,
            0,
            0,
        ));

        // `iter_prefix` order is hash-dependent, so look entries up by their
        // stable `index` rather than position.
        let challenges = StorageProvider::query_challenges_at(101);
        assert_eq!(challenges.len(), 2);
        let find = |idx: u16| {
            challenges
                .iter()
                .find(|c| c.index == idx)
                .unwrap_or_else(|| panic!("challenge index {idx} present"))
        };

        let c0 = find(0);
        assert_eq!(c0.bucket_id, bucket_id);
        assert_eq!(c0.provider, 2u64.encode());
        assert_eq!(c0.challenger, 3u64.encode());
        assert_eq!(c0.deadline, 101);
        assert_eq!(c0.deposit, 100);

        let c1 = find(1);
        assert_eq!(c1.provider, 4u64.encode());
        assert_eq!(c1.challenger, 5u64.encode());
        assert_eq!(c1.deadline, 101);

        // Supersede the challenged root, then resolve sibling index 0.
        Buckets::<Test>::mutate(bucket_id, |maybe_bucket| {
            let bucket = maybe_bucket.as_mut().unwrap();
            bucket.snapshot = Some(BucketSnapshot {
                mmr_root: H256::repeat_byte(0xCD),
                start_seq: 0,
                leaf_count: 10,
                checkpoint_block: 1,
                primary_signers: vec![0x03],
                commitment_nonce: 1,
            });
        });
        assert_ok!(StorageProvider::respond_to_challenge(
            RuntimeOrigin::signed(2),
            storage_primitives::ChallengeId {
                deadline: 101,
                index: 0,
            },
            crate::ChallengeResponse::Superseded,
        ));

        // After removing sibling index 0, the survivor is still reported at its
        // original index 1.
        let remaining = StorageProvider::query_challenges_at(101);
        assert_eq!(remaining.len(), 1);
        assert_eq!(remaining[0].index, 1);
        assert_eq!(remaining[0].provider, 4u64.encode());
    });
}

/// Helper: set up a bucket with two primaries (2 and 4), a snapshot signed by
/// both, and one open challenge per primary (challenger 3 -> provider 2,
/// challenger 5 -> provider 4). Returns the bucket_id.
fn setup_two_challenges() -> u64 {
    frame_system::Pallet::<Test>::set_block_number(1);
    register_provider(2, 200);
    register_provider(4, 200);
    let bucket_id = setup_agreement(2, 1, 50, 200);
    add_primary_to_bucket(4, 1, bucket_id, 50);

    Buckets::<Test>::mutate(bucket_id, |maybe_bucket| {
        if let Some(bucket) = maybe_bucket {
            bucket.snapshot = Some(BucketSnapshot {
                mmr_root: H256::repeat_byte(0xAB),
                start_seq: 0,
                leaf_count: 10,
                checkpoint_block: 1,
                primary_signers: vec![0x03],
                commitment_nonce: 0,
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
    assert_ok!(StorageProvider::challenge_checkpoint(
        RuntimeOrigin::signed(5),
        bucket_id,
        4,
        0,
        0,
    ));

    bucket_id
}

#[test]
fn query_bucket_challenges_empty_for_unknown() {
    new_test_ext().execute_with(|| {
        setup_two_challenges();

        let challenges = StorageProvider::query_bucket_challenges(999);
        assert!(challenges.is_empty());
    });
}

#[test]
fn query_provider_challenges_empty_for_unknown() {
    new_test_ext().execute_with(|| {
        setup_two_challenges();

        let challenges = StorageProvider::query_provider_challenges(&99);
        assert!(challenges.is_empty());
    });
}

#[test]
fn query_challenger_challenges_empty_for_unknown() {
    new_test_ext().execute_with(|| {
        setup_two_challenges();

        let challenges = StorageProvider::query_challenger_challenges(&99);
        assert!(challenges.is_empty());
    });
}

#[test]
fn agreement_response_includes_bucket_id() {
    new_test_ext().execute_with(|| {
        register_provider(2, 200);
        let bucket_id = setup_agreement(2, 1, 50, 200);

        let response = StorageProvider::query_agreement_info(bucket_id, &2).unwrap();
        assert_eq!(response.bucket_id, bucket_id);

        let bucket_agreements = StorageProvider::query_bucket_agreements(bucket_id);
        assert_eq!(bucket_agreements.len(), 1);
        assert_eq!(bucket_agreements[0].bucket_id, bucket_id);

        let provider_agreements = StorageProvider::query_provider_agreements(&2);
        assert_eq!(provider_agreements.len(), 1);
        assert_eq!(provider_agreements[0].bucket_id, bucket_id);
    });
}

#[test]
fn challenge_response_includes_index() {
    new_test_ext().execute_with(|| {
        frame_system::Pallet::<Test>::set_block_number(1);
        register_provider(2, 200);
        register_provider(3, 200);
        let bucket_id_a = setup_agreement(2, 1, 50, 200);
        let bucket_id_b = setup_agreement(3, 1, 50, 200);

        let snapshot = BucketSnapshot {
            mmr_root: H256::repeat_byte(0xAB),
            start_seq: 0,
            leaf_count: 10,
            checkpoint_block: 1,
            primary_signers: vec![0x01],
            commitment_nonce: 0,
        };
        Buckets::<Test>::mutate(bucket_id_a, |b| {
            if let Some(b) = b {
                b.snapshot = Some(snapshot.clone());
            }
        });
        Buckets::<Test>::mutate(bucket_id_b, |b| {
            if let Some(b) = b {
                b.snapshot = Some(snapshot);
            }
        });

        // Two challenges at the same deadline block
        assert_ok!(StorageProvider::challenge_checkpoint(
            RuntimeOrigin::signed(4),
            bucket_id_a,
            2,
            0,
            0,
        ));
        assert_ok!(StorageProvider::challenge_checkpoint(
            RuntimeOrigin::signed(4),
            bucket_id_b,
            3,
            0,
            0,
        ));

        let challenges = StorageProvider::query_challenges_at(101);
        assert_eq!(challenges.len(), 2);
        assert_eq!(challenges[0].index, 0);
        assert_eq!(challenges[1].index, 1);
    });
}

#[test]
fn query_bucket_challenges_returns_data() {
    new_test_ext().execute_with(|| {
        frame_system::Pallet::<Test>::set_block_number(1);
        register_provider(2, 200);
        register_provider(3, 200);
        let bucket_id_a = setup_agreement(2, 1, 50, 200);
        let bucket_id_b = setup_agreement(3, 1, 50, 200);

        let snapshot = BucketSnapshot {
            mmr_root: H256::repeat_byte(0xAB),
            start_seq: 0,
            leaf_count: 10,
            checkpoint_block: 1,
            primary_signers: vec![0x01],
            commitment_nonce: 0,
        };
        Buckets::<Test>::mutate(bucket_id_a, |b| {
            if let Some(b) = b {
                b.snapshot = Some(snapshot.clone());
            }
        });
        Buckets::<Test>::mutate(bucket_id_b, |b| {
            if let Some(b) = b {
                b.snapshot = Some(snapshot);
            }
        });

        assert_ok!(StorageProvider::challenge_checkpoint(
            RuntimeOrigin::signed(4),
            bucket_id_a,
            2,
            0,
            0,
        ));
        assert_ok!(StorageProvider::challenge_checkpoint(
            RuntimeOrigin::signed(4),
            bucket_id_b,
            3,
            0,
            0,
        ));

        let results = StorageProvider::query_bucket_challenges(bucket_id_a);
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].bucket_id, bucket_id_a);
        assert_eq!(results[0].deadline, 101);
        assert_eq!(results[0].index, 0);

        let results = StorageProvider::query_bucket_challenges(bucket_id_b);
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].bucket_id, bucket_id_b);

        let empty = StorageProvider::query_bucket_challenges(999);
        assert!(empty.is_empty());
    });
}

#[test]
fn query_provider_challenges_returns_data() {
    new_test_ext().execute_with(|| {
        frame_system::Pallet::<Test>::set_block_number(1);
        register_provider(2, 200);
        register_provider(3, 200);
        let bucket_id_a = setup_agreement(2, 1, 50, 200);
        let bucket_id_b = setup_agreement(3, 1, 50, 200);

        let snapshot = BucketSnapshot {
            mmr_root: H256::repeat_byte(0xAB),
            start_seq: 0,
            leaf_count: 10,
            checkpoint_block: 1,
            primary_signers: vec![0x01],
            commitment_nonce: 0,
        };
        Buckets::<Test>::mutate(bucket_id_a, |b| {
            if let Some(b) = b {
                b.snapshot = Some(snapshot.clone());
            }
        });
        Buckets::<Test>::mutate(bucket_id_b, |b| {
            if let Some(b) = b {
                b.snapshot = Some(snapshot);
            }
        });

        assert_ok!(StorageProvider::challenge_checkpoint(
            RuntimeOrigin::signed(4),
            bucket_id_a,
            2,
            0,
            0,
        ));
        assert_ok!(StorageProvider::challenge_checkpoint(
            RuntimeOrigin::signed(4),
            bucket_id_b,
            3,
            0,
            0,
        ));

        let results = StorageProvider::query_provider_challenges(&2);
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].bucket_id, bucket_id_a);
        assert_eq!(results[0].provider, 2u64.encode());
        assert_eq!(results[0].index, 0);

        let results = StorageProvider::query_provider_challenges(&3);
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].bucket_id, bucket_id_b);

        let empty = StorageProvider::query_provider_challenges(&99);
        assert!(empty.is_empty());
    });
}

#[test]
fn query_challenger_challenges_returns_data() {
    new_test_ext().execute_with(|| {
        frame_system::Pallet::<Test>::set_block_number(1);
        register_provider(2, 200);
        register_provider(3, 200);
        let bucket_id_a = setup_agreement(2, 1, 50, 200);
        let bucket_id_b = setup_agreement(3, 1, 50, 200);

        let snapshot = BucketSnapshot {
            mmr_root: H256::repeat_byte(0xAB),
            start_seq: 0,
            leaf_count: 10,
            checkpoint_block: 1,
            primary_signers: vec![0x01],
            commitment_nonce: 0,
        };
        Buckets::<Test>::mutate(bucket_id_a, |b| {
            if let Some(b) = b {
                b.snapshot = Some(snapshot.clone());
            }
        });
        Buckets::<Test>::mutate(bucket_id_b, |b| {
            if let Some(b) = b {
                b.snapshot = Some(snapshot);
            }
        });

        // challenger 4 challenges provider 2; challenger 5 challenges provider 3
        assert_ok!(StorageProvider::challenge_checkpoint(
            RuntimeOrigin::signed(4),
            bucket_id_a,
            2,
            0,
            0,
        ));
        assert_ok!(StorageProvider::challenge_checkpoint(
            RuntimeOrigin::signed(5),
            bucket_id_b,
            3,
            0,
            0,
        ));

        let results = StorageProvider::query_challenger_challenges(&4);
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].bucket_id, bucket_id_a);
        assert_eq!(results[0].challenger, 4u64.encode());
        assert_eq!(results[0].index, 0);

        let results = StorageProvider::query_challenger_challenges(&5);
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].bucket_id, bucket_id_b);
        assert_eq!(results[0].challenger, 5u64.encode());

        let empty = StorageProvider::query_challenger_challenges(&99);
        assert!(empty.is_empty());
    });
}
