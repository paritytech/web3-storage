//! Integration tests for `StorageUserClient`.
//!
//! Each test spins up a real in-process provider node on a random port so
//! there is no external dependency. On-chain operations are not exercised
//! here — see the end-to-end demo in `justfile` for those.

mod common;

use common::{make_client, start_test_provider};
use sp_core::H256;
use storage_client::{ChunkingStrategy, EncryptionKey, ENCRYPTION_OVERHEAD};

// ============================================================================
// Health
// ============================================================================

#[tokio::test]
async fn test_health_check() {
    let url = start_test_provider().await;
    let client = make_client(url);

    let health = client.health().await.unwrap();
    assert_eq!(health.status, "healthy");
}

// ============================================================================
// Upload
// ============================================================================

#[tokio::test]
async fn test_upload_returns_nonzero_root() {
    let url = start_test_provider().await;
    let client = make_client(url);

    let data_root = client
        .upload(1, b"Hello, World!", ChunkingStrategy::default())
        .await
        .unwrap();

    assert_ne!(data_root, H256::zero());
}

#[tokio::test]
async fn test_upload_empty_data_returns_error() {
    let url = start_test_provider().await;
    let client = make_client(url);

    let result = client.upload(1, b"", ChunkingStrategy::default()).await;

    // Empty data has no Merkle leaves — the client should surface a clear error.
    assert!(result.is_err(), "expected error for empty upload, got Ok");
}

#[tokio::test]
async fn test_upload_large_data_multiple_chunks() {
    let url = start_test_provider().await;
    let client = make_client(url);

    let chunk_size = 1024usize;
    let data: Vec<u8> = (0..chunk_size * 3 + 500).map(|i| (i % 256) as u8).collect();

    let data_root = client
        .upload(1, &data, ChunkingStrategy::Fixed(chunk_size))
        .await
        .unwrap();

    assert_ne!(data_root, H256::zero());
}

#[tokio::test]
async fn test_same_data_produces_same_root() {
    let url = start_test_provider().await;
    let client = make_client(url);

    let data = b"deterministic content";

    let root1 = client
        .upload(1, data, ChunkingStrategy::default())
        .await
        .unwrap();
    let root2 = client
        .upload(2, data, ChunkingStrategy::default())
        .await
        .unwrap();

    // Content-addressed storage: same bytes → same root regardless of bucket.
    assert_eq!(root1, root2);
}

#[tokio::test]
async fn test_different_data_produces_different_roots() {
    let url = start_test_provider().await;
    let client = make_client(url);

    let root1 = client
        .upload(1, b"data version one", ChunkingStrategy::default())
        .await
        .unwrap();
    let root2 = client
        .upload(1, b"data version two", ChunkingStrategy::default())
        .await
        .unwrap();

    assert_ne!(root1, root2);
}

// ============================================================================
// Commit
// ============================================================================

#[tokio::test]
async fn test_upload_then_commit() {
    let url = start_test_provider().await;
    let client = make_client(url);

    let data_root = client
        .upload(1, b"Test data for commit", ChunkingStrategy::default())
        .await
        .unwrap();

    let commit = client.commit(1, vec![data_root], 0u64).await.unwrap();

    assert!(!commit.mmr_root.is_empty());
    assert_eq!(commit.start_seq, 0);
    assert_eq!(commit.leaf_indices, vec![0]);
}

#[tokio::test]
async fn test_sequential_commits_increment_leaf_index() {
    let url = start_test_provider().await;
    let client = make_client(url);

    let root1 = client
        .upload(1, b"First piece of data", ChunkingStrategy::default())
        .await
        .unwrap();
    let commit1 = client.commit(1, vec![root1], 0u64).await.unwrap();
    assert_eq!(commit1.leaf_indices, vec![0]);

    let root2 = client
        .upload(1, b"Second piece of data", ChunkingStrategy::default())
        .await
        .unwrap();
    let commit2 = client.commit(1, vec![root2], 0u64).await.unwrap();
    assert_eq!(commit2.leaf_indices, vec![1]);
}

