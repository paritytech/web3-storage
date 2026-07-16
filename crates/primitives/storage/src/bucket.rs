// SPDX-License-Identifier: Apache-2.0

//! Bucket container and membership types.

use codec::{Decode, Encode, MaxEncodedLen};
use scale_info::TypeInfo;
use sp_core::H256;
use sp_runtime::{traits::Get, BoundedVec};

use crate::BucketSnapshot;

/// Bucket member with role.
#[derive(Clone, PartialEq, Eq, Encode, Decode, TypeInfo, MaxEncodedLen, Debug)]
pub struct Member<AccountId> {
    pub account: AccountId,
    pub role: crate::Role,
}

/// Bucket container for data with membership and storage agreements.
#[derive(Encode, Decode, TypeInfo)]
#[scale_info(skip_type_params(MaxMembers, MaxPrimaryProviders))]
pub struct Bucket<AccountId, BlockNumber, MaxMembers: Get<u32>, MaxPrimaryProviders: Get<u32>> {
    /// Members who can interact with this bucket.
    pub members: BoundedVec<Member<AccountId>, MaxMembers>,
    /// If Some, bucket is append-only from this start_seq.
    pub frozen_start_seq: Option<u64>,
    /// Minimum primary provider signatures required for checkpoint.
    pub min_providers: u32,
    /// Primary provider account IDs (limited to MaxPrimaryProviders).
    pub primary_providers: BoundedVec<AccountId, MaxPrimaryProviders>,
    /// Current canonical state.
    pub snapshot: Option<BucketSnapshot<BlockNumber>>,
    /// Historical MMR roots for replica sync validation.
    pub historical_roots: [(u32, H256); 6],
    /// Total snapshots created for this bucket.
    pub total_snapshots: u32,
}

// Manual impls instead of derives: std derives would put `Clone`/`PartialEq`/
// `Debug` bounds on the `Get<u32>` markers, which bound types (e.g.
// `parameter_types!` structs) do not implement.
impl<AccountId, BlockNumber, MaxMembers, MaxPrimaryProviders> Clone
    for Bucket<AccountId, BlockNumber, MaxMembers, MaxPrimaryProviders>
where
    AccountId: Clone,
    BlockNumber: Clone,
    MaxMembers: Get<u32>,
    MaxPrimaryProviders: Get<u32>,
{
    fn clone(&self) -> Self {
        Self {
            members: self.members.clone(),
            frozen_start_seq: self.frozen_start_seq,
            min_providers: self.min_providers,
            primary_providers: self.primary_providers.clone(),
            snapshot: self.snapshot.clone(),
            historical_roots: self.historical_roots,
            total_snapshots: self.total_snapshots,
        }
    }
}

impl<AccountId, BlockNumber, MaxMembers, MaxPrimaryProviders> PartialEq
    for Bucket<AccountId, BlockNumber, MaxMembers, MaxPrimaryProviders>
where
    AccountId: PartialEq,
    BlockNumber: PartialEq,
    MaxMembers: Get<u32>,
    MaxPrimaryProviders: Get<u32>,
{
    fn eq(&self, other: &Self) -> bool {
        self.members == other.members
            && self.frozen_start_seq == other.frozen_start_seq
            && self.min_providers == other.min_providers
            && self.primary_providers == other.primary_providers
            && self.snapshot == other.snapshot
            && self.historical_roots == other.historical_roots
            && self.total_snapshots == other.total_snapshots
    }
}

impl<AccountId, BlockNumber, MaxMembers, MaxPrimaryProviders> Eq
    for Bucket<AccountId, BlockNumber, MaxMembers, MaxPrimaryProviders>
where
    AccountId: Eq,
    BlockNumber: Eq,
    MaxMembers: Get<u32>,
    MaxPrimaryProviders: Get<u32>,
{
}

impl<AccountId, BlockNumber, MaxMembers, MaxPrimaryProviders> core::fmt::Debug
    for Bucket<AccountId, BlockNumber, MaxMembers, MaxPrimaryProviders>
where
    AccountId: core::fmt::Debug,
    BlockNumber: core::fmt::Debug,
    MaxMembers: Get<u32>,
    MaxPrimaryProviders: Get<u32>,
{
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_struct("Bucket")
            .field("members", &self.members)
            .field("frozen_start_seq", &self.frozen_start_seq)
            .field("min_providers", &self.min_providers)
            .field("primary_providers", &self.primary_providers)
            .field("snapshot", &self.snapshot)
            .field("historical_roots", &self.historical_roots)
            .field("total_snapshots", &self.total_snapshots)
            .finish()
    }
}
