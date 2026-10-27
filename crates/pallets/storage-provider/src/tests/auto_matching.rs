// SPDX-License-Identifier: Apache-2.0

//! Provider matching moved off-chain: clients query
//! `query_find_matching_providers` (runtime API), pick a provider, obtain
//! signed terms, and redeem them via `create_bucket_with_primary`.

use super::*;
use crate::runtime_api::{PartialMatchReason, StorageRequirements};
use codec::Encode;

fn requirements(
    bytes_needed: u64,
    min_duration: u32,
    max_price_per_byte: u128,
) -> StorageRequirements {
    StorageRequirements {
        bytes_needed,
        min_duration,
        max_price_per_byte,
        primary_only: true,
    }
}

fn register_provider_for_matching(who: u64, price_per_byte: u64, max_capacity: u64) {
    register_provider_with_settings(
        who,
        200,
        ProviderSettings {
            min_duration: 10u64,
            max_duration: 1000u64,
            price_per_byte,
            accepting_primary: true,
            replica_sync_price: None,
            accepting_extensions: true,
            max_capacity,
            min_bytes: 0,
        },
    );
}

#[test]
fn find_matching_providers_returns_perfect_match() {
    new_test_ext().execute_with(|| {
        register_provider_for_matching(2, 0, 200);

        let matches =
            StorageProvider::query_find_matching_providers(requirements(100, 100, 10), 10);

        assert_eq!(matches.len(), 1);
        assert_eq!(matches[0].account, 2u64.encode());
        assert_eq!(matches[0].match_score, 100);
        assert!(matches[0].partial_reason.is_none());
        assert_eq!(matches[0].available_capacity, Some(200));
    });
}

#[test]
fn find_matching_providers_returns_empty_without_providers() {
    new_test_ext().execute_with(|| {
        let matches =
            StorageProvider::query_find_matching_providers(requirements(100, 100, 10), 10);
        assert!(matches.is_empty());
    });
}

#[test]
fn find_matching_providers_flags_not_accepting() {
    new_test_ext().execute_with(|| {
        register_provider_with_settings(
            2,
            200,
            ProviderSettings {
                min_duration: 10u64,
                max_duration: 1000u64,
                price_per_byte: 1u64,
                accepting_primary: false, // Not accepting
                replica_sync_price: None,
                accepting_extensions: true,
                max_capacity: 200,
                min_bytes: 0,
            },
        );

        let matches =
            StorageProvider::query_find_matching_providers(requirements(100, 100, 10), 10);

        assert_eq!(matches.len(), 1);
        assert_eq!(matches[0].match_score, 0);
        assert_eq!(
            matches[0].partial_reason,
            Some(PartialMatchReason::NotAccepting)
        );
    });
}

#[test]
fn find_matching_providers_flags_price_too_high() {
    new_test_ext().execute_with(|| {
        // Provider charges 100, requirement caps at 10.
        register_provider_for_matching(2, 100, 200);

        let matches =
            StorageProvider::query_find_matching_providers(requirements(100, 100, 10), 10);

        assert_eq!(matches.len(), 1);
        assert_eq!(matches[0].match_score, 70);
        assert_eq!(
            matches[0].partial_reason,
            Some(PartialMatchReason::PriceTooHigh)
        );
    });
}

#[test]
fn find_matching_providers_flags_insufficient_capacity() {
    new_test_ext().execute_with(|| {
        // Provider only has 50 bytes capacity, requirement needs 100.
        register_provider_for_matching(2, 1, 50);

        let matches =
            StorageProvider::query_find_matching_providers(requirements(100, 100, 10), 10);

        assert_eq!(matches.len(), 1);
        assert_eq!(matches[0].match_score, 50);
        assert_eq!(
            matches[0].partial_reason,
            Some(PartialMatchReason::InsufficientCapacity)
        );
    });
}

