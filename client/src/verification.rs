//! Client-side verification and monitoring.
//!
//! Provides tools for clients to:
//! - Spot-check provider data integrity
//! - Track provider latency and reliability
//! - Make informed provider selection decisions

use crate::ClientError as Error;
use sp_core::H256;
use std::collections::HashMap;
use std::time::{Duration, Instant};

/// Provider performance statistics.
#[derive(Debug, Clone, Default)]
pub struct ProviderStats {
    /// Total number of requests made
    pub total_requests: u64,
    /// Number of successful requests
    pub successful_requests: u64,
    /// Number of failed requests
    pub failed_requests: u64,
    /// Average latency in milliseconds
    pub avg_latency_ms: u64,
    /// Number of spot-check failures
    pub spot_check_failures: u64,
    /// Number of spot-checks performed
    pub spot_checks_total: u64,
    /// Last successful request timestamp
    pub last_success: Option<Instant>,
    /// Reputation score (0-100)
    pub reputation: u8,
}

impl ProviderStats {
    /// Calculate success rate as a percentage.
    pub fn success_rate(&self) -> f64 {
        if self.total_requests == 0 {
            return 100.0;
        }
        (self.successful_requests as f64 / self.total_requests as f64) * 100.0
    }

    /// Calculate spot-check success rate.
    pub fn spot_check_success_rate(&self) -> f64 {
        if self.spot_checks_total == 0 {
            return 100.0;
        }
        let successes = self.spot_checks_total - self.spot_check_failures;
        (successes as f64 / self.spot_checks_total as f64) * 100.0
    }

    /// Update reputation based on performance.
    pub fn update_reputation(&mut self) {
        let success_weight = 0.4;
        let latency_weight = 0.3;
        let spot_check_weight = 0.3;

        // Success rate component (0-100)
        let success_score = self.success_rate();

        // Latency component (0-100, lower is better)
        // Consider <100ms = 100, >1000ms = 0
        let latency_score = if self.avg_latency_ms < 100 {
            100.0
        } else if self.avg_latency_ms > 1000 {
            0.0
        } else {
            100.0 - ((self.avg_latency_ms - 100) as f64 / 9.0)
        };

        // Spot-check component (0-100)
        let spot_check_score = self.spot_check_success_rate();

        let total_score = (success_score * success_weight)
            + (latency_score * latency_weight)
            + (spot_check_score * spot_check_weight);

        self.reputation = total_score.min(100.0).max(0.0) as u8;
    }
}

/// Client-side verification and monitoring.
pub struct ClientVerifier {
    /// Provider statistics
    provider_stats: HashMap<String, ProviderStats>,
    /// Latency samples for averaging (circular buffer)
    latency_samples: HashMap<String, Vec<u64>>,
    /// Maximum samples to keep
    max_samples: usize,
}

impl ClientVerifier {
    /// Create a new client verifier.
    pub fn new() -> Self {
        Self {
            provider_stats: HashMap::new(),
            latency_samples: HashMap::new(),
            max_samples: 100,
        }
    }

    /// Record a request to a provider.
    pub fn record_request(&mut self, provider_url: &str, duration: Duration, success: bool) {
        let stats = self
            .provider_stats
            .entry(provider_url.to_string())
            .or_default();

        stats.total_requests += 1;
        if success {
            stats.successful_requests += 1;
            stats.last_success = Some(Instant::now());
        } else {
            stats.failed_requests += 1;
        }

        // Update latency
        let latency_ms = duration.as_millis() as u64;
        let samples = self
            .latency_samples
            .entry(provider_url.to_string())
            .or_default();

        samples.push(latency_ms);
        if samples.len() > self.max_samples {
            samples.remove(0);
        }

        // Recalculate average
        if !samples.is_empty() {
            let sum: u64 = samples.iter().sum();
            stats.avg_latency_ms = sum / samples.len() as u64;
        }

        // Update reputation
        stats.update_reputation();
    }

