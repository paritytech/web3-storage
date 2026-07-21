// SPDX-License-Identifier: Apache-2.0

use crate::*;
use frame_support::{
    pallet_prelude::*,
    traits::{Currency, ReservableCurrency},
};
use sp_core::H256;
use sp_runtime::traits::{One, Saturating, Zero};
use storage_primitives::{BucketId, ChallengeId, ChunkLocation, SlashReason};

impl<T: Config> Pallet<T> {
    /// The `on_initialize` slash sweep: slash providers whose challenges expired
    /// unanswered. See the [`Hooks::on_initialize`](crate::pallet) doc for the
    /// full rationale (which deadline keys are final, the budget model, why this
    /// runs in `on_initialize`). Split from the hook into range-resolution
    /// ([`Self::challenge_sweep_range`]) and per-key drain
    /// ([`Self::slash_expired_at`]) so each step reads and tests on its own.
    pub(crate) fn sweep_expired_challenges() -> Weight {
        // Provider read + cursor read/write, charged up front regardless of the
        // range resolved below.
        let mut weight = T::DbWeight::get().reads_writes(2, 1);

        let (first, end) = match Self::challenge_sweep_range() {
            Some(range) => range,
            None => return weight,
        };

        // Per-block slash budget, capped so one maturing deadline cannot eat the
        // whole block's PoV. Lower bound of 1 so a (nonsensical) zero cap cannot
        // park the cursor forever.
        let mut budget =
            u32::from(T::MaxChallengesPerDeadline::get()).clamp(1, MAX_SWEEP_SLASH_BUDGET);

        let mut key = first;
        while key <= end {
            let count = Self::slash_expired_at(key, budget);
            budget = budget.saturating_sub(count);
            // Benchmarked for a single key's drain; applying it per key
            // over-charges the fixed overhead. Conservative.
            weight = weight.saturating_add(T::WeightInfo::on_initialize_slash_challenges(count));

            if budget == 0 {
                // Budget exhausted, possibly mid-key (`drain_prefix` removed the
                // entries already visited). Keep the key's `NextChallengeIndex`
                // allocator and park the cursor just below it; the remainder and
                // the allocator cleanup drain on later blocks.
                LastSweptChallengeBlock::<T>::put(key.saturating_sub(One::one()));
                return weight;
            }
            // Clear the per-deadline index allocator now the deadline has passed;
            // no further challenges can target this key.
            NextChallengeIndex::<T>::remove(key);
            key = key.saturating_add(One::one());
        }

        LastSweptChallengeBlock::<T>::put(end);
        weight
    }

    /// Inclusive range of deadline keys to drain this block, or `None` when
    /// there is nothing to do (clock not ready, or cursor already caught up).
    /// On the first run it anchors [`LastSweptChallengeBlock`] at the current
    /// height (rather than scanning from zero — relay numbers start in the
    /// millions) and returns `None`.
    fn challenge_sweep_range() -> Option<(BlockNumberFor<T>, BlockNumberFor<T>)> {
        let now = Self::current_anchor_block();
        if now.is_zero() {
            return None;
        }
        // In `on_initialize` the validation-data inherent has not run, so `now`
        // is the relay parent of the *previous* parachain block; keys strictly
        // below it are unrespondable (see the hook doc), so sweep up to
        // `now - 1`.
        let sweepable = now.saturating_sub(One::one());

        let last = match LastSweptChallengeBlock::<T>::get() {
            Some(last) => last,
            None => {
                LastSweptChallengeBlock::<T>::put(sweepable);
                return None;
            }
        };
        if sweepable <= last {
            return None;
        }

        // Cap the span probed per block; the remainder carries over via the
        // cursor on later blocks.
        let end = sweepable.min(last.saturating_add(MAX_SWEEP_SPAN.into()));
        Some((last.saturating_add(One::one()), end))
    }

    /// Drain up to `budget` challenges at deadline `key`, slashing each provider
    /// for failing to respond, and return the number slashed. `drain_prefix`
    /// removes entries as it iterates, so breaking at the budget leaves the
    /// unvisited siblings in place to drain on a later block.
    fn slash_expired_at(key: BlockNumberFor<T>, budget: u32) -> u32 {
        let mut count: u32 = 0;
        for (index, challenge) in Challenges::<T>::drain_prefix(key) {
            let challenge_id = ChallengeId {
                deadline: key,
                index,
            };
            // Timeout is a resolution: decrement the pending counters exactly
            // once here, mirroring the increment in `create_challenge`. (The
            // slash helper is shared with the invalid-response path, so it must
            // NOT touch the counters.)
            Self::decrement_pending(challenge.bucket_id, &challenge.provider);
            Self::slash_provider_for_failed_challenge(
                &challenge,
                challenge_id,
                SlashReason::Timeout,
            );
            count = count.saturating_add(1);
            if count >= budget {
                break;
            }
        }
        count
    }

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

        let anchor_block = Self::current_anchor_block();
        let deadline = anchor_block.saturating_add(T::ChallengeTimeout::get());

        let challenge = ChallengeOf::<T> {
            bucket_id,
            provider: provider.clone(),
            challenger: challenger.clone(),
            mmr_root,
            start_seq,
            target: ChunkLocation {
                leaf_index,
                chunk_index,
            },
            deposit,
        };

        // Cap the number of challenges that can share this deadline so the
        // `on_initialize` sweep's per-key drain stays bounded.
        // `NextChallengeIndex(deadline)` is the count ever allocated for that
        // deadline and is never decremented, so it is a tight upper bound on
        // the per-key sweep size.
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
        // `respond_to_challenge`, or timeout in the `on_initialize` sweep), so a
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
    /// 1. Slashes the provider's entire stake, routing it to the Treasury
    /// 2. Refunds the challenger's deposit (no reward — see body)
    /// 3. Updates provider statistics
    /// 4. Emits `ChallengeSlashed` with the supplied `SlashReason`
    ///
    /// `reason` distinguishes a timeout (`on_initialize` sweep) from an
    /// invalid response (`respond_to_challenge` paths). Both lead to the
    /// same financial outcome — the distinction is for observers
    /// reading the event log.
    pub(crate) fn slash_provider_for_failed_challenge(
        challenge: &ChallengeOf<T>,
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
    /// and the `on_initialize` sweep (per drained timed-out challenge) — never from
    /// `slash_provider_for_failed_challenge`, which both sites share and
    /// which would otherwise double-count. `saturating_sub` keeps the
    /// counters non-negative even if invariants are ever violated.
    pub(crate) fn decrement_pending(bucket_id: BucketId, provider: &T::AccountId) {
        PendingChallenges::<T>::mutate(provider, |n| *n = n.saturating_sub(1));
        PendingChallengesByBucket::<T>::mutate(bucket_id, provider, |n| *n = n.saturating_sub(1));
    }
}
