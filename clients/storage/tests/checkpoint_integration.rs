// SPDX-License-Identifier: Apache-2.0

//! Integration tests for checkpoint-related provider behaviour.
//!
//! These tests require an in-process provider node and verify operations
//! that are specific to the checkpoint workflow (signatures, multi-provider
//! setups). Unit tests for the checkpoint types live in `src/checkpoint.rs`.

mod common;

use common::{make_client, start_test_provider};
use storage_client::ChunkingStrategy;

// ============================================================================
// Provider health
// ============================================================================

#[tokio::test]
async fn test_provider_health_returns_healthy() {
    let url = start_test_provider().await;
    let client = make_client(url);

    let health = client.health().await.unwrap();

    assert_eq!(health.status, "healthy");
    assert!(!health.version.is_empty());
}

// ============================================================================
// Checkpoint signature
// ============================================================================

#[tokio::test]
async fn test_checkpoint_signature_available_after_commit() {
    let url = start_test_provider().await;
    let client = make_client(url);

    let bucket_id = 1u64;
    let data_root = client
        .upload(
            bucket_id,
            b"data to checkpoint",
            ChunkingStrategy::default(),
        )
        .await
        .unwrap();
    client.commit(bucket_id, vec![data_root]).await.unwrap();

    let sig = client.get_checkpoint_signature(bucket_id).await.unwrap();

    assert_eq!(sig.bucket_id, bucket_id);
    assert_eq!(sig.leaf_count, 1);
    assert_ne!(sig.mmr_root, sp_core::H256::zero());
    // Deserialization already proves the signature decodes; pin the scheme.
    assert!(matches!(
        sig.provider_signature,
        sp_runtime::MultiSignature::Sr25519(_)
    ));
}

// ============================================================================
// Multi-provider independence
// ============================================================================

#[tokio::test]
async fn test_multiple_providers_track_commitments_independently() {
    let urls = common::start_providers(3).await;

    // Each provider gets a different bucket's data.
    for (i, url) in urls.iter().enumerate() {
        let client = make_client(url.clone());
        let bucket_id = (i + 1) as u64;
        let data = format!("provider {i} data");

        let root = client
            .upload(bucket_id, data.as_bytes(), ChunkingStrategy::default())
            .await
            .unwrap();
        client.commit(bucket_id, vec![root]).await.unwrap();

        let commitment = client.get_commitment(bucket_id).await.unwrap();
        assert_eq!(commitment.bucket_id, bucket_id);
        assert_eq!(commitment.leaf_count, 1);
    }

    // Cross-check: provider 0's URL does not know about provider 1's bucket.
    let client0 = make_client(urls[0].clone());
    assert!(
        client0.get_commitment(2).await.is_err(),
        "provider 0 should not have bucket 2's data"
    );
}
