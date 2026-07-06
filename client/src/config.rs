// SPDX-License-Identifier: Apache-2.0

//! Client configuration and chunking strategy.

/// Configuration for connecting to the storage system.
#[derive(Debug, Clone)]
pub struct ClientConfig {
    /// WebSocket URL for the substrate node (e.g., "ws://localhost:2222")
    pub chain_ws_url: String,
    /// Default provider node URLs
    pub provider_urls: Vec<String>,
    /// Request timeout in seconds
    pub timeout_secs: u64,
    /// Enable automatic retries
    pub enable_retries: bool,
}

impl Default for ClientConfig {
    fn default() -> Self {
        Self {
            chain_ws_url: "ws://localhost:2222".to_string(),
            provider_urls: vec!["http://localhost:3333".to_string()],
            timeout_secs: 30,
            enable_retries: true,
        }
    }
}

/// Chunking strategy for data upload.
#[derive(Debug, Clone, Copy)]
pub enum ChunkingStrategy {
    /// Fixed-size chunks (default: 256 KiB)
    Fixed(usize),
    /// TODO: Content-defined chunking (not yet implemented)
    ///
    /// ContentDefined {
    ///     min_size: usize,
    ///     target_size: usize,
    ///     max_size: usize,
    /// },
    ContentDefined,
}

impl Default for ChunkingStrategy {
    fn default() -> Self {
        Self::Fixed(256 * 1024)
    }
}
