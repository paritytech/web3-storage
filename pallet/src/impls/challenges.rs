use crate::*;
use frame_support::{
    pallet_prelude::*,
    traits::{Currency, ReservableCurrency},
};
use frame_system::pallet_prelude::*;
use sp_core::H256;
use sp_runtime::traits::{Saturating, Zero};
use storage_primitives::{BucketId, ChallengeId, SlashReason};

impl<T: Config> Pallet<T> {
    pub(crate) fn create_challenge(
        challenger: T::AccountId,
        bucket_id: BucketId,
        provider: T::AccountId,
        mmr_root: H256,
        start_seq: u64,
        leaf_index: u64,
        chunk_index: u64,
    ) -> DispatchResult {
        // Deposit comes from `T::ChallengeDeposit` — a runtime constant
        // sized to make spam expensive without pricing out legitimate
        // challengers. Previously hardcoded `100u32` (1e-10 of a token
        // at 12 decimals), which made challenge spam effectively free.
        let deposit: BalanceOf<T> = T::ChallengeDeposit::get();

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

        // Cap the number of challenges that can share this deadline so the
        // `on_finalize` sweep stays bounded (and within the weight
        // `on_initialize` reserves for it). `NextChallengeIndex(deadline)`
        // is the count ever allocated for that deadline and is never
        // decremented, so it is a tight upper bound on the sweep size.
        ensure!(
            NextChallengeIndex::<T>::get(deadline) < T::MaxChallengesPerDeadline::get(),
            Error::<T>::TooManyChallengesThisBlock
        );

        // Allocate a stable per-deadline index. Unlike the old
        // `Vec`-position scheme, this counter is never decremented when a
        // sibling challenge resolves, so the `ChallengeId` we emit stays
        // valid for the life of the challenge.
        let index = NextChallengeIndex::<T>::mutate(deadline, |n| {
            let i = *n;
            *n = n.saturating_add(1);
            i
        });
        Challenges::<T>::insert(deadline, index, &challenge);

        // Bump the pending-challenge counters. These are decremented
        // exactly once per resolution (defended/invalid-response in
        // `respond_to_challenge`, or timeout in `on_finalize`), so a
        // fully-resolved provider/bucket returns to 0. They gate
        // `complete_deregister` and agreement teardown so a provider can't
        // escape a live challenge.
        PendingChallenges::<T>::mutate(&provider, |n| *n = n.saturating_add(1));
        PendingChallengesByBucket::<T>::mutate(bucket_id, &provider, |n| *n = n.saturating_add(1));

        // Update provider stats
        Providers::<T>::mutate(&provider, |maybe_provider| {
            if let Some(provider_info) = maybe_provider {
                provider_info.stats.challenges_received =
                    provider_info.stats.challenges_received.saturating_add(1);
            }
        });

        // Bump challenger's total_challenges aggregate so the SDK's
        // `get_challenge_stats` doesn't have to scan event history.
        ChallengerStats::<T>::mutate(&challenger, |stats| {
            stats.total_challenges = stats.total_challenges.saturating_add(1);
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

    /// Slash a provider for failing a challenge.
    ///
    /// This:
    /// 1. Slashes the provider's entire stake
    /// 2. Refunds the challenger's deposit plus a 10% slash reward
    /// 3. Updates provider statistics
    /// 4. Emits `ChallengeSlashed` with the supplied `SlashReason`
    ///
    /// `reason` distinguishes a timeout (`on_finalize` path) from an
    /// invalid response (`respond_to_challenge` paths). Both lead to the
    /// same financial outcome — the distinction is for observers
    /// reading the event log.
    pub(crate) fn slash_provider_for_failed_challenge(
        challenge: &Challenge<T>,
        challenge_id: ChallengeId<BlockNumberFor<T>>,
        reason: SlashReason,
    ) {
        // Get provider info
        if let Some(mut provider_info) = Providers::<T>::get(&challenge.provider) {
            // Slash the provider's entire stake
            let slashed_amount = provider_info.stake;

            // Slash the provider's stake, capturing the imbalance so we can
            // settle it into the Treasury instead of burning it.
            let (slashed_imbalance, remaining) =
                T::Currency::slash_reserved(&challenge.provider, slashed_amount);
            let actually_slashed = slashed_amount.saturating_sub(remaining);

            // Per the design, a successful challenger receives NO reward —
            // only their deposit back. Refund the deposit and route the
            // entire slashed amount to the Treasury. Paying the challenger
            // a cut of the slash would create a profit-from-slashing
            // incentive (the "refund me or I burn" blackmail channel the
            // design explicitly closes). `resolve_creating` restores the
            // issuance burned by `slash_reserved`, keeping issuance whole.
            T::Currency::unreserve(&challenge.challenger, challenge.deposit);
            T::Currency::resolve_creating(&T::Treasury::get(), slashed_imbalance);

            // Update provider stats
            provider_info.stats.challenges_failed =
                provider_info.stats.challenges_failed.saturating_add(1);
            provider_info.stake = Zero::zero();

            Providers::<T>::insert(&challenge.provider, provider_info);

            // Bump the challenger's successful-challenge count. Challengers
            // earn no reward (the slashed stake goes entirely to the
            // Treasury), so only the counter moves here.
            ChallengerStats::<T>::mutate(&challenge.challenger, |stats| {
                stats.successful_challenges = stats.successful_challenges.saturating_add(1);
            });

            // Emit event
            Self::deposit_event(Event::ChallengeSlashed {
                challenge_id,
                provider: challenge.provider.clone(),
                slashed_amount: actually_slashed,
                challenger_reward: Zero::zero(),
                reason,
            });
        }
    }

    /// Decrement both pending-challenge counters for a resolved
    /// `(bucket, provider)` challenge. Called from the two resolution
    /// sites — `respond_to_challenge` (after the `take` consumes the
    /// challenge, covering both the defended and invalid-response paths)
    /// and `on_finalize` (per drained timed-out challenge) — never from
    /// `slash_provider_for_failed_challenge`, which both sites share and
    /// which would otherwise double-count. `saturating_sub` keeps the
    /// counters non-negative even if invariants are ever violated.
    pub(crate) fn decrement_pending(bucket_id: BucketId, provider: &T::AccountId) {
        PendingChallenges::<T>::mutate(provider, |n| *n = n.saturating_sub(1));
        PendingChallengesByBucket::<T>::mutate(bucket_id, provider, |n| *n = n.saturating_sub(1));
    }
}
