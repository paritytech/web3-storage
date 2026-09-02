// SPDX-License-Identifier: Apache-2.0

use crate::Member;
use crate::*;
use alloc::vec::Vec;
use frame_support::{
    pallet_prelude::*,
    traits::{Currency, ExistenceRequirement, ReservableCurrency},
};
use sp_core::H256;
use sp_runtime::traits::{SaturatedConversion, Saturating, Zero};
use storage_primitives::{BucketId, Role};

impl<T: Config> Pallet<T> {
    /// Internal function to cleanup a bucket and all its agreements.
    /// This is called by Layer 1 (drive-registry) when deleting a drive.
    ///
    /// Returns the total amount refunded to the owner.
    pub fn cleanup_bucket_internal(
        bucket_id: BucketId,
        owner: &T::AccountId,
    ) -> Result<BalanceOf<T>, DispatchError> {
        // Verify bucket exists
        let bucket = Buckets::<T>::get(bucket_id).ok_or(Error::<T>::BucketNotFound)?;

        // Verify caller is an admin of the bucket
        Self::ensure_admin(owner, &bucket)?;

        // A frozen bucket is append-only forever by design; tearing it down
        // would delete every leaf at once, so the whole call is refused.
        ensure!(bucket.frozen_start_seq.is_none(), Error::<T>::BucketFrozen);

        let mut total_refunded: BalanceOf<T> = Zero::zero();

        // End all agreements for this bucket (pay providers fairly)
        let agreements: Vec<_> = StorageAgreements::<T>::iter_prefix(bucket_id).collect();

        // Refuse to delete the bucket while any of its agreements has a
        // pending challenge — otherwise tearing down here would let the
        // provider escape a live slashable challenge. Checked before any
        // state mutation/payout so the whole call is a no-op on failure.
        for (provider, _) in &agreements {
            ensure!(
                PendingChallengesByBucket::<T>::get(bucket_id, provider) == 0,
                Error::<T>::AgreementHasPendingChallenge
            );
        }

        for (provider, agreement) in agreements {
            // Calculate prorated refund based on remaining time
            let anchor_block = Self::current_anchor_block();
            let remaining_blocks = agreement.expires_at.saturating_sub(anchor_block);

            // If there's remaining time, calculate prorated refund
            let refund_to_owner = if remaining_blocks > Zero::zero() {
                let total_duration = agreement.expires_at.saturating_sub(agreement.started_at);
                if total_duration > Zero::zero() {
                    let remaining_u128: u128 = remaining_blocks.saturated_into();
                    let total_u128: u128 = total_duration.saturated_into();
                    let payment_u128: u128 = agreement.payment_locked.saturated_into();

                    // refund = payment * (remaining / total)
                    let refund_u128 = payment_u128
                        .saturating_mul(remaining_u128)
                        .saturating_div(total_u128);
                    refund_u128.saturated_into()
                } else {
                    Zero::zero()
                }
            } else {
                Zero::zero()
            };

            // Payment to provider = total locked - refund to owner
            let payment_to_provider = agreement.payment_locked.saturating_sub(refund_to_owner);

            // Unreserve from owner
            T::Currency::unreserve(&agreement.owner, agreement.payment_locked);

            // Pay provider their earned portion
            if !payment_to_provider.is_zero() {
                T::Currency::transfer(
                    &agreement.owner,
                    &provider,
                    payment_to_provider,
                    ExistenceRequirement::KeepAlive,
                )?;
            }

            // Track total refunded (owner keeps the unspent portion)
            total_refunded = total_refunded.saturating_add(refund_to_owner);

            // Update provider stats
            Providers::<T>::mutate(&provider, |maybe_provider| {
                if let Some(provider_info) = maybe_provider {
                    provider_info.committed_bytes = provider_info
                        .committed_bytes
                        .saturating_sub(agreement.max_bytes);
                    provider_info.stats.agreements_not_extended = provider_info
                        .stats
                        .agreements_not_extended
                        .saturating_add(1);
                }
            });

            // Remove agreement
            StorageAgreements::<T>::remove(bucket_id, &provider);

            Self::deposit_event(Event::AgreementEnded {
                bucket_id,
                provider: provider.clone(),
                payment_to_provider,
                burned: Zero::zero(),
            });
        }

        // Clean up reverse index for all members
        for member in &bucket.members {
            MemberBuckets::<T>::mutate(&member.account, |buckets| {
                buckets.retain(|id| *id != bucket_id);
            });
        }

        // Remove the bucket itself
        Buckets::<T>::remove(bucket_id);

        Self::deposit_event(Event::BucketDeleted { bucket_id });

        Ok(total_refunded)
    }

    /// Create a bucket internally (for use by other pallets like Layer 1 File System).
    ///
    /// This bypasses the normal extrinsic flow and creates a bucket directly,
    /// with the specified account as admin.
    ///
    /// Parameters:
    /// - `admin`: Account that will be the bucket admin.
    /// - `min_providers`: Minimum number of primary providers required to
    ///   sign each checkpoint.
    /// - `initial_primary`: Optional provider to seed as the bucket's
    ///   first `primary_providers` entry. Used by
    ///   `establish_storage_agreement_internal` to atomically create the
    ///   bucket together with its primary agreement; pass `None` for
    ///   buckets that will register primaries later.
    ///
    /// Returns: bucket_id
    pub(crate) fn create_bucket_internal(
        admin: &T::AccountId,
        min_providers: u32,
        initial_primary: Option<&T::AccountId>,
    ) -> Result<BucketId, DispatchError> {
        let bucket_id = NextBucketId::<T>::get();
        NextBucketId::<T>::put(bucket_id.saturating_add(1));

        let admin_member = Member {
            account: admin.clone(),
            role: Role::Admin,
        };

        let mut members = BoundedVec::new();
        members
            .try_push(admin_member)
            .map_err(|_| Error::<T>::MaxMembersReached)?;

        let mut primary_providers = BoundedVec::new();
        if let Some(p) = initial_primary {
            primary_providers
                .try_push(p.clone())
                .map_err(|_| Error::<T>::MaxPrimaryProvidersReached)?;
        }

        let bucket = Bucket {
            members,
            frozen_start_seq: None,
            min_providers,
            primary_providers,
            snapshot: None,
            historical_roots: [(0, H256::zero()); 6],
            total_snapshots: 0,
        };

        Buckets::<T>::insert(bucket_id, bucket);

        // Update reverse index for creator
        MemberBuckets::<T>::try_mutate(admin, |buckets| {
            buckets
                .try_push(bucket_id)
                .map_err(|_| Error::<T>::TooManyBucketsForMember)
        })?;

        Self::deposit_event(Event::BucketCreated {
            bucket_id,
            admin: admin.clone(),
        });

        Ok(bucket_id)
    }
}