#[tokio::test]
async fn test_batch_commit_multiple_roots() {
    let url = start_test_provider().await;
    let client = make_client(url);

    let root1 = client
        .upload(1, b"Part 1", ChunkingStrategy::default())
        .await
        .unwrap();
    let root2 = client
        .upload(1, b"Part 2", ChunkingStrategy::default())
        .await
        .unwrap();
    let root3 = client
        .upload(1, b"Part 3", ChunkingStrategy::default())
        .await
        .unwrap();

    let commit = client
        .commit(1, vec![root1, root2, root3], 0u64)
        .await
        .unwrap();

    assert_eq!(commit.leaf_indices, vec![0, 1, 2]);
}

#[tokio::test]
async fn test_commit_nonexistent_root_returns_error() {
    let url = start_test_provider().await;
    let client = make_client(url);

    // First upload so the bucket is initialised, then commit a random root.
    client
        .upload(1, b"seed", ChunkingStrategy::default())
        .await
        .unwrap();

    let phantom = H256::from_slice(&[0xAB; 32]);
    let result = client.commit(1, vec![phantom], 0u64).await;

    assert!(
        result.is_err(),
        "committing a root that was never uploaded should fail"
    );
}

// ============================================================================
// Download / roundtrip
// ============================================================================

#[tokio::test]
async fn test_upload_commit_download_roundtrip() {
    let url = start_test_provider().await;
    let client = make_client(url);

    let original = b"This is the original data that we want to store and retrieve.";
    let data_root = client
        .upload(1, original, ChunkingStrategy::default())
        .await
        .unwrap();
    client.commit(1, vec![data_root], 0u64).await.unwrap();

    let retrieved = client
        .download(&data_root, 0, original.len() as u64)
        .await
        .unwrap();

    assert_eq!(retrieved, original);
}

#[tokio::test]
async fn test_download_full_roundtrip() {
    let url = start_test_provider().await;
    let client = make_client(url);

    let original = b"download_full should return the complete blob";
    let data_root = client
        .upload(1, original, ChunkingStrategy::default())
        .await
        .unwrap();
    client.commit(1, vec![data_root], 0u64).await.unwrap();

    let retrieved = client
        .download_full(&data_root, original.len() as u64)
        .await
        .unwrap();

    assert_eq!(retrieved, original);
}

#[tokio::test]
async fn test_download_partial_offset() {
    let url = start_test_provider().await;
    let client = make_client(url);

    // Use a small fixed chunk size so we get predictable chunk boundaries.
    let chunk_size = 16usize;
    let data: Vec<u8> = (0u8..64).collect(); // 64 bytes, 4 chunks of 16
    let data_root = client
        .upload(1, &data, ChunkingStrategy::Fixed(chunk_size))
        .await
        .unwrap();
    client.commit(1, vec![data_root], 0u64).await.unwrap();

    // Download chunk 0 (bytes 0..16)
    let chunk0 = client
        .download(&data_root, 0, chunk_size as u64)
        .await
        .unwrap();
    assert_eq!(chunk0, &data[..chunk_size]);
}

// ============================================================================
// Read node
// ============================================================================

#[tokio::test]
async fn test_read_node_returns_leaf_data() {
    let url = start_test_provider().await;
    let client = make_client(url);

    // Single-chunk upload: the data root IS the leaf hash.
    let data = b"small leaf data";
    let data_root = client
        .upload(1, data, ChunkingStrategy::default())
        .await
        .unwrap();

    let (node_data, _children) = client.read_node(&data_root).await.unwrap();

    assert_eq!(node_data, data);
}

#[tokio::test]
async fn test_read_node_verified_passes_for_valid_hash() {
    let url = start_test_provider().await;
    let client = make_client(url);

    let data = b"node to verify";
    let data_root = client
        .upload(1, data, ChunkingStrategy::default())
        .await
        .unwrap();

    let verified_data = client.read_node_verified(&data_root).await.unwrap();

    assert_eq!(verified_data, data);
}

#[tokio::test]
async fn test_read_node_fails_for_unknown_hash() {
    let url = start_test_provider().await;
    let client = make_client(url);

    let phantom = H256::from_slice(&[0xFF; 32]);
    let result = client.read_node(&phantom).await;

    assert!(result.is_err(), "reading a non-existent node should fail");
}

