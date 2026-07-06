// SPDX-License-Identifier: Apache-2.0

//! Auto-challenge configuration, recommendations, and outcomes.

use crate::roles::challenger::ChallengeId;
use sp_core::H256;
use sp_runtime::AccountId32;
use std::time::{Duration, Instant};
use storage_primitives::BucketId;

/// Configuration for automatic challenge submission.
#[derive(Clone, Debug)]
pub struct AutoChallengeConfig {
    /// Whether auto-challenge is enabled.
    pub enabled: bool,
    /// Minimum conflict occurrences before challenging.
    pub min_conflict_count: u32,
    /// Time to wait for sync before considering challenge.
    pub sync_wait_duration: Duration,
    /// Whether to challenge on data divergence (same leaf count, different root).
    pub challenge_on_divergence: bool,
}

impl Default for AutoChallengeConfig {
    fn default() -> Self {
        Self {
            enabled: false, // Disabled by default for safety
            min_conflict_count: 3,
            sync_wait_duration: Duration::from_secs(60),
            challenge_on_divergence: true,
        }
    }
}

/// Challenge recommendation from conflict analysis.
#[derive(Clone, Debug)]
pub struct ChallengeRecommendation {
    /// Provider to potentially challenge.
    pub provider: AccountId32,
    /// Reason for the recommendation.
    pub reason: ChallengeReason,
    /// Confidence level (0.0 - 1.0).
    pub confidence: f64,
    /// Number of times this conflict was observed.
    pub occurrence_count: u32,
    /// Evidence for the challenge.
    pub evidence: ChallengeEvidence,
}

/// Reason for challenge recommendation.
#[derive(Clone, Debug)]
pub enum ChallengeReason {
    /// Same leaf count but different MMR root.
    DataDivergence {
        majority_root: H256,
        provider_root: H256,
        leaf_count: u64,
    },
    /// Provider persistently behind after sync wait.
    PersistentlySyncing { behind_by: u64, duration: Duration },
    /// Provider claiming to be ahead of majority.
    ClaimingAhead {
        claimed_leaf_count: u64,
        majority_leaf_count: u64,
    },
}

/// Evidence to support a challenge.
#[derive(Clone, Debug)]
pub struct ChallengeEvidence {
    /// Bucket ID where conflict occurred.
    pub bucket_id: BucketId,
    /// Majority commitment from agreeing providers.
    pub majority_commitment: (H256, u64, u64), // (mmr_root, start_seq, leaf_count)
    /// Signatures from majority providers.
    pub majority_signatures: Vec<(AccountId32, Vec<u8>)>,
    /// Provider's commitment that conflicts.
    pub provider_commitment: Option<(H256, u64, u64)>,
    /// Timestamps of conflict observations.
    pub observation_times: Vec<Instant>,
}

/// Result of executing auto-challenges.
#[derive(Clone, Debug)]
pub struct AutoChallengeResult {
    /// Number of providers analyzed.
    pub providers_analyzed: usize,
    /// Challenges successfully submitted.
    pub challenges_submitted: Vec<SubmittedChallenge>,
    /// Challenges that failed to submit.
    pub challenges_failed: Vec<FailedChallenge>,
    /// Providers skipped (below confidence threshold).
    pub providers_skipped: usize,
}

/// A successfully submitted challenge.
#[derive(Clone, Debug)]
pub struct SubmittedChallenge {
    /// The provider challenged.
    pub provider: AccountId32,
    /// Challenge ID from the chain.
    pub challenge_id: ChallengeId,
    /// Reason for the challenge.
    pub reason: ChallengeReason,
    /// Confidence level of the recommendation.
    pub confidence: f64,
}

/// A challenge that failed to submit.
#[derive(Clone, Debug)]
pub struct FailedChallenge {
    /// The provider we tried to challenge.
    pub provider: AccountId32,
    /// Reason for the challenge attempt.
    pub reason: ChallengeReason,
    /// Error message.
    pub error: String,
}