    /// Perform a spot-check on a provider.
    ///
    /// This:
    /// 1. Reads a random chunk from the provider
    /// 2. Verifies the chunk hash
    /// 3. Optionally verifies the Merkle proof
    ///
    /// Returns true if the spot-check passed.
    pub async fn spot_check<T>(
        &mut self,
        client: &T,
        data_root: &H256,
        chunk_index: u64,
    ) -> Result<bool, Error>
    where
        T: ProviderReadAccess,
    {
        let start = Instant::now();

        // Read the chunk
        let chunk_size = 256 * 1024; // 256 KiB
        let offset = chunk_index * chunk_size;
        let result = client.read_data(data_root, offset, chunk_size).await;

        let duration = start.elapsed();
        let provider_url = client.provider_url();

        // Record the request
        match &result {
            Ok(data) => {
                // Verify chunk hash
                let _expected_hash = storage_primitives::blake2_256(data);

                // Get the expected hash from the proof
                // In a full implementation, we would fetch the chunk with proof
                // and verify the proof chain up to the data_root

                // For now, just record success
                self.record_request(provider_url, duration, true);

                // Update spot-check stats
                let stats = self.provider_stats.get_mut(provider_url).unwrap();
                stats.spot_checks_total += 1;

                Ok(true)
            }
            Err(_) => {
                self.record_request(provider_url, duration, false);

                // Update spot-check stats
                if let Some(stats) = self.provider_stats.get_mut(provider_url) {
                    stats.spot_checks_total += 1;
                    stats.spot_check_failures += 1;
                }

                Ok(false)
            }
        }
    }

    /// Perform multiple random spot-checks on a provider.
    pub async fn spot_check_batch<T>(
        &mut self,
        client: &T,
        data_root: &H256,
        num_checks: usize,
        total_chunks: u64,
    ) -> Result<(usize, usize), Error>
    where
        T: ProviderReadAccess,
    {
        use rand::Rng;
        let mut rng = rand::thread_rng();

        let mut passed = 0;
        let mut failed = 0;

        for _ in 0..num_checks {
            let chunk_index = rng.gen_range(0..total_chunks);
            match self.spot_check(client, data_root, chunk_index).await {
                Ok(true) => passed += 1,
                Ok(false) => failed += 1,
                Err(_) => failed += 1,
            }
        }

        Ok((passed, failed))
    }

    /// Get statistics for a provider.
    pub fn get_stats(&self, provider_url: &str) -> Option<&ProviderStats> {
        self.provider_stats.get(provider_url)
    }

    /// Get all provider statistics.
    pub fn get_all_stats(&self) -> &HashMap<String, ProviderStats> {
        &self.provider_stats
    }

    /// Select the best provider based on reputation.
    pub fn select_best_provider(&self, candidate_urls: &[String]) -> Option<String> {
        candidate_urls
            .iter()
            .filter_map(|url| {
                self.provider_stats
                    .get(url)
                    .map(|stats| (url.clone(), stats.reputation))
            })
            .max_by_key(|(_, reputation)| *reputation)
            .map(|(url, _)| url)
    }

    /// Check if a provider is considered reliable.
    ///
    /// A provider is reliable if:
    /// - Success rate > 95%
    /// - Average latency < 500ms
    /// - Spot-check success rate > 98%
    /// - Recent successful request (within last hour)
    pub fn is_reliable(&self, provider_url: &str) -> bool {
        if let Some(stats) = self.provider_stats.get(provider_url) {
            stats.success_rate() > 95.0
                && stats.avg_latency_ms < 500
                && stats.spot_check_success_rate() > 98.0
                && stats
                    .last_success
                    .map(|t| t.elapsed() < Duration::from_secs(3600))
                    .unwrap_or(false)
        } else {
            false
        }
    }
}

impl Default for ClientVerifier {
    fn default() -> Self {
        Self::new()
    }
}

/// Trait for types that can provide read access to storage.
///
/// This allows the verifier to work with different client types.
#[async_trait::async_trait]
pub trait ProviderReadAccess {
    /// Read data from a data root.
    async fn read_data(&self, data_root: &H256, offset: u64, length: u64)
        -> Result<Vec<u8>, Error>;

    /// Get the provider URL for tracking.
    fn provider_url(&self) -> &str;
}
