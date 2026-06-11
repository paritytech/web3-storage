//! Coordinator integration tests — consolidated into a single test binary.
//!
//! Each sub-module covers one coordinator; shared helpers live here.

mod challenge;
mod checkpoint;
mod replica_sync;

use std::sync::Arc;
use storage_provider_node::{ProviderState, Storage};

/// Full Alice SS58 address (substrate prefix 42).
pub const ALICE_SS58: &str = "5GrwvaEF5zXb26Fz9rcQpDWS57CtERHpNehXCPcNoHGKutQY";
pub const ALICE_SEED: &str = "//Alice";

/// Create a standard test `ProviderState` for coordinator tests.
pub fn test_state() -> Arc<ProviderState> {
    let storage = Arc::new(Storage::new());
    Arc::new(ProviderState::new(storage, ALICE_SS58.to_string()))
}

/// Create a test `ProviderState` with a keypair derived from the given seed.
pub fn test_state_with_seed(seed: &str) -> Arc<ProviderState> {
    let storage = Arc::new(Storage::new());
    Arc::new(ProviderState::with_seed(storage, seed).unwrap())
}

/// Poll a condition with timeout. Returns `true` if the condition was met
/// before the timeout, `false` otherwise.
pub async fn wait_for<F, Fut>(timeout_secs: u64, poll_ms: u64, mut f: F) -> bool
where
    F: FnMut() -> Fut,
    Fut: std::future::Future<Output = bool>,
{
    let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(timeout_secs);
    loop {
        if f().await {
            return true;
        }
        if tokio::time::Instant::now() >= deadline {
            return false;
        }
        tokio::time::sleep(std::time::Duration::from_millis(poll_ms)).await;
    }
}
