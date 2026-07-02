use crate::*;
use alloc::vec::Vec;
use frame_support::pallet_prelude::*;
use sp_runtime::traits::{CheckedMul, SaturatedConversion};

impl<T: Config> Pallet<T> {
    /// Find providers matching the given storage requirements.
    pub fn query_find_matching_providers(
        requirements: crate::runtime_api::StorageRequirements,
        limit: u32,
    ) -> Vec<crate::runtime_api::MatchedProvider> {
        use crate::runtime_api::{MatchedProvider, PartialMatchReason};

        let mut results: Vec<MatchedProvider> = Vec::new();

        for (account, info) in Providers::<T>::iter() {
            // Skip providers that have announced deregistration — they are
            // winding down and must not be offered for new agreements.
            if info.deregister_at.is_some() {
                continue;
            }

            let max_capacity = info.settings.max_capacity;
            let available = if max_capacity > 0 {
                max_capacity.saturating_sub(info.committed_bytes)
            } else {
                u64::MAX // Unlimited
            };

            let price: u128 = info.settings.price_per_byte.saturated_into();
            let min_dur: u32 = info.settings.min_duration.saturated_into();
            let max_dur: u32 = info.settings.max_duration.saturated_into();

            // Determine match score and partial reason
            let mut score: u8 = 100;
            let mut partial_reason: Option<PartialMatchReason> = None;

            // Check accepting status
            // Primary required: must accept primary
            // Replica acceptable: must accept primary OR have replica sync price
            let not_accepting = if requirements.primary_only {
                !info.settings.accepting_primary
            } else {
                !info.settings.accepting_primary && info.settings.replica_sync_price.is_none()
            };
            if not_accepting {
                score = 0;
                partial_reason = Some(PartialMatchReason::NotAccepting);
            }

            // Check capacity
            if score > 0 && available < requirements.bytes_needed {
                score = score.saturating_sub(50);
                if partial_reason.is_none() {
                    partial_reason = Some(PartialMatchReason::InsufficientCapacity);
                }
            }

            // Check price
            if score > 0 && price > requirements.max_price_per_byte {
                score = score.saturating_sub(30);
                if partial_reason.is_none() {
                    partial_reason = Some(PartialMatchReason::PriceTooHigh);
                }
            }

            // Check duration
            if score > 0
                && (requirements.min_duration < min_dur || requirements.min_duration > max_dur)
            {
                score = score.saturating_sub(20);
                if partial_reason.is_none() {
                    partial_reason = Some(PartialMatchReason::DurationMismatch);
                }
            }

            // Build the available_capacity field
            let available_capacity = if max_capacity > 0 {
                Some(available)
            } else {
                None
            };

            let provider_response = crate::runtime_api::ProviderInfoResponse {
                multiaddr: info.multiaddr.to_vec(),
                public_key: info.public_key.to_vec(),
                stake: info.stake.saturated_into::<u128>(),
                committed_bytes: info.committed_bytes,
                min_duration: min_dur,
                max_duration: max_dur,
                price_per_byte: price,
                accepting_primary: info.settings.accepting_primary,
                replica_sync_price: info
                    .settings
                    .replica_sync_price
                    .map(|p| p.saturated_into::<u128>()),
                accepting_extensions: info.settings.accepting_extensions,
                registered_at: info.stats.registered_at.saturated_into::<u32>(),
                agreements_total: info.stats.agreements_total,
                agreements_extended: info.stats.agreements_extended,
                agreements_not_extended: info.stats.agreements_not_extended,
                agreements_burned: info.stats.agreements_burned,
                challenges_received: info.stats.challenges_received,
                challenges_failed: info.stats.challenges_failed,
                max_capacity,
                available_capacity,
            };

            results.push(MatchedProvider {
                account: account.encode(),
                info: provider_response,
                match_score: score,
                available_capacity,
                partial_reason,
            });
        }

        // Sort by score descending, then by price ascending for ties
        results.sort_by(|a, b| {
            b.match_score
                .cmp(&a.match_score)
                .then(a.info.price_per_byte.cmp(&b.info.price_per_byte))
        });

        results.truncate(limit as usize);
        results
    }

    /// Get providers with sufficient capacity for the given bytes (paginated).
    pub fn query_providers_with_capacity(
        bytes_needed: u64,
        offset: u32,
        limit: u32,
    ) -> Vec<(T::AccountId, crate::runtime_api::ProviderInfoResponse)> {
        Providers::<T>::iter()
            .filter(|(_, info)| {
                // Check accepting status
                if !info.settings.accepting_primary && info.settings.replica_sync_price.is_none() {
                    return false;
                }

                // Check capacity
                let max_capacity = info.settings.max_capacity;
                if max_capacity > 0 {
                    let available = max_capacity.saturating_sub(info.committed_bytes);
                    if available < bytes_needed {
                        return false;
                    }
                }

                // Check stake (can they back the additional bytes?)
                let new_committed = info.committed_bytes.saturating_add(bytes_needed);
                let bytes_as_balance: BalanceOf<T> = new_committed.saturated_into();
                if let Some(required_stake) =
                    T::MinStakePerByte::get().checked_mul(&bytes_as_balance)
                {
                    return info.stake >= required_stake;
                }
                false
            })
            .skip(offset as usize)
            .take(limit as usize)
            .map(|(account, info)| {
                let max_capacity = info.settings.max_capacity;
                let available_capacity = if max_capacity > 0 {
                    Some(max_capacity.saturating_sub(info.committed_bytes))
                } else {
                    None
                };

                (
                    account,
                    crate::runtime_api::ProviderInfoResponse {
                        multiaddr: info.multiaddr.to_vec(),
                        public_key: info.public_key.to_vec(),
                        stake: info.stake.saturated_into::<u128>(),
                        committed_bytes: info.committed_bytes,
                        min_duration: info.settings.min_duration.saturated_into::<u32>(),
                        max_duration: info.settings.max_duration.saturated_into::<u32>(),
                        price_per_byte: info.settings.price_per_byte.saturated_into::<u128>(),
                        accepting_primary: info.settings.accepting_primary,
                        replica_sync_price: info
                            .settings
                            .replica_sync_price
                            .map(|p| p.saturated_into::<u128>()),
                        accepting_extensions: info.settings.accepting_extensions,
                        registered_at: info.stats.registered_at.saturated_into::<u32>(),
                        agreements_total: info.stats.agreements_total,
                        agreements_extended: info.stats.agreements_extended,
                        agreements_not_extended: info.stats.agreements_not_extended,
                        agreements_burned: info.stats.agreements_burned,
                        challenges_received: info.stats.challenges_received,
                        challenges_failed: info.stats.challenges_failed,
                        max_capacity,
                        available_capacity,
                    },
                )
            })
            .collect()
    }
}
