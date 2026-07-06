// SPDX-License-Identifier: Apache-2.0

//! Checkpoint manager and background-loop configuration.

use std::time::Duration;

/// Configuration for the Checkpoint Manager.
#[derive(Clone, Debug)]
pub struct CheckpointConfig {
    /// Maximum time to wait for provider responses.
    pub provider_timeout: Duration,
    /// Number of retries for failed provider queries.
    pub max_retries: u32,
    /// Base delay between retries (exponential backoff).
    pub retry_delay: Duration,
    /// Minimum percentage of providers that must agree (0-100).
    pub consensus_threshold_percent: u8,
    /// How long to cache provider info before refreshing.
    pub provider_cache_ttl: Duration,
}

impl Default for CheckpointConfig {
    fn default() -> Self {
        Self {
            provider_timeout: Duration::from_secs(30),
            max_retries: 3,
            retry_delay: Duration::from_secs(2),
            consensus_threshold_percent: 51,
            provider_cache_ttl: Duration::from_secs(300), // 5 minutes
        }
    }
}

/// Configuration for background batched checkpoints.
#[derive(Clone, Debug)]
pub struct BatchedCheckpointConfig {
    /// Interval between checkpoint submissions (in blocks or duration).
    pub interval: BatchedInterval,
    /// Whether to submit checkpoint even if no changes detected.
    pub submit_on_empty: bool,
    /// Maximum number of consecutive failures before pausing.
    pub max_consecutive_failures: u32,
    /// Delay after failure before retrying.
    pub failure_retry_delay: Duration,
}

impl Default for BatchedCheckpointConfig {
    fn default() -> Self {
        Self {
            interval: BatchedInterval::Blocks(100),
            submit_on_empty: false,
            max_consecutive_failures: 5,
            failure_retry_delay: Duration::from_secs(30),
        }
    }
}

/// Interval specification for batched checkpoints.
#[derive(Clone, Debug)]
pub enum BatchedInterval {
    /// Number of blocks between checkpoints.
    Blocks(u32),
    /// Time duration between checkpoints.
    Duration(Duration),
}