#[test]
fn find_matching_providers_flags_duration_mismatch() {
    new_test_ext().execute_with(|| {
        register_provider_with_settings(
            2,
            200,
            ProviderSettings {
                min_duration: 500u64, // Minimum 500 blocks
                max_duration: 1000u64,
                price_per_byte: 1u64,
                accepting_primary: true,
                replica_sync_price: None,
                accepting_extensions: true,
                max_capacity: 200,
                min_bytes: 0,
            },
        );

        // Requirement asks for 100 blocks, below provider's min of 500.
        let matches =
            StorageProvider::query_find_matching_providers(requirements(100, 100, 10), 10);

        assert_eq!(matches.len(), 1);
        assert_eq!(matches[0].match_score, 80);
        assert_eq!(
            matches[0].partial_reason,
            Some(PartialMatchReason::DurationMismatch)
        );
    });
}

#[test]
fn find_matching_providers_ranks_cheapest_first() {
    new_test_ext().execute_with(|| {
        // Provider 2: expensive (price = 5) - but still affordable
        register_provider_for_matching(2, 5, 200);
        // Provider 3: cheap (price = 0)
        register_provider_for_matching(3, 0, 200);

        let matches = StorageProvider::query_find_matching_providers(requirements(10, 10, 10), 10);

        // Both are perfect matches; ties are broken by ascending price.
        assert_eq!(matches.len(), 2);
        assert_eq!(matches[0].account, 3u64.encode());
        assert_eq!(matches[0].match_score, 100);
        assert_eq!(matches[1].account, 2u64.encode());
        assert_eq!(matches[1].match_score, 100);
    });
}

#[test]
fn matched_provider_redeems_signed_terms() {
    new_test_ext().execute_with(|| {
        register_provider_for_matching(2, 0, 200);

        // Client-side flow: query for a match, then redeem the provider's
        // signed quote, which creates the bucket + primary agreement.
        let matches =
            StorageProvider::query_find_matching_providers(requirements(100, 100, 10), 10);
        assert_eq!(matches[0].account, 2u64.encode());

        let bucket_id = setup_agreement(2, 1, 100, 100);

        // Verify bucket was created
        let bucket = Buckets::<Test>::get(bucket_id).unwrap();
        assert_eq!(bucket.min_providers, 1);
        assert_eq!(bucket.primary_providers.to_vec(), vec![2]);

        // Verify agreement was created
        let agreement = StorageAgreements::<Test>::get(bucket_id, 2).unwrap();
        assert_eq!(agreement.max_bytes, 100);
        assert_eq!(agreement.owner, 1);

        // Verify provider's committed_bytes was updated
        let provider = Providers::<Test>::get(2).unwrap();
        assert_eq!(provider.committed_bytes, 100);
    });
}

fn register_provider_with_min_bytes(who: u64, min_bytes: u64) {
    register_provider_with_settings(
        who,
        200,
        ProviderSettings {
            accepting_primary: true,
            max_capacity: 200,
            min_bytes,
            ..Default::default()
        },
    );
}

#[test]
fn find_matching_providers_penalises_requests_below_min_bytes() {
    new_test_ext().execute_with(|| {
        register_provider_with_min_bytes(2, 100);

        let below = StorageProvider::query_find_matching_providers(requirements(99, 100, 10), 10);
        assert_eq!(below[0].match_score, 50);
        assert_eq!(
            below[0].partial_reason,
            Some(PartialMatchReason::BelowMinBytes)
        );
        assert_eq!(below[0].info.min_bytes, 100);

        // Exactly the minimum matches fully.
        let at = StorageProvider::query_find_matching_providers(requirements(100, 100, 10), 10);
        assert_eq!(at[0].match_score, 100);
        assert!(at[0].partial_reason.is_none());
    });
}

#[test]
fn providers_with_capacity_excludes_providers_whose_min_bytes_is_above_request() {
    new_test_ext().execute_with(|| {
        register_provider_with_min_bytes(2, 100);
        register_provider_with_min_bytes(3, 0);

        let accounts = |bytes_needed| -> Vec<u64> {
            StorageProvider::query_providers_with_capacity(bytes_needed, 0, 10)
                .into_iter()
                .map(|(account, _)| account)
                .collect()
        };

        assert_eq!(accounts(99), vec![3]);

        let mut both = accounts(100);
        both.sort();
        assert_eq!(both, vec![2, 3]);

        let info = StorageProvider::query_providers_with_capacity(100, 0, 10)
            .into_iter()
            .find(|(account, _)| *account == 2)
            .unwrap()
            .1;
        assert_eq!(info.min_bytes, 100);
    });
}
