// SPDX-License-Identifier: Apache-2.0

use crate::*;
use frame_support::pallet_prelude::*;
use sp_runtime::traits::Saturating;
use storage_primitives::{
    BucketId, ChallengeId, ChunkLocation, Commitment, ProviderRole, Visibility,
};

impl<T: Config> Pallet<T> {
    /// Open a challenge against `provider`'s signature on the bucket's current
    /// snapshot and return the new challenge's id. Shared by the
    /// `challenge_checkpoint` call and the precompile, which hands the id back
    /// to the contract.
    pub fn do_challenge_checkpoint(
        who: T::AccountId,
        bucket_id: BucketId,
        provider: T::AccountId,
        target: ChunkLocation,
    ) -> Result<ChallengeId<BlockNumberFor<T>>, DispatchError> {
        let bucket = Buckets::<T>::get(bucket_id).ok_or(Error::<T>::BucketNotFound)?;
        let snapshot = bucket.snapshot.as_ref().ok_or(Error::<T>::NoSnapshot)?;

        let provider_idx = bucket
            .primary_providers
            .iter()
            .position(|p| p == &provider)
            .ok_or(Error::<T>::ProviderNotInSnapshot)?;
        ensure!(
            snapshot.has_provider_signed(provider_idx),
            Error::<T>::ProviderNotInSnapshot
        );

        // A challenge can only open while the agreement is live (not into the
        // settlement window), so an expired checkpoint can no longer be
        // challenged.
        let agreement = StorageAgreements::<T>::get(bucket_id, &provider)
            .ok_or(Error::<T>::AgreementNotFound)?;
        ensure!(
            Self::current_anchor_block() < agreement.expires_at,
            Error::<T>::AgreementExpired
        );

        Self::create_challenge(
            who,
            bucket_id,
            &bucket,
            provider,
            &agreement.role,
            snapshot.commitment,
            target,
        )
    }

    pub(crate) fn create_challenge(
        challenger: T::AccountId,
        bucket_id: BucketId,
        bucket: &Bucket<T>,
        provider: T::AccountId,
        provider_role: &ProviderRole<BalanceOf<T>, BlockNumberFor<T>>,
        commitment: Commitment,
        target: ChunkLocation,
    ) -> Result<ChallengeId<BlockNumberFor<T>>, DispatchError> {
        ensure!(challenger != provider, Error::<T>::SelfChallenge);

        // Private-bucket gate: the public has no legitimate reliance on data
        // it cannot read, and must not be able to force private bytes
        // on-chain via the response. Keyed on the challenged provider's role
        // in its *current* agreement; replicas stay challengeable by anyone
        // (their content is public — the anti-censorship guarantee).
        if bucket.visibility == Visibility::Private
            && matches!(provider_role, ProviderRole::Primary)
        {
            ensure!(
                Self::is_authorized_for_private(&challenger, bucket_id, bucket),
                Error::<T>::NotAuthorizedForPrivateBucket
            );
        }

        // Challenger tier, evaluated once here and snapshotted in the
        // challenge: membership/agreement changes between creation and
        // response cannot alter the fee split in `respond_to_challenge`.
        let authorized = Self::is_authorized(&challenger, bucket_id, bucket);

        // Deposit comes from `T::ChallengeDeposit` — a runtime constant
        // sized to make spam expensive without pricing out legitimate
        // challengers. Previously hardcoded `100u32` (1e-10 of a token
        // at 12 decimals), which made challenge spam effectively free.
        let deposit: BalanceOf<T> = T::ChallengeDeposit::get();

        Self::hold_challenge_deposit(&challenger, deposit)?;

        let anchor_block = Self::current_anchor_block();
        let deadline = anchor_block.saturating_add(T::ChallengeTimeout::get());

        let challenge = Challenge {
            bucket_id,
            provider: provider.clone(),
            challenger: challenger.clone(),
            mmr_root: commitment.mmr_root,
            start_seq: commitment.start_seq,
            target,
            deposit,
            authorized,
        };

        // Cap the number of challenges that can share this deadline.
        // `NextChallengeIndex(deadline)` is the count ever allocated for that
        // deadline and is never decremented, so it is a tight upper bound.
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
        // exactly once per resolution (defended in `respond_to_challenge`,
        // or timed out in `resolve_expired_challenge`), so a
        // fully-resolved provider/bucket returns to 0. They gate
        // `complete_deregister` and agreement teardown so a provider can't
        // escape a live challenge.
        PendingChallenges::<T>::mutate(&provider, |n| *n = n.saturating_add(1));
        PendingChallengesByBucket::<T>::mutate(bucket_id, &provider, |n| *n = n.saturating_add(1));

        let challenge_id = ChallengeId { deadline, index };

        Self::deposit_event(Event::ChallengeCreated {
            challenge_id,
            bucket_id,
            provider,
            challenger,
            respond_by: deadline,
        });

        Ok(challenge_id)
    }

    /// Slash a provider for a challenge that expired unanswered: the whole
    /// stake goes to the Treasury, the challenger's deposit is released in
    /// full (no reward), `challenges_failed` is bumped and `ChallengeSlashed`
    /// is emitted. Does not touch the pending counters; the caller does.
    pub(crate) fn slash_provider_for_failed_challenge(
        challenge: &Challenge<T>,
        challenge_id: ChallengeId<BlockNumberFor<T>>,
    ) {
        Providers::<T>::mutate(&challenge.provider, |maybe_provider| {
            let Some(provider_info) = maybe_provider else {
                return;
            };

            // Slash the provider's entire held stake into the Treasury rather
            // than burning it, keeping total issuance whole.
            let actually_slashed =
                Self::slash_stake_to_treasury(&challenge.provider, provider_info.stake);

            Self::release_challenge_deposit(&challenge.challenger, challenge.deposit);

            provider_info.stats.challenges_failed =
                provider_info.stats.challenges_failed.saturating_add(1);
            // BestEffort slash: record what actually moved, so a residual hold
            // can never go unaccounted. An under-slash (ruled out by
            // `try_state`) would keep `remove_slashed` gated on purpose —
            // bookkeeping honesty over guaranteed cleanup.
            provider_info.stake = provider_info.stake.saturating_sub(actually_slashed);

            Self::deposit_event(Event::ChallengeSlashed {
                challenge_id,
                provider: challenge.provider.clone(),
                slashed_amount: actually_slashed,
            });
        });
    }

    /// Marks one challenge against `provider` on `bucket_id` as resolved.
    /// A counter that reaches 0 is deleted, so both maps only contain
    /// providers with an open challenge.
    pub(crate) fn decrement_pending(bucket_id: BucketId, provider: &T::AccountId) {
        PendingChallenges::<T>::mutate_exists(provider, Self::decrement_or_delete);
        PendingChallengesByBucket::<T>::mutate_exists(
            bucket_id,
            provider,
            Self::decrement_or_delete,
        );
    }

    fn decrement_or_delete(count: &mut Option<u32>) {
        *count = count.and_then(|n| n.checked_sub(1)).filter(|n| *n > 0);
    }

    /// Fails while any provider has an open challenge on `bucket_id`.
    pub(crate) fn ensure_no_pending_challenge(bucket_id: BucketId) -> DispatchResult {
        ensure!(
            PendingChallengesByBucket::<T>::iter_key_prefix(bucket_id)
                .next()
                .is_none(),
            Error::<T>::BucketHasPendingChallenge
        );
        Ok(())
    }
}