#[tokio::test]
async fn test_read_internal_node_has_children() {
    let url = start_test_provider().await;
    let client = make_client(url);

    // Two chunks → root is an internal node with two leaf children.
    let chunk_size = 8usize;
    let data = b"leftright"; // 9 bytes → 2 chunks with size 8
    let data_root = client
        .upload(1, data, ChunkingStrategy::Fixed(chunk_size))
        .await
        .unwrap();

    let (_node_data, children) = client.read_node(&data_root).await.unwrap();

    assert!(
        children.is_some(),
        "internal node should report its children"
    );
    assert_eq!(
        children.unwrap().len(),
        2,
        "two-chunk upload should produce a root with two children"
    );
}

// ============================================================================
// Encryption
// ============================================================================

#[tokio::test]
async fn test_encryption_roundtrip() {
    let url = start_test_provider().await;
    let key = EncryptionKey::generate();
    let client = make_client(url).with_encryption_key(&key);

    assert!(client.is_encryption_enabled());

    let original = b"secret message that the provider never sees in plaintext";
    let data_root = client
        .upload(1, original, ChunkingStrategy::default())
        .await
        .unwrap();
    client.commit(1, vec![data_root], 0u64).await.unwrap();

    // The provider stores ciphertext, which is larger than the plaintext by
    // ENCRYPTION_OVERHEAD bytes.  We must fetch the full ciphertext so the
    // cipher can verify the authentication tag before decrypting.
    let enc_len = (original.len() + ENCRYPTION_OVERHEAD) as u64;
    let retrieved = client.download(&data_root, 0, enc_len).await.unwrap();

    assert_eq!(retrieved, original);
}

#[tokio::test]
async fn test_encrypted_upload_differs_from_plaintext_upload() {
    let url = start_test_provider().await;

    let data = b"same content, different roots";

    // Plain upload
    let plain_client = make_client(url.clone());
    let plain_root = plain_client
        .upload(1, data, ChunkingStrategy::default())
        .await
        .unwrap();

    // Encrypted upload
    let key = EncryptionKey::generate();
    let enc_client = make_client(url).with_encryption_key(&key);
    let enc_root = enc_client
        .upload(2, data, ChunkingStrategy::default())
        .await
        .unwrap();

    // Encrypted ciphertext differs from plaintext → different content-addressed root.
    assert_ne!(
        plain_root, enc_root,
        "encrypted upload should produce a different data root than plaintext"
    );
}

#[tokio::test]
async fn test_decryption_with_wrong_key_fails() {
    let url = start_test_provider().await;

    let key1 = EncryptionKey::generate();
    let client1 = make_client(url.clone()).with_encryption_key(&key1);

    let data = b"secret";
    let data_root = client1
        .upload(1, data, ChunkingStrategy::default())
        .await
        .unwrap();
    client1.commit(1, vec![data_root], 0u64).await.unwrap();

    // Try to decrypt with a different key.
    // We must request the full ciphertext length so the cipher can attempt
    // tag verification (and fail) rather than returning empty data.
    let key2 = EncryptionKey::generate();
    let client2 = make_client(url).with_encryption_key(&key2);
    let enc_len = (data.len() + ENCRYPTION_OVERHEAD) as u64;

    let result = client2.download(&data_root, 0, enc_len).await;

    assert!(
        result.is_err(),
        "decrypting with wrong key should return an error"
    );
}

// ============================================================================
// Commitment query
// ============================================================================

#[tokio::test]
async fn test_get_commitment_reflects_uploads() {
    let url = start_test_provider().await;
    let client = make_client(url);

    let root = client
        .upload(1, b"Data for commitment test", ChunkingStrategy::default())
        .await
        .unwrap();
    client.commit(1, vec![root], 0u64).await.unwrap();

    let commitment = client.get_commitment(1, 0u64).await.unwrap();

    assert_eq!(commitment.bucket_id, 1);
    assert_eq!(commitment.leaf_count, 1);
    assert_eq!(commitment.start_seq, 0);
    assert!(!commitment.mmr_root.is_empty());
}

