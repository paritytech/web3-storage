// SPDX-License-Identifier: Apache-2.0

//! `try_state` invariant checks. They assert config and cross-storage
//! invariants against live state, catching violations that `integrity_test`
//! (build-time only) and the extrinsic guards (bypassed by
//! `setStorage`/migrations) cannot. The `Hooks::try_state` hook invoking them
//! on every block / runtime-upgrade dry-run is still gated by `try-runtime`,
//! but the checks themselves are always compiled and callable.
//!
//! Read-only, never panics: a violated invariant returns `TryRuntimeError`.

use crate::*;
use alloc::collections::{BTreeMap, BTreeSet};
use frame_support::pallet_prelude::*;
use sp_runtime::TryRuntimeError;
use storage_primitives::{BucketId, ProviderRole};

impl<T: Config> Pallet<T> {
    pub fn do_try_state() -> Result<(), TryRuntimeError> {
        Self::check_timing_config()?;
        Self::check_committed_bytes()?;
        Self::check_buckets_and_membership()?;
        Self::check_challenge_sweep_cursor()?;
        Ok(())
    }

    /// P0: config timing invariants. Mirror of `integrity_test`, but run
    /// against live storage so a `setStorage`/migration mutation is caught.
    fn check_timing_config() -> Result<(), TryRuntimeError> {
        ensure!(
            T::RequestTimeout::get() < T::DeregisterAnnouncementPeriod::get(),
            "RequestTimeout must be < DeregisterAnnouncementPeriod (re-register replay window)"
        );
        ensure!(
            T::DeregisterAnnouncementPeriod::get() > T::ChallengeTimeout::get(),
            "DeregisterAnnouncementPeriod must be > ChallengeTimeout (challenge maturity)"
        );
        Ok(())
    }

    /// P1.5: no unresolved challenge sits at or below the swept cursor. The
    /// `on_initialize` sweep drains every deadline key up to its cursor (parking
    /// one below a key it only partly drained), and `create_challenge` always
    /// sets `deadline = now + ChallengeTimeout`, above the cursor — so anything
    /// at or below it must already have been drained. A violation means a
    /// challenge was stranded unslashed (e.g. an upgrade that left
    /// parachain-denominated keys below the anchor).
    fn check_challenge_sweep_cursor() -> Result<(), TryRuntimeError> {
        if let Some(cursor) = LastSweptChallengeBlock::<T>::get() {
            for (deadline, _index, _challenge) in Challenges::<T>::iter() {
                ensure!(
                    deadline > cursor,
                    "unresolved challenge sits at or below the swept cursor"
                );
            }
        }
        Ok(())
    }

    /// P1.1: each provider's `committed_bytes` equals the sum of `max_bytes`
    /// over all its storage agreements (the accounting that gates capacity and
    /// required stake).
    fn check_committed_bytes() -> Result<(), TryRuntimeError> {
        let mut summed: BTreeMap<T::AccountId, u64> = BTreeMap::new();
        for (_bucket_id, provider, agreement) in StorageAgreements::<T>::iter() {
            let entry = summed.entry(provider).or_default();
            *entry = entry
                .checked_add(agreement.max_bytes)
                .ok_or("committed_bytes sum overflows u64")?;
        }
        // Every provider with agreements is registered, with matching committed_bytes.
        for (provider, committed) in summed.iter() {
            let info = Providers::<T>::get(provider)
                .ok_or("storage agreement exists for an unregistered provider")?;
            ensure!(
                info.committed_bytes == *committed,
                "provider committed_bytes != sum of agreement max_bytes"
            );
        }
        // A registered provider with no agreements has zero committed_bytes.
        for (provider, info) in Providers::<T>::iter() {
            if !summed.contains_key(&provider) {
                ensure!(
                    info.committed_bytes == 0,
                    "registered provider has committed_bytes but no agreements"
                );
            }
        }
        Ok(())
    }

    /// P1.3: per bucket, `primary_providers` has no duplicates and equals
    /// exactly the set of `Primary`-role agreement providers.
    /// P1.4: `MemberBuckets` is the correct and complete reverse index of
    /// bucket membership, with no duplicates.
    fn check_buckets_and_membership() -> Result<(), TryRuntimeError> {
        // Decode the reverse index once, up front, rejecting duplicate bucket
        // ids as we build it. The forward check below looks up each bucket
        // member's entry: calling `MemberBuckets::get` per member would
        // re-decode a member's (potentially large) bucket list once per bucket
        // it belongs to — an N+1 with quadratic decode cost. The reverse check
        // already needs a full pass over `MemberBuckets`, so materialising the
        // map here is effectively free, and both directions then check against
        // it in memory.
        let mut member_index: BTreeMap<T::AccountId, BTreeSet<BucketId>> = BTreeMap::new();
        for (account, buckets) in MemberBuckets::<T>::iter() {
            let mut indexed = BTreeSet::new();
            for bucket_id in buckets.iter() {
                ensure!(
                    indexed.insert(*bucket_id),
                    "duplicate bucket id in MemberBuckets entry"
                );
            }
            member_index.insert(account, indexed);
        }

        for (bucket_id, bucket) in Buckets::<T>::iter() {
            // P1.3
            let declared: BTreeSet<T::AccountId> =
                bucket.primary_providers.iter().cloned().collect();
            ensure!(
                declared.len() == bucket.primary_providers.len(),
                "duplicate entry in bucket primary_providers"
            );
            let primaries: BTreeSet<T::AccountId> = StorageAgreements::<T>::iter_prefix(bucket_id)
                .filter(|(_p, a)| matches!(a.role, ProviderRole::Primary))
                .map(|(p, _a)| p)
                .collect();
            ensure!(
                declared == primaries,
                "primary_providers does not match Primary-role agreements for bucket"
            );

            // P1.4 (forward): every member is indexed under MemberBuckets.
            for member in bucket.members.iter() {
                ensure!(
                    member_index
                        .get(&member.account)
                        .is_some_and(|buckets| buckets.contains(&bucket_id)),
                    "bucket member missing from MemberBuckets reverse index"
                );
            }
        }

        // P1.4 (reverse): every reverse-index entry is a real membership.
        for (account, buckets) in member_index.iter() {
            for bucket_id in buckets {
                let bucket = Buckets::<T>::get(bucket_id)
                    .ok_or("MemberBuckets references a non-existent bucket")?;
                ensure!(
                    bucket.members.iter().any(|m| m.account == *account),
                    "MemberBuckets entry is not an actual member of the bucket"
                );
            }
        }
        Ok(())
    }
}
