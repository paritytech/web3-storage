// SPDX-License-Identifier: GPL-3.0-only

//! Coordinator integration tests — consolidated into a single test binary.
//!
//! Each sub-module covers one coordinator; shared helpers live here.

mod challenge;
mod checkpoint;
mod event_fanout;
mod replica_sync;

use std::sync::Arc;
use storage_primitives::blake2_256;
use storage_provider_node::{build_padded_merkle_tree, DetectedChallenge, ProviderState, Storage};

/// Full Alice SS58 address (substrate prefix 42).
pub const ALICE_SS58: &str = "5GrwvaEF5zXb26Fz9rcQpDWS57CtERHpNehXCPcNoHGKutQY";
pub const ALICE_SEED: &str = "//Alice";

/// Create a standard test `ProviderState` for coordinator tests.
pub fn test_state() -> Arc<ProviderState> {
    let storage = Arc::new(Storage::new());
    Arc::new(ProviderState::with_provider_id(
        storage,
        ALICE_SS58.to_string(),
    ))
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

/// Create a provider state with a bucket containing a single committed chunk,
/// and return the state along with a matching challenge.
pub fn test_state_with_data() -> (Arc<ProviderState>, DetectedChallenge) {
    let storage = Arc::new(Storage::new());
    storage.init_bucket(1, 1024 * 1024);

    let chunk_data = b"test-chunk-data-for-challenge";
    let chunk_hash = blake2_256(chunk_data);
    storage
        .store_node(1, chunk_hash, chunk_data.to_vec(), None)
        .unwrap();

    let data_root = build_padded_merkle_tree(storage.as_ref(), 1, &[chunk_hash]);
    assert_eq!(data_root, chunk_hash);

    let (mmr_root, start_seq, leaf_indices) = storage.commit(1, vec![data_root]).unwrap();
    assert_eq!(leaf_indices, vec![0]);

    let challenge = DetectedChallenge {
        bucket_id: 1,
        deadline: 1000,
        index: 0,
        mmr_root,
        start_seq,
        leaf_index: 0,
        chunk_index: 0,
        challenger: ALICE_SS58.to_string(),
    };

    let state = Arc::new(ProviderState::with_provider_id(
        storage,
        ALICE_SS58.to_string(),
    ));
    (state, challenge)
}
