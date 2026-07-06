// SPDX-License-Identifier: Apache-2.0

//! Provider commitment collection and conflict detection.

use sp_core::H256;
use sp_runtime::AccountId32;
use std::time::Instant;
use storage_primitives::BucketId;

/// Result of collecting commitments from providers.
#[derive(Clone, Debug)]
pub struct CommitmentCollection {
    /// Bucket ID.
    pub bucket_id: BucketId,
    /// Majority MMR root (most providers agree on this).
    pub mmr_root: H256,
    /// Start sequence number.
    pub start_seq: u64,
    /// Number of leaves in the MMR.
    pub leaf_count: u64,
    /// Signatures from agreeing providers: (account_id, signature_bytes).
    pub signatures: Vec<(AccountId32, Vec<u8>)>,
    /// Providers that agreed on the majority root.
    pub agreeing_providers: Vec<AccountId32>,
    /// Providers with different roots: (account_id, their_root).
    pub disagreeing_providers: Vec<(AccountId32, H256)>,
    /// Providers that couldn't be reached.
    pub unreachable_providers: Vec<AccountId32>,
}

/// Detected conflict between providers.
#[derive(Clone, Debug)]
pub struct ProviderConflict {
    /// Bucket where conflict was detected.
    pub bucket_id: BucketId,
    /// The majority MMR root (what most providers agree on).
    pub majority_root: H256,
    /// Number of providers agreeing on majority.
    pub majority_count: usize,
    /// Conflicting providers with their different roots.
    pub conflicts: Vec<ConflictingProvider>,
    /// When the conflict was detected.
    pub detected_at: Instant,
    /// Possible resolution strategy.
    pub resolution: ConflictResolution,
}

/// A provider that disagrees with the majority.
#[derive(Clone, Debug)]
pub struct ConflictingProvider {
    /// Provider account ID.
    pub account_id: AccountId32,
    /// Their MMR root (different from majority).
    pub mmr_root: H256,
    /// Their leaf count.
    pub leaf_count: u64,
    /// Whether they're behind (likely sync delay) or divergent (data corruption).
    pub conflict_type: ConflictType,
}

/// Type of conflict detected.
#[derive(Clone, Debug, PartialEq)]
pub enum ConflictType {
    /// Provider is behind (lower leaf count) - likely sync delay.
    SyncDelay { behind_by: u64 },
    /// Provider has same leaf count but different root - data divergence.
    DataDivergence,
    /// Provider is ahead of majority - unusual.
    Ahead { ahead_by: u64 },
}

/// Suggested resolution for a conflict.
#[derive(Clone, Debug, PartialEq)]
pub enum ConflictResolution {
    /// Wait for sync and retry.
    WaitForSync { estimated_blocks: u32 },
    /// Proceed with majority (above threshold).
    ProceedWithMajority,
    /// Manual intervention required.
    ManualIntervention { reason: String },
    /// Consider challenging the provider.
    ConsiderChallenge { provider: AccountId32 },
}
