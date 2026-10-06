// SPDX-License-Identifier: Apache-2.0

use crate::*;
use frame_support::pallet_prelude::*;
use sp_runtime::traits::{CheckedAdd, CheckedMul, SaturatedConversion, Saturating, Zero};
use storage_primitives::{
    BucketId, BucketTarget, EndAction, ProviderRole, RemovalReason, Visibility,
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

    /// Creates a bucket with `provider` as its first primary.
    ///
    /// The quote must name [`BucketTarget::New`]. The bucket is written only
    /// after the quote is accepted and the payment held.
    pub fn create_bucket_with_primary_internal(
        owner: &T::AccountId,
        provider: &T::AccountId,
        terms: AgreementTermsOf<T>,
        sig: &sp_runtime::MultiSignature,
        visibility: Visibility,
    ) -> Result<BucketId, DispatchError> {
        let accepted = Self::accept_quote(
            owner,
            provider,
            &terms,
            sig,
            BucketTarget::New,
            QuoteKind::Primary,
        )?;
        // min_providers = 1: the single primary signs every checkpoint.
        let bucket_id = Self::create_bucket_internal(owner, 1, Some(provider), visibility)?;
        Self::insert_primary_agreement(bucket_id, owner, provider, terms, accepted);
        Ok(bucket_id)
    }

    /// Adds a primary provider to an existing bucket. `admin` must be a bucket
    /// admin; the quote must name [`BucketTarget::Existing`] with `bucket_id`.
    pub(crate) fn add_primary_provider_internal(
        admin: &T::AccountId,
        bucket_id: BucketId,
        provider: &T::AccountId,
        terms: AgreementTermsOf<T>,
        sig: &sp_runtime::MultiSignature,
    ) -> DispatchResult {
        let mut bucket = Buckets::<T>::get(bucket_id).ok_or(Error::<T>::BucketNotFound)?;
        Self::ensure_admin(admin, &bucket)?;
        bucket
            .primary_providers
            .try_push(provider.clone())
            .map_err(|_| Error::<T>::MaxPrimaryProvidersReached)?;

        let accepted = Self::accept_quote(
            admin,
            provider,
            &terms,
            sig,
            BucketTarget::Existing(bucket_id),
            QuoteKind::Primary,
        )?;
        Buckets::<T>::insert(bucket_id, bucket);
        Self::insert_primary_agreement(bucket_id, admin, provider, terms, accepted);
        Ok(())
    }

    /// Opens a replica agreement on an existing bucket. Anyone the provider
    /// quoted for may redeem it; the quote must name
    /// [`BucketTarget::Existing`] with `bucket_id` and contain
    /// `replica_params`.
    pub(crate) fn add_replica_provider_internal(
        owner: &T::AccountId,
        bucket_id: BucketId,
        provider: &T::AccountId,
        terms: AgreementTermsOf<T>,
        sig: &sp_runtime::MultiSignature,
    ) -> DispatchResult {
        let accepted = Self::accept_quote(
            owner,
            provider,
            &terms,
            sig,
            BucketTarget::Existing(bucket_id),
            QuoteKind::Replica,
        )?;
        let expires_at = Self::insert_agreement(bucket_id, owner, provider, &terms, accepted);

        Self::deposit_event(Event::ReplicaAgreementEstablished {
            bucket_id,
            provider: provider.clone(),
            owner: owner.clone(),
            terms,
            expires_at,
        });
        Ok(())
    }

    /// Checks owner, bucket target, `max_bytes > 0` and the validity window.
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

    /// Runs the shared quote checks for `kind` and `target` (terms, bucket,
    /// signature, provider state, capacity), advances the owner's agreement
    /// nonce and holds the payment (plus the sync balance for a replica).
    /// Writes nothing else.
    fn accept_quote(
        owner: &T::AccountId,
        provider: &T::AccountId,
        terms: &AgreementTermsOf<T>,
        sig: &sp_runtime::MultiSignature,
        target: BucketTarget,
        kind: QuoteKind,
    ) -> Result<AcceptedQuote<T>, DispatchError> {
        let anchor_block = Self::validate_terms(owner, terms, target)?;
        if let BucketTarget::Existing(bucket_id) = target {
            ensure!(
                Buckets::<T>::contains_key(bucket_id),
                Error::<T>::BucketNotFound
            );
            ensure!(
                !StorageAgreements::<T>::contains_key(bucket_id, provider),
                Error::<T>::AgreementAlreadyExists
            );
        }

        // A replica's `sync_balance` is held with the payment;
        // `finalize_agreement` releases it.
        let (role, sync_balance) = match kind {
            QuoteKind::Primary => {
                ensure!(
                    terms.replica_params.is_none(),
                    Error::<T>::UnexpectedReplicaTerms
                );
                (ProviderRole::Primary, Zero::zero())
            }
            QuoteKind::Replica => {
                let replica = terms
                    .replica_params
                    .as_ref()
                    .ok_or(Error::<T>::MissingReplicaTerms)?;
                (
                    ProviderRole::Replica {
                        sync_balance: replica.sync_balance,
                        sync_price: replica.sync_price,
                        min_sync_interval: replica.min_sync_interval,
                        last_sync: None,
                    },
                    replica.sync_balance,
                )
            }
        };

        // Signature over `blake2_256(context | SCALE(terms))`, then the nonce:
        // it must match the owner's next expected value, and advancing it
        // makes a signed quote redeemable at most once.
        let provider_info = Providers::<T>::get(provider).ok_or(Error::<T>::ProviderNotFound)?;
        Self::verify_terms_signature(&provider_info, terms, sig, kind.context())?;
        ensure!(
            terms.nonce == AgreementNonces::<T>::get(owner),
            Error::<T>::NonceMismatch
        );
        AgreementNonces::<T>::mutate(owner, |n| *n = n.saturating_add(1));

        Self::ensure_provider_active(&provider_info)?;
        match kind {
            QuoteKind::Primary => ensure!(
                provider_info.settings.accepting_primary,
                Error::<T>::ProviderNotAcceptingPrimary
            ),
            QuoteKind::Replica => ensure!(
                provider_info.settings.replica_sync_price.is_some(),
                Error::<T>::ProviderNotAcceptingReplicas
            ),
        }
        let new_committed = Self::check_capacity(&provider_info, terms)?;

        // Pay at the price the provider signed for.
        let payment =
            Self::calculate_payment(terms.price_per_byte, terms.max_bytes, terms.duration)?;
        let total_hold = payment
            .checked_add(&sync_balance)
            .ok_or(Error::<T>::ArithmeticOverflow)?;
        Self::hold_payment(owner, total_hold)?;

        Ok(AcceptedQuote {
            anchor_block,
            new_committed,
            payment,
            role,
        })
    }

    /// Checks duration, capacity and stake for `terms`. Returns the
    /// provider's `committed_bytes` with `terms.max_bytes` added.
    fn check_capacity(
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

    /// Stores the agreement an accepted quote pays for and updates the
    /// provider's counters. Returns the agreement's `expires_at`.
    fn insert_agreement(
        bucket_id: BucketId,
        owner: &T::AccountId,
        provider: &T::AccountId,
        terms: &AgreementTermsOf<T>,
        accepted: AcceptedQuote<T>,
    ) -> BlockNumberFor<T> {
        let AcceptedQuote {
            anchor_block,
            new_committed,
            payment,
            role,
        } = accepted;
        let expires_at = anchor_block.saturating_add(terms.duration);

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

        StorageAgreements::<T>::insert(
            bucket_id,
            provider,
            StorageAgreement {
                owner: owner.clone(),
                max_bytes: terms.max_bytes,
                payment_locked: payment,
                price_per_byte: terms.price_per_byte,
                expires_at,
                extensions_blocked: false,
                role,
                started_at: anchor_block,
            },
        );
        expires_at
    }

    /// [`Pallet::insert_agreement`] plus the primary events,
    /// `ProviderAddedToBucket` then `StorageAgreementEstablished`.
    fn insert_primary_agreement(
        bucket_id: BucketId,
        owner: &T::AccountId,
        provider: &T::AccountId,
        terms: AgreementTermsOf<T>,
        accepted: AcceptedQuote<T>,
    ) {
        let expires_at = Self::insert_agreement(bucket_id, owner, provider, &terms, accepted);
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
    }
}

/// The agreement an extrinsic opens from a quote.
#[derive(Clone, Copy)]
enum QuoteKind {
    Primary,
    Replica,
}

impl QuoteKind {
    /// Domain-separation prefix the provider signed the terms under.
    fn context(self) -> &'static [u8] {
        match self {
            Self::Primary => storage_primitives::PRIMARY_TERM_CONTEXT,
            Self::Replica => storage_primitives::REPLICA_TERM_CONTEXT,
        }
    }
}

/// Output of `accept_quote`, input to `insert_agreement`.
struct AcceptedQuote<T: Config> {
    anchor_block: BlockNumberFor<T>,
    new_committed: u64,
    payment: BalanceOf<T>,
    role: ProviderRole<BalanceOf<T>, BlockNumberFor<T>>,
}
