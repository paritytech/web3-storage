// SPDX-License-Identifier: Apache-2.0

//! Provider health tracking and checkpoint metrics.

use crate::checkpoint::conflict::{ConflictType, ProviderConflict};
use crate::checkpoint::result::CheckpointResult;
use sp_runtime::AccountId32;
use std::collections::VecDeque;
use std::time::Instant;

/// Information about a storage provider.
#[derive(Clone, Debug)]
pub struct ProviderInfo {
    /// Provider's account ID.
    pub account_id: AccountId32,
    /// HTTP endpoint for the provider.
    pub endpoint: String,
    /// Provider's public key for signature verification.
    pub public_key: Vec<u8>,
    /// Last time we successfully contacted this provider.
    pub last_seen: Option<Instant>,
    /// Current health status.
    pub status: ProviderStatus,
}

/// Health status of a provider.
#[derive(Clone, Debug, PartialEq)]
pub enum ProviderStatus {
    /// Provider is responding normally.
    Healthy,
    /// Provider is responding but with issues.
    Degraded { last_error: String },
    /// Provider is not responding.
    Unreachable { since: Instant },
    /// Status unknown (not yet checked).
    Unknown,
}

/// Provider health history for tracking reliability over time.
#[derive(Clone, Debug)]
pub struct ProviderHealthHistory {
    /// Provider account ID.
    pub account_id: AccountId32,
    /// Total number of requests made.
    pub total_requests: u64,
    /// Number of successful requests.
    pub successful_requests: u64,
    /// Number of failed requests.
    pub failed_requests: u64,
    /// Average response time in milliseconds.
    pub avg_response_time_ms: u64,
    /// Last N status changes for trend analysis.
    pub recent_statuses: VecDeque<(Instant, ProviderStatus)>,
    /// Last successful contact time.
    pub last_success: Option<Instant>,
    /// Last failure time.
    pub last_failure: Option<Instant>,
    /// Current consecutive failures.
    pub consecutive_failures: u32,
}

impl ProviderHealthHistory {
    /// Create a new health history.
    pub fn new(account_id: AccountId32) -> Self {
        Self {
            account_id,
            total_requests: 0,
            successful_requests: 0,
            failed_requests: 0,
            avg_response_time_ms: 0,
            recent_statuses: VecDeque::new(),
            last_success: None,
            last_failure: None,
            consecutive_failures: 0,
        }
    }

    /// Record a successful request.
    pub fn record_success(&mut self, response_time_ms: u64) {
        self.total_requests += 1;
        self.successful_requests += 1;
        self.consecutive_failures = 0;
        self.last_success = Some(Instant::now());

        // Update average response time
        let total = self.successful_requests;
        self.avg_response_time_ms =
            (self.avg_response_time_ms * (total - 1) + response_time_ms) / total;

        // Track status change
        self.add_status(ProviderStatus::Healthy);
    }

    /// Record a failed request.
    pub fn record_failure(&mut self, error: String) {
        self.total_requests += 1;
        self.failed_requests += 1;
        self.consecutive_failures += 1;
        self.last_failure = Some(Instant::now());

        // Track status change
        self.add_status(ProviderStatus::Degraded { last_error: error });
    }

    /// Add a status to the history (keep last 10).
    fn add_status(&mut self, status: ProviderStatus) {
        self.recent_statuses.push_back((Instant::now(), status));
        if self.recent_statuses.len() > 10 {
            self.recent_statuses.pop_front();
        }
    }

    /// Calculate success rate (0.0 to 1.0).
    pub fn success_rate(&self) -> f64 {
        if self.total_requests == 0 {
            return 1.0;
        }
        self.successful_requests as f64 / self.total_requests as f64
    }

    /// Check if provider is considered healthy (success rate > 80%, no recent failures).
    pub fn is_healthy(&self) -> bool {
        self.success_rate() > 0.8 && self.consecutive_failures < 3
    }

    /// Get current status based on history.
    pub fn current_status(&self) -> ProviderStatus {
        if self.consecutive_failures >= 5 {
            ProviderStatus::Unreachable {
                since: self.last_failure.unwrap_or_else(Instant::now),
            }
        } else if self.consecutive_failures > 0 || self.success_rate() < 0.8 {
            ProviderStatus::Degraded {
                last_error: format!(
                    "{} consecutive failures, {:.0}% success rate",
                    self.consecutive_failures,
                    self.success_rate() * 100.0
                ),
            }
        } else if self.total_requests == 0 {
            ProviderStatus::Unknown
        } else {
            ProviderStatus::Healthy
        }
    }
}

/// Metrics for checkpoint operations.
#[derive(Clone, Debug, Default)]
pub struct CheckpointMetrics {
    /// Total checkpoints attempted.
    pub total_attempts: u64,
    /// Successful checkpoints submitted.
    pub successful_submissions: u64,
    /// Checkpoints failed due to insufficient consensus.
    pub insufficient_consensus_count: u64,
    /// Checkpoints failed due to unreachable providers.
    pub unreachable_failures: u64,
    /// Checkpoints failed due to transaction errors.
    pub transaction_failures: u64,
    /// Total conflicts detected.
    pub conflicts_detected: u64,
    /// Conflicts where auto-challenge was recommended.
    pub auto_challenge_recommended: u64,
    /// Total providers queried.
    pub providers_queried: u64,
    /// Successful provider queries.
    pub providers_responded: u64,
    /// Average checkpoint submission time (ms).
    pub avg_submission_time_ms: u64,
    /// Last checkpoint timestamp.
    pub last_checkpoint_time: Option<Instant>,
    /// Rolling average of consensus rate (0.0 - 1.0).
    pub avg_consensus_rate: f64,
}

impl CheckpointMetrics {
    /// Record a checkpoint attempt result.
    pub fn record_attempt(&mut self, result: &CheckpointResult, duration_ms: u64) {
        self.total_attempts += 1;
        self.last_checkpoint_time = Some(Instant::now());

        match result {
            CheckpointResult::Submitted { signers, .. } => {
                self.successful_submissions += 1;
                // Update rolling average submission time (after incrementing count)
                let n = self.successful_submissions;
                self.avg_submission_time_ms =
                    (self.avg_submission_time_ms * (n - 1) + duration_ms) / n;
                self.providers_responded += signers.len() as u64;
            }
            CheckpointResult::InsufficientConsensus { agreeing, .. } => {
                self.insufficient_consensus_count += 1;
                self.providers_responded += *agreeing as u64;
            }
            CheckpointResult::ProvidersUnreachable { providers } => {
                self.unreachable_failures += 1;
                // These providers didn't respond
                self.providers_queried += providers.len() as u64;
            }
            CheckpointResult::NoProviders => {
                // No providers configured
            }
            CheckpointResult::TransactionFailed { .. } => {
                self.transaction_failures += 1;
            }
        }
    }

    /// Record a conflict detection.
    pub fn record_conflict(&mut self, conflict: &ProviderConflict) {
        self.conflicts_detected += 1;

        // Check if auto-challenge is recommended
        for c in &conflict.conflicts {
            if matches!(c.conflict_type, ConflictType::DataDivergence) {
                self.auto_challenge_recommended += 1;
                break;
            }
        }
    }

    /// Calculate success rate.
    pub fn success_rate(&self) -> f64 {
        if self.total_attempts == 0 {
            1.0
        } else {
            self.successful_submissions as f64 / self.total_attempts as f64
        }
    }

    /// Calculate provider response rate.
    pub fn provider_response_rate(&self) -> f64 {
        if self.providers_queried == 0 {
            1.0
        } else {
            self.providers_responded as f64 / self.providers_queried as f64
        }
    }
}
