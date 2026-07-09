// SPDX-License-Identifier: Apache-2.0

//! E2E Workflow 04 — Data Upload & Retrieval (happy-case port of
//! `examples/papi/e2e/04-data-upload-and-retrieval.ts`).
//!
//! Accounts: `alice` (provider), `bob` (client).
//!
//! Covers raw-chunk upload/download roundtrips at different sizes, sequential
//! uploads, binary data, duplicate content, and blake2-256 hash verification
//! via `StorageUserClient`. The S3 HTTP sub-cases (4.5-4.8) and the
//! non-existent-hash failure case (4.9) are out of scope for this happy-case,
//! Layer-0-only suite.
//!
//! Requires a running parachain (`just start-chain`) and a live provider
//! node registered on-chain (`just start-provider`); skipped (not failed)
//! when either is unreachable.

#[path = "common.rs"]
mod e2e_common;

use e2e_common::common::chain_guard;
use e2e_common::{
    current_block, ensure_provider_registered, negotiate_and_establish, PROVIDER_URL,
};
use sp_core::H256;
use storage_client::{ChunkingStrategy, ClientConfig, StorageUserClient};
use storage_primitives::{blake2_256, BucketId};

/// `bob` is the bucket admin (he redeemed the terms in `setup_bucket`), so he
/// has Writer access - required by the provider node's default auth-enabled
/// config for `upload`/`commit`.
fn user_client() -> StorageUserClient {
    StorageUserClient::new(ClientConfig {
        chain_ws_url: e2e_common::common::CHAIN_WS.to_string(),
        provider_urls: vec![PROVIDER_URL.to_string()],
        timeout_secs: 30,
        enable_retries: false,
    })
    .expect("ClientConfig should be valid")
    .with_dev_signer("bob")
    .expect("bob is a valid dev signer")
}

/// Establish a fresh bucket for upload tests. Returns `None` if the chain or
/// provider is unreachable.
async fn setup_bucket() -> Option<BucketId> {
    ensure_provider_registered("alice", 1).await?;
    let alice_ss58 = e2e_common::common::dev_ss58("alice");
    negotiate_and_establish("bob", &alice_ss58, 10_485_760, 100, 1).await
}

fn random_bytes(n: usize) -> Vec<u8> {
    use rand::RngCore;
    let mut buf = vec![0u8; n];
    rand::thread_rng().fill_bytes(&mut buf);
    buf
}

/// 4.1/4.2/4.3 - Roundtrip integrity at small (100B), medium (64KiB), and max
/// (256KiB) chunk sizes - all single-leaf under the default chunking strategy.
#[tokio::test]
async fn upload_download_roundtrip_at_various_sizes() {
    let _guard = chain_guard().await;
    let Some(bucket_id) = setup_bucket().await else {
        eprintln!("skipping: chain or provider node unreachable");
        return;
    };
    let client = user_client();

    for size in [100usize, 64 * 1024, 256 * 1024] {
        let data = random_bytes(size);
        let data_root = client
            .upload(bucket_id, &data, ChunkingStrategy::default())
            .await
            .unwrap_or_else(|e| panic!("upload({size} bytes) failed: {e}"));
        let downloaded = client
            .download(&data_root, 0, data.len() as u64)
            .await
            .unwrap_or_else(|e| panic!("download({size} bytes) failed: {e}"));
        assert_eq!(downloaded, data, "{size}-byte roundtrip should match");
    }
}

/// 4.4 - Multiple sequential uploads produce strictly increasing leaf indices
/// and a changing MMR root.
#[tokio::test]
async fn sequential_uploads_increment_leaf_index() {
    let _guard = chain_guard().await;
    let Some(bucket_id) = setup_bucket().await else {
        eprintln!("skipping: chain or provider node unreachable");
        return;
    };
    let Some(nonce) = current_block().await else {
        eprintln!("skipping: chain unreachable");
        return;
    };
    let client = user_client();

    let mut commits = Vec::new();
    for i in 0..5 {
        let data = format!("sequential upload #{i}");
        let data_root = client
            .upload(bucket_id, data.as_bytes(), ChunkingStrategy::default())
            .await
            .expect("upload should succeed");
        let commit = client
            .commit(bucket_id, vec![data_root], nonce as u64)
            .await
            .expect("commit should succeed");
        commits.push(commit);
    }

    for i in 1..commits.len() {
        assert!(
            commits[i].leaf_indices[0] > commits[i - 1].leaf_indices[0],
            "leaf index should increase: {} > {}",
            commits[i].leaf_indices[0],
            commits[i - 1].leaf_indices[0]
        );
    }
    assert_ne!(
        commits[0].mmr_root, commits[4].mmr_root,
        "MMR root should change with new uploads"
    );
}

/// 4.10 - Binary (non-UTF8) data roundtrips correctly.
#[tokio::test]
async fn upload_binary_data_roundtrip() {
    let _guard = chain_guard().await;
    let Some(bucket_id) = setup_bucket().await else {
        eprintln!("skipping: chain or provider node unreachable");
        return;
    };
    let client = user_client();

    let binary = random_bytes(512);
    let data_root = client
        .upload(bucket_id, &binary, ChunkingStrategy::default())
        .await
        .expect("upload should succeed");
    let downloaded = client
        .download(&data_root, 0, binary.len() as u64)
        .await
        .expect("download should succeed");
    assert_eq!(downloaded, binary, "binary roundtrip should match");
}

/// 4.11 - Uploading identical content twice yields the same data root but
/// distinct leaf indices in the MMR.
#[tokio::test]
async fn duplicate_content_same_root_different_leaves() {
    let _guard = chain_guard().await;
    let Some(bucket_id) = setup_bucket().await else {
        eprintln!("skipping: chain or provider node unreachable");
        return;
    };
    let Some(nonce) = current_block().await else {
        eprintln!("skipping: chain unreachable");
        return;
    };
    let client = user_client();
    let data = b"duplicate content for e2e";

    let root1 = client
        .upload(bucket_id, data, ChunkingStrategy::default())
        .await
        .expect("first upload should succeed");
    let root2 = client
        .upload(bucket_id, data, ChunkingStrategy::default())
        .await
        .expect("second upload should succeed");
    assert_eq!(root1, root2, "same data should produce same root");

    let commit1 = client
        .commit(bucket_id, vec![root1], nonce as u64)
        .await
        .expect("first commit should succeed");
    let commit2 = client
        .commit(bucket_id, vec![root2], nonce as u64)
        .await
        .expect("second commit should succeed");
    assert_ne!(
        commit1.leaf_indices[0], commit2.leaf_indices[0],
        "leaf indices should differ for separate uploads"
    );
}

/// 4.12 - A single-chunk upload's data root equals blake2-256 of the raw
/// bytes (a one-leaf Merkle tree's root is just the leaf hash).
#[tokio::test]
async fn data_root_matches_blake2_256_for_single_chunk() {
    let _guard = chain_guard().await;
    let Some(bucket_id) = setup_bucket().await else {
        eprintln!("skipping: chain or provider node unreachable");
        return;
    };
    let client = user_client();

    let data = b"verify hash computation";
    let expected: H256 = blake2_256(data);
    let data_root = client
        .upload(bucket_id, data, ChunkingStrategy::default())
        .await
        .expect("upload should succeed");
    assert_eq!(
        data_root, expected,
        "data_root should match local blake2-256"
    );
}
