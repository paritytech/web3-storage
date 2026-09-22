// SPDX-License-Identifier: Apache-2.0

use crate::*;
use frame_support::pallet_prelude::*;
use sp_runtime::traits::{CheckedAdd, CheckedMul, SaturatedConversion, Saturating, Zero};
use storage_primitives::{
    BucketId, BucketTarget, EndAction, ProviderRole, RemovalReason, ReplayError, Visibility,
};

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
        // the storage-fee part of the hold.
        Self::settle_payment(&agreement.owner, provider, to_provider)?;
        Self::settle_payment(&agreement.owner, &T::Treasury::get(), to_burn)?;

        // A replica's unspent sync balance is escrowed alongside the fee; the
        // agreement record is about to go, so return it to the owner rather
        // than stranding it on hold.
        if let ProviderRole::Replica { sync_balance, .. } = &agreement.role {
            Self::release_payment(&agreement.owner, *sync_balance)?;
        }

        // Update provider stats
        Providers::<T>::mutate(provider, |maybe_provider| {
            if let Some(provider_info) = maybe_provider {
                provider_info.committed_bytes = provider_info
                    .committed_bytes
                    .saturating_sub(agreement.max_bytes);
                provider_info.stats.lifetime_revenue = provider_info
                    .stats
                    .lifetime_revenue
                    .saturating_add(to_provider);

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

    /// Creates a bucket and opens its first primary agreement atomically.
    ///
    /// Used by the `create_bucket_with_primary` extrinsic and by higher-layer
    /// pallets that fold bucket creation into their own flows. The quote must
    /// name [`BucketTarget::New`].
    pub fn create_bucket_with_primary_internal(
        owner: &T::AccountId,
        provider: &T::AccountId,
        terms: AgreementTermsOf<T>,
        sig: &sp_runtime::MultiSignature,
        visibility: Visibility,
    ) -> Result<BucketId, DispatchError> {
        Self::open_primary_agreement(owner, provider, terms, sig, BucketTarget::New, || {
            Self::create_bucket_internal(owner, 1, Some(provider), visibility)
        })
    }

    /// Adds a primary provider to an existing bucket. `admin` must be a bucket
    /// admin; the quote must name [`BucketTarget::Existing`] with `bucket_id`.
    pub fn add_primary_provider_internal(
        admin: &T::AccountId,
        bucket_id: BucketId,
        provider: &T::AccountId,
        terms: AgreementTermsOf<T>,
        sig: &sp_runtime::MultiSignature,
    ) -> DispatchResult {
        let bucket = Buckets::<T>::get(bucket_id).ok_or(Error::<T>::BucketNotFound)?;
        Self::ensure_admin(admin, &bucket)?;
        ensure!(
            !StorageAgreements::<T>::contains_key(bucket_id, provider),
            Error::<T>::AgreementAlreadyExists
        );
        ensure!(
            (bucket.primary_providers.len() as u32) < T::MaxPrimaryProviders::get(),
            Error::<T>::MaxPrimaryProvidersReached
        );

        Self::open_primary_agreement(
            admin,
            provider,
            terms,
            sig,
            BucketTarget::Existing(bucket_id),
            || Ok(bucket_id),
        )?;

        // Appended, never inserted: the snapshot's signer bitfield is indexed
        // by position, so the existing entries must keep theirs. Last, so the
        // quote is fully accepted before the bucket changes.
        Buckets::<T>::try_mutate(bucket_id, |maybe_bucket| -> DispatchResult {
            let bucket = maybe_bucket.as_mut().ok_or(Error::<T>::BucketNotFound)?;
            bucket
                .primary_providers
                .try_push(provider.clone())
                .map_err(|_| Error::<T>::MaxPrimaryProvidersReached)?;
            Ok(())
        })
    }

    /// Opens a replica agreement on an existing bucket. Anyone the provider
    /// quoted for may redeem it; the quote must name
    /// [`BucketTarget::Existing`] with `bucket_id` and carry
    /// `replica_params`.
    pub(crate) fn add_replica_provider_internal(
        owner: &T::AccountId,
        bucket_id: BucketId,
        provider: &T::AccountId,
        terms: AgreementTermsOf<T>,
        sig: &sp_runtime::MultiSignature,
    ) -> DispatchResult {
        let anchor_block = Self::validate_terms(owner, &terms, BucketTarget::Existing(bucket_id))?;

        ensure!(
            Buckets::<T>::contains_key(bucket_id),
            Error::<T>::BucketNotFound
        );
        ensure!(
            !StorageAgreements::<T>::contains_key(bucket_id, provider),
            Error::<T>::AgreementAlreadyExists
        );

        let replica_terms = terms
            .replica_params
            .as_ref()
            .ok_or(Error::<T>::MissingReplicaTerms)?
            .clone();

        let provider_info = Self::accept_quote(
            provider,
            &terms,
            sig,
            storage_primitives::REPLICA_TERM_CONTEXT,
        )?;
        Self::ensure_provider_active(&provider_info)?;
        ensure!(
            provider_info.settings.replica_sync_price.is_some(),
            Error::<T>::ProviderNotAcceptingReplicas
        );
        let new_committed = Self::reserve_capacity(&provider_info, &terms)?;

        // Pay at the price the provider signed for, plus the sync balance.
        let payment =
            Self::calculate_payment(terms.price_per_byte, terms.max_bytes, terms.duration)?;
        let total_lock = payment
            .checked_add(&replica_terms.sync_balance)
            .ok_or(Error::<T>::ArithmeticOverflow)?;
        Self::hold_payment(owner, total_lock)?;

        let expires_at = anchor_block.saturating_add(terms.duration);
        Self::record_agreement(
            bucket_id,
            provider,
            new_committed,
            StorageAgreement {
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
            },
        );

        Self::deposit_event(Event::ReplicaAgreementEstablished {
            bucket_id,
            provider: provider.clone(),
            owner: owner.clone(),
            terms,
            expires_at,
        });

        Ok(())
    }

    /// Checks the parts of a quote that do not depend on the provider's
    /// on-chain record: it binds this owner and this bucket, asks for a
    /// non-zero quota, and is still inside the chain-enforced validity
    /// window.
    ///
    /// Returns the current anchor block.
    fn validate_terms(
        owner: &T::AccountId,
        terms: &AgreementTermsOf<T>,
        target: BucketTarget,
    ) -> Result<BlockNumberFor<T>, DispatchError> {
        ensure!(&terms.owner == owner, Error::<T>::TermsOwnerMismatch);
        ensure!(terms.bucket == target, Error::<T>::TermsBucketMismatch);
        ensure!(terms.max_bytes > 0, Error::<T>::InvalidMaxBytesRequest);

        let anchor_block = Self::current_anchor_block();
        ensure!(terms.valid_until >= anchor_block, Error::<T>::TermsExpired);
        ensure!(
            terms.valid_until <= anchor_block.saturating_add(T::RequestTimeout::get()),
            Error::<T>::TermsValidityTooLong
        );

        Ok(anchor_block)
    }

    /// Verifies the provider's signature over `blake2_256(context |
    /// SCALE(terms))` and consumes the quote's nonce in the provider's replay
    /// window, so a signed quote is redeemable at most once.
    ///
    /// Returns the provider's on-chain record.
    fn accept_quote(
        provider: &T::AccountId,
        terms: &AgreementTermsOf<T>,
        sig: &sp_runtime::MultiSignature,
        context: &[u8],
    ) -> Result<ProviderInfo<T>, DispatchError> {
        let provider_info = Providers::<T>::get(provider).ok_or(Error::<T>::ProviderNotFound)?;
        Self::verify_terms_signature(&provider_info, terms, sig, context)?;

        ProviderReplayStates::<T>::try_mutate(provider, |window| -> DispatchResult {
            window.try_accept(terms.nonce).map_err(|e| match e {
                ReplayError::AlreadyUsed => Error::<T>::NonceAlreadyUsed,
                ReplayError::TooOld => Error::<T>::NonceTooOld,
            })?;
            Ok(())
        })?;

        Ok(provider_info)
    }

    /// Checks the provider can take the quote's duration and quota on top of
    /// what it already committed, and that its stake still backs the total.
    ///
    /// Returns the provider's `committed_bytes` with the quota added.
    fn reserve_capacity(
        provider_info: &ProviderInfo<T>,
        terms: &AgreementTermsOf<T>,
    ) -> Result<u64, DispatchError> {
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

        let bytes_as_balance: BalanceOf<T> = new_committed.saturated_into();
        let required_stake = T::MinStakePerByte::get()
            .checked_mul(&bytes_as_balance)
            .ok_or(Error::<T>::ArithmeticOverflow)?;
        ensure!(
            provider_info.stake >= required_stake,
            Error::<T>::InsufficientStakeForBytes
        );

        Ok(new_committed)
    }

    /// Stores the agreement and moves the provider's counters to match.
    fn record_agreement(
        bucket_id: BucketId,
        provider: &T::AccountId,
        new_committed: u64,
        agreement: StorageAgreement<T>,
    ) {
        let max_bytes = agreement.max_bytes;
        Providers::<T>::mutate(provider, |maybe_provider| {
            if let Some(p) = maybe_provider {
                p.committed_bytes = new_committed;
                p.stats.agreements_total = p.stats.agreements_total.saturating_add(1);
                p.stats.total_bytes_committed =
                    p.stats.total_bytes_committed.saturating_add(max_bytes);
            }
        });
        StorageAgreements::<T>::insert(bucket_id, provider, agreement);
    }

    /// Redeems provider-signed primary terms and records the agreement.
    ///
    /// `target` is the [`BucketTarget`] the quote must name. `bucket` runs
    /// once the quote is accepted and the payment held, and returns the bucket
    /// the agreement goes on: the caller either creates it there
    /// ([`BucketTarget::New`]) or returns one it has already checked
    /// ([`BucketTarget::Existing`]). Nothing is written before the quote is
    /// accepted, so a rejected quote always reports its own error.
    fn open_primary_agreement(
        owner: &T::AccountId,
        provider: &T::AccountId,
        terms: AgreementTermsOf<T>,
        sig: &sp_runtime::MultiSignature,
        target: BucketTarget,
        bucket: impl FnOnce() -> Result<BucketId, DispatchError>,
    ) -> Result<BucketId, DispatchError> {
        let anchor_block = Self::validate_terms(owner, &terms, target)?;
        ensure!(
            terms.replica_params.is_none(),
            Error::<T>::UnexpectedReplicaTerms
        );

        let provider_info = Self::accept_quote(
            provider,
            &terms,
            sig,
            storage_primitives::PRIMARY_TERM_CONTEXT,
        )?;
        Self::ensure_provider_active(&provider_info)?;
        ensure!(
            provider_info.settings.accepting_primary,
            Error::<T>::ProviderNotAcceptingPrimary
        );
        let new_committed = Self::reserve_capacity(&provider_info, &terms)?;

        let payment =
            Self::calculate_payment(terms.price_per_byte, terms.max_bytes, terms.duration)?;
        Self::hold_payment(owner, payment)?;

        let bucket_id = bucket()?;

        let expires_at = anchor_block.saturating_add(terms.duration);
        Self::record_agreement(
            bucket_id,
            provider,
            new_committed,
            StorageAgreement {
                owner: owner.clone(),
                max_bytes: terms.max_bytes,
                payment_locked: payment,
                price_per_byte: terms.price_per_byte,
                expires_at,
                extensions_blocked: false,
                role: ProviderRole::Primary,
                started_at: anchor_block,
            },
        );

        Self::deposit_event(Event::ProviderAddedToBucket {
            bucket_id,
            provider: provider.clone(),
        });
        Self::deposit_event(Event::StorageAgreementEstablished {
            bucket_id,
            provider: provider.clone(),
            owner: owner.clone(),
            terms,
            expires_at,
        });

        Ok(bucket_id)
    }
}
