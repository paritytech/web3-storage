// SPDX-License-Identifier: GPL-3.0-only

//! Coordinator integration tests — consolidated into a single test binary.
//!
//! Each sub-module covers one coordinator; shared helpers live here.

mod challenge;
mod event_fanout;
mod gc;
mod membership;
mod replica_sync;

use provider_auth::{Authenticator, StaticMembershipResolver};
use provider_storage::{build_padded_merkle_tree, temp_rocksdb, StorageBackend};
use sp_runtime::AccountId32;
use std::str::FromStr;
use std::sync::Arc;
use storage_primitives::blake2_256;
use storage_provider_node::{DetectedChallenge, ProviderDeps, ProviderState};
use tempfile::TempDir;

/// Full Alice SS58 address (substrate prefix 42).
pub const ALICE_SS58: &str = "5GrwvaEF5zXb26Fz9rcQpDWS57CtERHpNehXCPcNoHGKutQY";
pub const ALICE_SEED: &str = "//Alice";

/// [`ALICE_SS58`] decoded to an [`AccountId32`].
pub fn alice_account() -> AccountId32 {
    AccountId32::from_str(ALICE_SS58).unwrap()
}

/// Standard test dependencies around the given backend: an empty static
/// membership set.
pub fn test_deps(
    storage: Arc<dyn StorageBackend>,
    nonce_store: Arc<dyn provider_storage::NonceStore>,
) -> ProviderDeps {
    ProviderDeps {
        storage,
        nonce_store,
        auth: Arc::new(Authenticator::new(StaticMembershipResolver(vec![]))),
    }
}

/// Create a standard test `ProviderState` for coordinator tests.
pub fn test_state() -> (Arc<ProviderState>, TempDir) {
    let (storage, nonce_store, dir) = temp_rocksdb();
    let state = Arc::new(ProviderState::with_provider_id(
        test_deps(storage, nonce_store),
        ALICE_SS58.to_string(),
    ));
    (state, dir)
}

/// Create a test `ProviderState` with a keypair derived from the given seed.
pub fn test_state_with_seed(seed: &str) -> (Arc<ProviderState>, TempDir) {
    let (storage, nonce_store, dir) = temp_rocksdb();
    let state = Arc::new(ProviderState::with_seed(test_deps(storage, nonce_store), seed).unwrap());
    (state, dir)
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
pub fn test_state_with_data() -> (Arc<ProviderState>, DetectedChallenge, TempDir) {
    let (storage, nonce_store, dir) = temp_rocksdb();
    storage
        .init_bucket(1, 1024 * 1024)
        .expect("bucket initialises");

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
        test_deps(storage, nonce_store),
        ALICE_SS58.to_string(),
    ));
    (state, challenge, dir)
}
