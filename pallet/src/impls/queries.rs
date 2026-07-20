// SPDX-License-Identifier: Apache-2.0

use crate::*;
use alloc::vec::Vec;
use frame_support::pallet_prelude::*;
use sp_runtime::traits::{CheckedMul, SaturatedConversion};
use storage_primitives::{BucketId, BucketSnapshot, ProviderRole, ReplicaSyncRecord};

fn challenge_to_response<T: Config>(
    deadline: BlockNumberFor<T>,
    index: u16,
    c: Challenge<T>,
) -> crate::runtime_api::ChallengeResponse {
    crate::runtime_api::ChallengeResponse {
        bucket_id: c.bucket_id,
        provider: c.provider.encode(),
        challenger: c.challenger.encode(),
        mmr_root: c.mmr_root,
        start_seq: c.start_seq,
        leaf_index: c.target.leaf_index,
        chunk_index: c.target.chunk_index,
        deadline: deadline.saturated_into(),
        index,
        deposit: c.deposit.saturated_into::<u128>(),
    }
}

fn agreement_to_response<T: Config>(
    bucket_id: BucketId,
    provider: &T::AccountId,
    agreement: StorageAgreement<T>,
) -> crate::runtime_api::AgreementResponse {
    crate::runtime_api::AgreementResponse {
        bucket_id,
        owner: agreement.owner.encode(),
        provider: provider.encode(),
        max_bytes: agreement.max_bytes,
        payment_locked: agreement.payment_locked.saturated_into::<u128>(),
        price_per_byte: agreement.price_per_byte.saturated_into::<u128>(),
        expires_at: agreement.expires_at.saturated_into::<u32>(),
        extensions_blocked: agreement.extensions_blocked,
        role: match agreement.role {
            ProviderRole::Primary => ProviderRole::Primary,
            ProviderRole::Replica {
                sync_balance,
                sync_price,
                min_sync_interval,
                last_sync,
            } => ProviderRole::Replica {
                sync_balance: sync_balance.saturated_into::<u128>(),
                sync_price: sync_price.saturated_into::<u128>(),
                min_sync_interval: min_sync_interval.saturated_into::<u32>(),
                last_sync: last_sync.map(|r| ReplicaSyncRecord {
                    root: r.root,
                    range: r.range,
                    block: r.block.saturated_into::<u32>(),
                }),
            },
        },
        started_at: agreement.started_at.saturated_into::<u32>(),
    }
}

impl<T: Config> Pallet<T> {
    /// The anchor block: the clock every pallet duration (timeouts, expiries,
    /// `valid_until`, nonce age) is measured against. Whether that is a relay
    /// chain block, parachain block, or something else is a
    /// [`Config::BlockNumberProvider`] detail — callers (including off-chain
    /// consumers via the `current_anchor_block` runtime API) need not care.
    /// The relay chain in production, `System` in tests.
    ///
    /// During `on_initialize` this returns the *previous* block's anchor value
    /// (the validation-data inherent has not run yet); everywhere else it is
    /// the current block's.
    pub fn current_anchor_block() -> BlockNumberFor<T> {
        <T::BlockNumberProvider as sp_runtime::traits::BlockNumberProvider>::current_block_number()
    }

    /// Milliseconds per anchor block; pairs with
    /// [`Self::current_anchor_block`] so off-chain consumers can humanize
    /// anchor-denominated durations.
    pub fn anchor_block_time_millis() -> u64 {
        T::AnchorBlockTimeMillis::get()
    }

