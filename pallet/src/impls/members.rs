// SPDX-License-Identifier: Apache-2.0

use crate::Member;
use crate::*;
use frame_support::pallet_prelude::*;
use storage_primitives::{BucketId, Role};

impl<T: Config> Pallet<T> {
    pub(crate) fn ensure_admin(who: &T::AccountId, bucket: &Bucket<T>) -> DispatchResult {
        ensure!(
            bucket
                .members
                .iter()
                .any(|m| &m.account == who && m.role == Role::Admin),
            Error::<T>::NotBucketAdmin
        );
        Ok(())
    }

    /// Single `bucket.member` iteration to find `member` matching. Returns:
    /// - the target member's index (if present),
    /// - whether that member currently holds `Role::Admin`,
    /// - the total number of admins in the bucket.
    pub(crate) fn locate_member(
        bucket: &Bucket<T>,
        member: &T::AccountId,
    ) -> (Option<usize>, bool, u32) {
        let mut target_idx = None;
        let mut target_is_admin = false;
        let mut admin_count: u32 = 0;
        for (i, m) in bucket.members.iter().enumerate() {
            if m.role == Role::Admin {
                admin_count = admin_count.saturating_add(1);
            }
            if &m.account == member {
                target_idx = Some(i);
                target_is_admin = m.role == Role::Admin;
            }
        }
        (target_idx, target_is_admin, admin_count)
    }

    pub(crate) fn ensure_writer_or_admin(who: &T::AccountId, bucket: &Bucket<T>) -> DispatchResult {
        ensure!(
            bucket
                .members
                .iter()
                .any(|m| &m.account == who && (m.role == Role::Admin || m.role == Role::Writer)),
            Error::<T>::NotBucketWriter
        );
        Ok(())
    }

    /// Add or update a member's role on a bucket (callable from other pallets).
    ///
    /// The `caller` must be an Admin of the bucket.
    pub fn set_member_internal(
        caller: &T::AccountId,
        bucket_id: BucketId,
        member: T::AccountId,
        role: Role,
    ) -> DispatchResult {
        Buckets::<T>::try_mutate(bucket_id, |maybe_bucket| -> DispatchResult {
            let bucket = maybe_bucket.as_mut().ok_or(Error::<T>::BucketNotFound)?;

            Self::ensure_admin(caller, bucket)?;

            let (target_idx, target_is_admin, admin_count) = Self::locate_member(bucket, &member);
            if let Some(idx) = target_idx {
                if target_is_admin && role != Role::Admin {
                    // Admins can only demote themselves, never another admin.
                    ensure!(member == *caller, Error::<T>::CannotDemoteAdmin);
                    // And even self-demotion must leave at least one admin.
                    ensure!(admin_count > 1, Error::<T>::LastAdminCannotBeRemoved);
                }
                bucket.members[idx].role = role;
            } else {
                let new_member = Member {
                    account: member.clone(),
                    role,
                };
                bucket
                    .members
                    .try_push(new_member)
                    .map_err(|_| Error::<T>::MaxMembersReached)?;

                MemberBuckets::<T>::try_mutate(&member, |buckets| {
                    if !buckets.contains(&bucket_id) {
                        buckets
                            .try_push(bucket_id)
                            .map_err(|_| Error::<T>::TooManyBucketsForMember)
                    } else {
                        Ok(())
                    }
                })?;
            }

            Self::deposit_event(Event::MemberSet {
                bucket_id,
                member,
                role,
            });

            Ok(())
        })
    }

    /// Remove a member from a bucket (callable from other pallets).
    ///
    /// The `caller` must be an Admin of the bucket.
    pub fn remove_member_internal(
        caller: &T::AccountId,
        bucket_id: BucketId,
        member: T::AccountId,
    ) -> DispatchResult {
        Buckets::<T>::try_mutate(bucket_id, |maybe_bucket| -> DispatchResult {
            let bucket = maybe_bucket.as_mut().ok_or(Error::<T>::BucketNotFound)?;

            Self::ensure_admin(caller, bucket)?;

            let (target_idx, target_is_admin, admin_count) = Self::locate_member(bucket, &member);
            let member_idx = target_idx.ok_or(Error::<T>::MemberNotFound)?;

            if target_is_admin {
                // Admins can only remove themselves, never another admin.
                ensure!(member == *caller, Error::<T>::CannotDemoteAdmin);
                // And even self-removal must leave at least one admin.
                ensure!(admin_count > 1, Error::<T>::LastAdminCannotBeRemoved);
            }

            bucket.members.remove(member_idx);

            MemberBuckets::<T>::mutate(&member, |buckets| {
                buckets.retain(|id| *id != bucket_id);
            });

            Self::deposit_event(Event::MemberRemoved { bucket_id, member });

            Ok(())
        })
    }
}
