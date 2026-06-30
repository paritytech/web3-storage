use crate::*;
use alloc::vec::Vec;
use frame_support::{
    pallet_prelude::*,
    traits::{Currency, ReservableCurrency},
};
use frame_system::pallet_prelude::*;
use sp_core::H256;
use sp_runtime::traits::{Saturating, Zero};
use storage_primitives::{BucketId, ChallengeId};

impl<T: Config> Pallet<T> {
    pub fn create_challenge(
        challenger: T::AccountId,
        bucket_id: BucketId,
        provider: T::AccountId,
        mmr_root: H256,
        start_seq: u64,
        leaf_index: u64,
        chunk_index: u64,
    ) -> DispatchResult {
        // Calculate deposit (simplified - would be based on expected costs)
        let deposit: BalanceOf<T> = 100u32.into();

        T::Currency::reserve(&challenger, deposit)?;

        let current_block = frame_system::Pallet::<T>::block_number();
        let deadline = current_block.saturating_add(T::ChallengeTimeout::get());

        let challenge = Challenge {
            bucket_id,
            provider: provider.clone(),
            challenger: challenger.clone(),
            mmr_root,
            start_seq,
            leaf_index,
            chunk_index,
            deposit,
        };

        let index = Challenges::<T>::mutate(deadline, |challenges| {
            let challenges = challenges.get_or_insert_with(Vec::new);
            let idx = challenges.len() as u16;
            challenges.push(challenge);
            idx
        });

        // Update provider stats
        Providers::<T>::mutate(&provider, |maybe_provider| {
            if let Some(provider_info) = maybe_provider {
                provider_info.stats.challenges_received =
                    provider_info.stats.challenges_received.saturating_add(1);
            }
        });

        let challenge_id = ChallengeId { deadline, index };

        Self::deposit_event(Event::ChallengeCreated {
            challenge_id,
            bucket_id,
            provider,
            challenger,
            respond_by: deadline,
        });

        Ok(())
    }

    /// Slash a provider who failed to respond to a challenge.
    ///
    /// This:
    /// 1. Slashes the provider's entire stake
    /// 2. Refunds the challenger with their deposit plus a reward
    /// 3. Updates provider statistics
    /// 4. Marks the provider as slashed (so they can be removed from buckets)
    pub fn slash_provider_for_failed_challenge(
        challenge: &Challenge<T>,
        challenge_id: ChallengeId<BlockNumberFor<T>>,
    ) {
        // Get provider info
        if let Some(mut provider_info) = Providers::<T>::get(&challenge.provider) {
            // Slash the provider's entire stake
            let slashed_amount = provider_info.stake;

            // Unreserve and slash the stake
            // In Substrate, slashing typically burns or sends to treasury
            let (_, remaining) = T::Currency::slash_reserved(&challenge.provider, slashed_amount);
            let actually_slashed = slashed_amount.saturating_sub(remaining);

            // Calculate challenger reward (e.g., 10% of slashed amount, rest goes to treasury)
            let challenger_reward = actually_slashed / 10u32.into();
            let to_treasury = actually_slashed.saturating_sub(challenger_reward);

            // Refund challenger's deposit
            T::Currency::unreserve(&challenge.challenger, challenge.deposit);

            // Transfer reward to challenger
            // Note: We need to handle potential errors gracefully in on_finalize
            let _ = T::Currency::deposit_creating(&challenge.challenger, challenger_reward);

            // The rest goes to treasury (burned by slash_reserved)
            let _ = to_treasury; // Acknowledged

            // Update provider stats
            provider_info.stats.challenges_failed =
                provider_info.stats.challenges_failed.saturating_add(1);
            provider_info.stake = Zero::zero();

            Providers::<T>::insert(&challenge.provider, provider_info);

            // Emit event
            Self::deposit_event(Event::ChallengeSlashed {
                challenge_id,
                provider: challenge.provider.clone(),
                slashed_amount: actually_slashed,
                challenger_reward,
            });
        }
    }
}