    /// Query provider information.
    pub fn query_provider_info(
        provider: &T::AccountId,
    ) -> Option<crate::runtime_api::ProviderInfoResponse> {
        Providers::<T>::get(provider).map(|info| {
            let max_capacity = info.settings.max_capacity;
            let available_capacity = if max_capacity > 0 {
                Some(max_capacity.saturating_sub(info.committed_bytes))
            } else {
                None // Unlimited
            };

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
            }
        })
    }

    /// Query all providers (paginated).
    pub fn query_providers(
        offset: u32,
        limit: u32,
    ) -> Vec<(T::AccountId, crate::runtime_api::ProviderInfoResponse)> {
        Providers::<T>::iter()
            .skip(offset as usize)
            .take(limit as usize)
            .map(|(account, info)| {
                let max_capacity = info.settings.max_capacity;
                let available_capacity = if max_capacity > 0 {
                    Some(max_capacity.saturating_sub(info.committed_bytes))
                } else {
                    None // Unlimited
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

    /// Query bucket information.
    pub fn query_bucket_info(bucket_id: BucketId) -> Option<crate::runtime_api::BucketResponse> {
        Buckets::<T>::get(bucket_id).map(|bucket| crate::runtime_api::BucketResponse {
            bucket_id,
            members: bucket
                .members
                .iter()
                .map(|m| crate::runtime_api::BucketMemberResponse {
                    account: m.account.encode(),
                    role: m.role,
                })
                .collect(),
            frozen_start_seq: bucket.frozen_start_seq,
            min_providers: bucket.min_providers,
            primary_providers: bucket
                .primary_providers
                .iter()
                .map(|p| p.encode())
                .collect(),
            snapshot: bucket.snapshot.map(|s| BucketSnapshot {
                commitment: s.commitment,
                checkpoint_block: s.checkpoint_block.saturated_into::<u32>(),
                primary_signers: s.primary_signers.clone(),
                commitment_nonce: s.commitment_nonce,
            }),
            total_snapshots: bucket.total_snapshots,
        })
    }

    /// Query bucket providers.
    pub fn query_bucket_providers(bucket_id: BucketId) -> Vec<T::AccountId> {
        Buckets::<T>::get(bucket_id)
            .map(|bucket| bucket.primary_providers.to_vec())
            .unwrap_or_default()
    }

    /// Query agreement information.
    pub fn query_agreement_info(
        bucket_id: BucketId,
        provider: &T::AccountId,
    ) -> Option<crate::runtime_api::AgreementResponse> {
        StorageAgreements::<T>::get(bucket_id, provider)
            .map(|agreement| agreement_to_response::<T>(bucket_id, provider, agreement))
    }

    /// Query all agreements for a bucket.
    pub fn query_bucket_agreements(
        bucket_id: BucketId,
    ) -> Vec<crate::runtime_api::AgreementResponse> {
        StorageAgreements::<T>::iter_prefix(bucket_id)
            .map(|(provider, agreement)| {
                agreement_to_response::<T>(bucket_id, &provider, agreement)
            })
            .collect()
    }

    /// Query all bucket IDs (paginated).
    pub fn query_bucket_ids(offset: u32, limit: u32) -> Vec<BucketId> {
        Buckets::<T>::iter_keys()
            .skip(offset as usize)
            .take(limit as usize)
            .collect()
    }

    /// Query all agreements for a provider.
    pub fn query_provider_agreements(
        provider: &T::AccountId,
    ) -> Vec<crate::runtime_api::AgreementResponse> {
        StorageAgreements::<T>::iter()
            .filter(|(_, p, _)| p == provider)
            .map(|(bucket_id, _, agreement)| {
                agreement_to_response::<T>(bucket_id, provider, agreement)
            })
            .collect()
    }

    /// Query challenges expiring at a specific deadline. `block` is an anchor
    /// block (see [`Self::current_anchor_block`]), not a parachain height.
    pub fn query_challenges_at(
        block: BlockNumberFor<T>,
    ) -> Vec<crate::runtime_api::ChallengeResponse> {
        Challenges::<T>::iter_prefix(block)
            .map(|(index, challenge)| challenge_to_response::<T>(block, index, challenge))
            .collect()
    }

    /// Query all challenges for a specific bucket.
    pub fn query_bucket_challenges(
        bucket_id: BucketId,
    ) -> Vec<crate::runtime_api::ChallengeResponse> {
        Challenges::<T>::iter()
            .filter(|(_, _, c)| c.bucket_id == bucket_id)
            .map(|(deadline, index, c)| challenge_to_response::<T>(deadline, index, c))
            .collect()
    }

    /// Query all challenges targeting a specific provider.
    pub fn query_provider_challenges(
        provider: &T::AccountId,
    ) -> Vec<crate::runtime_api::ChallengeResponse> {
        Challenges::<T>::iter()
            .filter(|(_, _, c)| &c.provider == provider)
            .map(|(deadline, index, c)| challenge_to_response::<T>(deadline, index, c))
            .collect()
    }

    /// Query all challenges created by a specific challenger.
    pub fn query_challenger_challenges(
        challenger: &T::AccountId,
    ) -> Vec<crate::runtime_api::ChallengeResponse> {
        Challenges::<T>::iter()
            .filter(|(_, _, c)| &c.challenger == challenger)
            .map(|(deadline, index, c)| challenge_to_response::<T>(deadline, index, c))
            .collect()
    }

    /// Check if provider can accept additional bytes.
    pub fn query_can_accept_bytes(provider: &T::AccountId, additional_bytes: u64) -> bool {
        if let Some(provider_info) = Providers::<T>::get(provider) {
            let new_committed_bytes = provider_info
                .committed_bytes
                .saturating_add(additional_bytes);

            // Check capacity constraint
            if provider_info.settings.max_capacity > 0
                && new_committed_bytes > provider_info.settings.max_capacity
            {
                return false;
            }

            let bytes_as_balance: BalanceOf<T> = new_committed_bytes.saturated_into();

            if let Some(required_stake) = T::MinStakePerByte::get().checked_mul(&bytes_as_balance) {
                return provider_info.stake >= required_stake;
            }
        }
        false
    }
}