#[tokio::test]
async fn test_sequential_commits_grow_leaf_count() {
    let url = start_test_provider().await;
    let client = make_client(url);

    let root1 = client
        .upload(1, b"Leaf 1", ChunkingStrategy::default())
        .await
        .unwrap();
    client.commit(1, vec![root1], 0u64).await.unwrap();

    let root2 = client
        .upload(1, b"Leaf 2", ChunkingStrategy::default())
        .await
        .unwrap();
    client.commit(1, vec![root2], 0u64).await.unwrap();

    let commitment = client.get_commitment(1, 0u64).await.unwrap();
    assert_eq!(commitment.leaf_count, 2);
}

#[tokio::test]
async fn test_get_commitment_fails_for_empty_bucket() {
    let url = start_test_provider().await;
    let client = make_client(url);

    // Bucket 99 has never received any data.
    let result = client.get_commitment(99, 0u64).await;

    assert!(
        result.is_err(),
        "get_commitment on a bucket with no data should fail"
    );
}

// ============================================================================
// Check exists
// ============================================================================

#[tokio::test]
async fn test_check_exists_distinguishes_present_and_absent() {
    let url = start_test_provider().await;
    let client = make_client(url);

    let data_root = client
        .upload(1, b"Data to check existence", ChunkingStrategy::default())
        .await
        .unwrap();

    let absent = H256::from_slice(&[0xDE; 32]);
    let response = client
        .check_exists(1, vec![data_root, absent])
        .await
        .unwrap();

    assert!(!response.exists.is_empty(), "uploaded root should exist");
    assert!(!response.missing.is_empty(), "dummy hash should be missing");
}

#[tokio::test]
async fn test_check_exists_all_present() {
    let url = start_test_provider().await;
    let client = make_client(url);

    let root1 = client
        .upload(1, b"exists a", ChunkingStrategy::default())
        .await
        .unwrap();
    let root2 = client
        .upload(1, b"exists b", ChunkingStrategy::default())
        .await
        .unwrap();

    let response = client.check_exists(1, vec![root1, root2]).await.unwrap();

    assert_eq!(response.missing.len(), 0, "all uploaded roots should exist");
    assert_eq!(response.exists.len(), 2);
}

#[tokio::test]
async fn test_check_exists_all_absent() {
    let url = start_test_provider().await;
    let client = make_client(url);

    let phantom1 = H256::from_slice(&[0x11; 32]);
    let phantom2 = H256::from_slice(&[0x22; 32]);

    let response = client
        .check_exists(1, vec![phantom1, phantom2])
        .await
        .unwrap();

    assert_eq!(response.exists.len(), 0, "no roots should be found");
    assert_eq!(response.missing.len(), 2);
}

// ============================================================================
// Spot check
// ============================================================================

#[tokio::test]
async fn test_spot_check_passes_for_valid_data() {
    let url = start_test_provider().await;
    let mut client = make_client(url);

    let data = b"spot check me";
    let data_root = client
        .upload(1, data, ChunkingStrategy::default())
        .await
        .unwrap();
    client.commit(1, vec![data_root], 0u64).await.unwrap();

    let passed = client.spot_check(&data_root, 0).await.unwrap();

    assert!(passed, "spot check on valid data should pass");
}

#[tokio::test]
async fn test_spot_check_returns_false_for_missing_data() {
    let url = start_test_provider().await;
    let mut client = make_client(url);

    let phantom = H256::from_slice(&[0xCC; 32]);
    let passed = client.spot_check(&phantom, 0).await.unwrap();

    assert!(
        !passed,
        "spot check on a non-existent root should return false"
    );
}

// ============================================================================
// Bucket isolation
// ============================================================================

#[tokio::test]
async fn test_different_buckets_are_independent() {
    let url = start_test_provider().await;
    let client = make_client(url);

    let root1 = client
        .upload(1, b"Bucket 1 data", ChunkingStrategy::default())
        .await
        .unwrap();
    client.commit(1, vec![root1], 0u64).await.unwrap();

    let root2 = client
        .upload(2, b"Bucket 2 data", ChunkingStrategy::default())
        .await
        .unwrap();
    client.commit(2, vec![root2], 0u64).await.unwrap();

    let c1 = client.get_commitment(1, 0u64).await.unwrap();
    let c2 = client.get_commitment(2, 0u64).await.unwrap();

    assert_eq!(c1.bucket_id, 1);
    assert_eq!(c2.bucket_id, 2);
    assert_ne!(c1.mmr_root, c2.mmr_root);
}
