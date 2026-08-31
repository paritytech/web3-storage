// SPDX-License-Identifier: Apache-2.0

use crate::*;
use frame_support::pallet_prelude::*;
use sp_runtime::traits::{CheckedAdd, CheckedMul, SaturatedConversion, Saturating, Zero};
use storage_primitives::{BucketId, EndAction, ProviderRole, RemovalReason, ReplayError};

impl<T: Config> Pallet<T> {
    pub(crate) fn validate_duration(
        settings: &ProviderSettings<T>,
        duration: BlockNumberFor<T>,
    ) -> DispatchResult {
        ensure!(
            duration >= settings.min_duration,
            Error::<T>::DurationTooShort
        );
        ensure!(
            duration <= settings.max_duration,
            Error::<T>::DurationTooLong
        );
        Ok(())
    }

    pub(crate) fn calculate_payment(
        price_per_byte: BalanceOf<T>,
        max_bytes: u64,
        duration: BlockNumberFor<T>,
    ) -> Result<BalanceOf<T>, DispatchError> {
        // payment = price_per_byte * max_bytes * duration
        // Use saturated_from for type conversions
        let bytes_balance: BalanceOf<T> = max_bytes.saturated_into();
        let duration_u128: u128 = duration.saturated_into();
        let duration_balance: BalanceOf<T> = duration_u128.saturated_into();

        price_per_byte
            .checked_mul(&bytes_balance)
            .and_then(|p| p.checked_mul(&duration_balance))
            .ok_or(Error::<T>::ArithmeticOverflow.into())
    }

    pub(crate) fn finalize_agreement(
        bucket_id: BucketId,
        provider: &T::AccountId,
        agreement: &StorageAgreement<T>,
        action: EndAction,
        is_early: bool,
    ) -> DispatchResult {
        let (to_provider, to_burn) = match action {
            EndAction::Pay => (agreement.payment_locked, Zero::zero()),
            EndAction::Burn { burn_percent } => {
                let burn_percent = burn_percent.min(100);
                let burn_amount = agreement.payment_locked * burn_percent.into() / 100u32.into();
                let pay_amount = agreement.payment_locked.saturating_sub(burn_amount);
                (pay_amount, burn_amount)
            }
        };

        // Both arms above split `payment_locked` exactly, so these two drain
        // the hold.
        Self::settle_payment(&agreement.owner, provider, to_provider)?;
        Self::settle_payment(&agreement.owner, &T::Treasury::get(), to_burn)?;

        // Update provider stats
        Providers::<T>::mutate(provider, |maybe_provider| {
            if let Some(provider_info) = maybe_provider {
                provider_info.committed_bytes = provider_info
                    .committed_bytes
                    .saturating_sub(agreement.max_bytes);

                if to_burn > Zero::zero() {
                    provider_info.stats.agreements_burned =
                        provider_info.stats.agreements_burned.saturating_add(1);
                } else {
                    provider_info.stats.agreements_not_extended = provider_info
                        .stats
                        .agreements_not_extended
                        .saturating_add(1);
                }
            }
        });

        // Remove from primary_providers if primary
        if matches!(agreement.role, ProviderRole::Primary) {
            Buckets::<T>::mutate(bucket_id, |maybe_bucket| {
                if let Some(bucket) = maybe_bucket {
                    // Capture the position before removal so the snapshot's
                    // positional signer bitfield can be re-indexed to match.
                    let pos = bucket.primary_providers.iter().position(|p| p == provider);
                    bucket.primary_providers.retain(|p| p != provider);
                    if let (Some(pos), Some(snapshot)) = (pos, bucket.snapshot.as_mut()) {
                        snapshot.remove_provider_bit(pos);
                    }
                }
            });

            let reason = if is_early {
                RemovalReason::AdminTerminated
            } else {
                RemovalReason::Expired
            };

            Self::deposit_event(Event::PrimaryProviderRemoved {
                bucket_id,
                provider: provider.clone(),
                reason,
            });
        }

        // Remove agreement
        StorageAgreements::<T>::remove(bucket_id, provider);

        Self::deposit_event(Event::AgreementEnded {
            bucket_id,
            provider: provider.clone(),
            payment_to_provider: to_provider,
            burned: to_burn,
        });

        Ok(())
    }

    /// Redeem provider-signed terms (used directly by the
    /// `establish_storage_agreement` extrinsic and by higher-layer pallets that
    /// fold bucket creation into their own flows).
    ///
    /// Verifies the signature, advances the provider's replay window,
    /// then runs the same provider/capacity/stake checks as
    /// `create_bucket_with_storage` before creating the bucket + primary
    /// agreement.
    pub fn establish_storage_agreement_internal(
        owner: &T::AccountId,
        provider: &T::AccountId,
        terms: AgreementTermsOf<T>,
        sig: &sp_runtime::MultiSignature,
    ) -> Result<BucketId, DispatchError> {
        // Origin must match the owner the provider signed for.
        ensure!(&terms.owner == owner, Error::<T>::TermsOwnerMismatch);

        // Primary terms must not be bound to an existing bucket — the
        // bucket is created at redemption.
        ensure!(terms.bucket_id.is_none(), Error::<T>::TermsBucketMismatch);

        // Request's terms.max_bytes must greater than 0
        ensure!(terms.max_bytes > 0, Error::<T>::InvalidMaxBytesRequest);

        // Quote must not be stale and must not exceed the chain-enforced window.
        // `terms.valid_until` must in range [anchor_block, anchor_block + RequestTimeout]
        let anchor_block = Self::current_anchor_block();
        ensure!(terms.valid_until >= anchor_block, Error::<T>::TermsExpired);
        ensure!(
            terms.valid_until <= anchor_block.saturating_add(T::RequestTimeout::get()),
            Error::<T>::TermsValidityTooLong
        );

        // Provider lookup + signature check over
        // blake2_256(PRIMARY_TERM_CONTEXT | SCALE(terms)).
        let provider_info = Providers::<T>::get(provider).ok_or(Error::<T>::ProviderNotFound)?;
        Self::verify_terms_signature(
            &provider_info,
            &terms,
            sig,
            storage_primitives::PRIMARY_TERM_CONTEXT,
        )?;

        // Replay window: at most once per nonce, within the trailing REPLAY_WINDOW_BITS slots.
        ProviderReplayStates::<T>::try_mutate(provider, |window| -> DispatchResult {
            window.try_accept(terms.nonce).map_err(|e| match e {
                ReplayError::AlreadyUsed => Error::<T>::NonceAlreadyUsed,
                ReplayError::TooOld => Error::<T>::NonceTooOld,
            })?;
            Ok(())
        })?;

        // Validate on-chain provider's state then create bucket
        Self::ensure_provider_active(&provider_info)?;
        ensure!(
            provider_info.settings.accepting_primary,
            Error::<T>::ProviderNotAcceptingPrimary
        );
        Self::validate_duration(&provider_info.settings, terms.duration)?;

        let new_committed = provider_info
            .committed_bytes
            .checked_add(terms.max_bytes)
            .ok_or(Error::<T>::ArithmeticOverflow)?;
        if provider_info.settings.max_capacity > 0 {
            ensure!(
                new_committed <= provider_info.settings.max_capacity,
                Error::<T>::CapacityExceeded
            );
        }

        {
            let bytes_as_balance: BalanceOf<T> = new_committed.saturated_into();
            let required_stake = T::MinStakePerByte::get()
                .checked_mul(&bytes_as_balance)
                .ok_or(Error::<T>::ArithmeticOverflow)?;
            ensure!(
                provider_info.stake >= required_stake,
                Error::<T>::InsufficientStakeForBytes
            );
        }

        // Pay at the price the provider signed for.
        let payment =
            Self::calculate_payment(terms.price_per_byte, terms.max_bytes, terms.duration)?;
        Self::hold_payment(owner, payment)?;

        // Bucket creation folded in: owner is sole admin, provider is the
        // bucket's single primary. `create_bucket_internal` emits
        // `BucketCreated` for us.
        let bucket_id = Self::create_bucket_internal(owner, 1, Some(provider))?;

        let expires_at = anchor_block.saturating_add(terms.duration);
        let agreement = StorageAgreement {
            owner: owner.clone(),
            max_bytes: terms.max_bytes,
            payment_locked: payment,
            price_per_byte: terms.price_per_byte,
            expires_at,
            extensions_blocked: false,
            role: ProviderRole::Primary,
            started_at: anchor_block,
        };

        Providers::<T>::mutate(provider, |maybe_provider| {
            if let Some(p) = maybe_provider {
                p.committed_bytes = new_committed;
                p.stats.agreements_total = p.stats.agreements_total.saturating_add(1);
                p.stats.total_bytes_committed = p
                    .stats
                    .total_bytes_committed
                    .saturating_add(terms.max_bytes);
            }
        });
        StorageAgreements::<T>::insert(bucket_id, provider, agreement);

        Self::deposit_event(Event::StorageAgreementEstablished {
            bucket_id,
            provider: provider.clone(),
            owner: owner.clone(),
            terms,
            expires_at,
        });

        Ok(bucket_id)
    }

    /// Redeem provider-signed terms for a replica agreement (used directly
    /// by the `establish_replica_agreement` extrinsic and by higher-layer
    /// pallets that fold replica establishment into their own flows).
    ///
    /// Verifies the signature, advances the provider's replay window, then
    /// runs the provider/capacity/stake checks before opening the replica
    /// agreement on an existing bucket. `terms.replica_params` must be
    /// `Some(_)`.
    pub(crate) fn establish_replica_agreement_internal(
        owner: &T::AccountId,
        bucket_id: BucketId,
        provider: &T::AccountId,
        terms: AgreementTermsOf<T>,
        sig: &sp_runtime::MultiSignature,
    ) -> DispatchResult {
        // Origin must match the owner the provider signed for.
        ensure!(&terms.owner == owner, Error::<T>::TermsOwnerMismatch);

        // The provider's signed quote must be bound to the bucket this
        // extrinsic targets.
        ensure!(
            terms.bucket_id == Some(bucket_id),
            Error::<T>::TermsBucketMismatch
        );

        // Request's terms.max_bytes must greater than 0
        ensure!(terms.max_bytes > 0, Error::<T>::InvalidMaxBytesRequest);

        // Quote must not be stale and must not exceed the chain-enforced window.
        let anchor_block = Self::current_anchor_block();
        ensure!(terms.valid_until >= anchor_block, Error::<T>::TermsExpired);
        ensure!(
            terms.valid_until <= anchor_block.saturating_add(T::RequestTimeout::get()),
            Error::<T>::TermsValidityTooLong
        );

        // Target bucket must exist.
        ensure!(
            Buckets::<T>::contains_key(bucket_id),
            Error::<T>::BucketNotFound
        );

        // No existing agreement for (bucket, provider).
        ensure!(
            !StorageAgreements::<T>::contains_key(bucket_id, provider),
            Error::<T>::AgreementAlreadyExists
        );

        // Replica terms must be present for a replica agreement.
        let replica_terms = terms
            .replica_params
            .as_ref()
            .ok_or(Error::<T>::MissingReplicaTerms)?
            .clone();

        // Provider lookup + signature check over
        // blake2_256(REPLICA_TERM_CONTEXT | SCALE(terms)).
        let provider_info = Providers::<T>::get(provider).ok_or(Error::<T>::ProviderNotFound)?;
        Self::verify_terms_signature(
            &provider_info,
            &terms,
            sig,
            storage_primitives::REPLICA_TERM_CONTEXT,
        )?;

        // Replay window: at most once per nonce, within the trailing REPLAY_WINDOW_BITS slots.
        ProviderReplayStates::<T>::try_mutate(provider, |window| -> DispatchResult {
            window.try_accept(terms.nonce).map_err(|e| match e {
                ReplayError::AlreadyUsed => Error::<T>::NonceAlreadyUsed,
                ReplayError::TooOld => Error::<T>::NonceTooOld,
            })?;
            Ok(())
        })?;

        // Validate on-chain provider's state.
        Self::ensure_provider_active(&provider_info)?;
        // Provider is no longer accept replica node
        let _ = provider_info
            .settings
            .replica_sync_price
            .ok_or(Error::<T>::ProviderNotAcceptingReplicas)?;
        Self::validate_duration(&provider_info.settings, terms.duration)?;

        let new_committed = provider_info
            .committed_bytes
            .checked_add(terms.max_bytes)
            .ok_or(Error::<T>::ArithmeticOverflow)?;
        if provider_info.settings.max_capacity > 0 {
            ensure!(
                new_committed <= provider_info.settings.max_capacity,
                Error::<T>::CapacityExceeded
            );
        }

        {
            let bytes_as_balance: BalanceOf<T> = new_committed.saturated_into();
            let required_stake = T::MinStakePerByte::get()
                .checked_mul(&bytes_as_balance)
                .ok_or(Error::<T>::ArithmeticOverflow)?;
            ensure!(
                provider_info.stake >= required_stake,
                Error::<T>::InsufficientStakeForBytes
            );
        }

        // Pay at the price the provider signed for, plus the sync balance.
        let payment =
            Self::calculate_payment(terms.price_per_byte, terms.max_bytes, terms.duration)?;
        let total_lock = payment
            .checked_add(&replica_terms.sync_balance)
            .ok_or(Error::<T>::ArithmeticOverflow)?;
        Self::hold_payment(owner, total_lock)?;

        let expires_at = anchor_block.saturating_add(terms.duration);
        let agreement = StorageAgreement {
            owner: owner.clone(),
            max_bytes: terms.max_bytes,
            payment_locked: payment,
            price_per_byte: terms.price_per_byte,
            expires_at,
            extensions_blocked: false,
            role: ProviderRole::Replica {
                sync_balance: replica_terms.sync_balance,
                sync_price: replica_terms.sync_price,
                min_sync_interval: replica_terms.min_sync_interval,
                last_sync: None,
            },
            started_at: anchor_block,
        };

        Providers::<T>::mutate(provider, |maybe_provider| {
            if let Some(p) = maybe_provider {
                p.committed_bytes = new_committed;
                p.stats.agreements_total = p.stats.agreements_total.saturating_add(1);
                p.stats.total_bytes_committed = p
                    .stats
                    .total_bytes_committed
                    .saturating_add(terms.max_bytes);
            }
        });
        StorageAgreements::<T>::insert(bucket_id, provider, agreement);

        Self::deposit_event(Event::ReplicaAgreementEstablished {
            bucket_id,
            provider: provider.clone(),
            owner: owner.clone(),
            terms,
            expires_at,
        });

        Ok(())
    }
}
