//! Integration tests for storage utility functions (hex, merkle tree, chunk operations).

use sp_core::H256;
use std::sync::Arc;
use storage_provider_node::{
    build_merkle_proof, build_padded_merkle_tree, hex_decode, hex_encode, Storage, StorageBackend,
};

fn test_storage() -> Arc<Storage> {
    Arc::new(Storage::new())
}

// ─────────────────────────────────────────────────────────────────────────────
// hex_encode / hex_decode
// ─────────────────────────────────────────────────────────────────────────────

#[test]
fn test_hex_encode_decode_roundtrip() {
    let original = vec![0xDE, 0xAD, 0xBE, 0xEF, 0x00, 0xFF];
    let encoded = hex_encode(&original);
    assert_eq!(encoded, "deadbeef00ff");

    let decoded = hex_decode(&encoded).unwrap();
    assert_eq!(decoded, original);
}

#[test]
fn test_hex_decode_0x_prefix() {
    let decoded = hex_decode("0xabcd").unwrap();
    assert_eq!(decoded, vec![0xAB, 0xCD]);

    // Without prefix should give the same result
    let decoded_no_prefix = hex_decode("abcd").unwrap();
    assert_eq!(decoded, decoded_no_prefix);
}

#[test]
fn test_hex_decode_invalid_chars() {
    let result = hex_decode("zzzz");
    assert!(result.is_err());
}

#[test]
fn test_hex_decode_odd_length() {
    let result = hex_decode("abc");
    assert!(result.is_err());
}

#[test]
fn test_hex_encode_empty() {
    assert_eq!(hex_encode(&[]), "");
    assert_eq!(hex_decode("").unwrap(), Vec::<u8>::new());
}

// ─────────────────────────────────────────────────────────────────────────────
// build_padded_merkle_tree
// ─────────────────────────────────────────────────────────────────────────────

#[test]
fn test_build_padded_merkle_tree_empty() {
    let storage = test_storage();
    let root = build_padded_merkle_tree(storage.as_ref(), 1, &[]);
    assert_eq!(root, H256::zero());
}

#[test]
fn test_build_padded_merkle_tree_single() {
    let storage = test_storage();
    storage.init_bucket(1, u64::MAX);

    let leaf = H256::repeat_byte(0x42);
    let root = build_padded_merkle_tree(storage.as_ref(), 1, &[leaf]);
    // Single leaf → root is the leaf itself
    assert_eq!(root, leaf);
}

#[test]
fn test_build_padded_merkle_tree_two_leaves() {
    let storage = test_storage();
    storage.init_bucket(1, u64::MAX);

    // Store actual leaf nodes so children-exist checks pass
    let data_a = b"leaf A data";
    let leaf_a = storage_primitives::blake2_256(data_a);
    storage
        .store_node(1, leaf_a, data_a.to_vec(), None)
        .unwrap();

    let data_b = b"leaf B data";
    let leaf_b = storage_primitives::blake2_256(data_b);
    storage
        .store_node(1, leaf_b, data_b.to_vec(), None)
        .unwrap();

    let root = build_padded_merkle_tree(storage.as_ref(), 1, &[leaf_a, leaf_b]);

    // Root should not be zero and should differ from both leaves
    assert_ne!(root, H256::zero());
    assert_ne!(root, leaf_a);
    assert_ne!(root, leaf_b);

    // Internal node should be stored in storage
    let node = storage.get_node(&root).expect("root node should be stored");
    assert!(node.children.is_some());
    let children = node.children.unwrap();
    assert_eq!(children.len(), 2);
    assert_eq!(children[0], leaf_a);
    assert_eq!(children[1], leaf_b);
}

#[test]
fn test_build_padded_merkle_tree_three_leaves() {
    let storage = test_storage();
    storage.init_bucket(1, u64::MAX);

    // Store actual leaf nodes
    let data_a = b"three A";
    let leaf_a = storage_primitives::blake2_256(data_a);
    storage
        .store_node(1, leaf_a, data_a.to_vec(), None)
        .unwrap();

    let data_b = b"three B";
    let leaf_b = storage_primitives::blake2_256(data_b);
    storage
        .store_node(1, leaf_b, data_b.to_vec(), None)
        .unwrap();

    let data_c = b"three C";
    let leaf_c = storage_primitives::blake2_256(data_c);
    storage
        .store_node(1, leaf_c, data_c.to_vec(), None)
        .unwrap();

    let root = build_padded_merkle_tree(storage.as_ref(), 1, &[leaf_a, leaf_b, leaf_c]);

    // 3 leaves pads to 4, so we get a full binary tree
    assert_ne!(root, H256::zero());

    // The root node should be stored and have 2 children
    let root_node = storage.get_node(&root).unwrap();
    let children = root_node.children.as_ref().unwrap();
    assert_eq!(children.len(), 2);
}

// ─────────────────────────────────────────────────────────────────────────────
// build_merkle_proof
// ─────────────────────────────────────────────────────────────────────────────

#[test]
fn test_build_merkle_proof_single() {
    let leaf = H256::repeat_byte(0x42);
    let proof = build_merkle_proof(&[leaf], 0);
    // Single leaf: no siblings needed
    assert!(proof.siblings.is_empty());
    assert!(proof.path.is_empty());
}

#[test]
fn test_build_merkle_proof_two_leaves() {
    let leaf_a = H256::repeat_byte(0x01);
    let leaf_b = H256::repeat_byte(0x02);

    // Proof for index 0
    let proof_0 = build_merkle_proof(&[leaf_a, leaf_b], 0);
    assert_eq!(proof_0.siblings.len(), 1);
    assert_eq!(proof_0.siblings[0], leaf_b); // sibling is the other leaf
    assert_eq!(proof_0.path, vec![false]); // index 0 is left

    // Proof for index 1
    let proof_1 = build_merkle_proof(&[leaf_a, leaf_b], 1);
    assert_eq!(proof_1.siblings.len(), 1);
    assert_eq!(proof_1.siblings[0], leaf_a); // sibling is the other leaf
    assert_eq!(proof_1.path, vec![true]); // index 1 is right
}

#[test]
fn test_build_merkle_proof_four_leaves() {
    let leaves: Vec<H256> = (1..=4).map(H256::repeat_byte).collect();
    let proof = build_merkle_proof(&leaves, 2);
    // 4 leaves → 2 levels → 2 siblings
    assert_eq!(proof.siblings.len(), 2);
}

// ─────────────────────────────────────────────────────────────────────────────
// collect_chunks / collect_chunk_hashes / calculate_tree_size
// ─────────────────────────────────────────────────────────────────────────────

#[test]
fn test_collect_chunks_single_leaf() {
    let storage = test_storage();
    storage.init_bucket(1, u64::MAX);

    let data = b"hello chunk";
    let hash = storage_primitives::blake2_256(data);
    storage.store_node(1, hash, data.to_vec(), None).unwrap();

    let chunks = storage.collect_chunks(hash);
    assert_eq!(chunks.len(), 1);
    assert_eq!(chunks[0], data);
}

#[test]
fn test_collect_chunks_multi_leaf_tree() {
    let storage = test_storage();
    storage.init_bucket(1, u64::MAX);

    // Store two leaf chunks
    let data_a = b"chunk A";
    let hash_a = storage_primitives::blake2_256(data_a);
    storage
        .store_node(1, hash_a, data_a.to_vec(), None)
        .unwrap();

    let data_b = b"chunk B";
    let hash_b = storage_primitives::blake2_256(data_b);
    storage
        .store_node(1, hash_b, data_b.to_vec(), None)
        .unwrap();

    // Build tree over them
    let root = build_padded_merkle_tree(storage.as_ref(), 1, &[hash_a, hash_b]);

    let chunks = storage.collect_chunks(root);
    assert_eq!(chunks.len(), 2);
    assert_eq!(chunks[0], data_a);
    assert_eq!(chunks[1], data_b);
}

#[test]
fn test_collect_chunk_hashes() {
    let storage = test_storage();
    storage.init_bucket(1, u64::MAX);

    let data_a = b"hash test A";
    let hash_a = storage_primitives::blake2_256(data_a);
    storage
        .store_node(1, hash_a, data_a.to_vec(), None)
        .unwrap();

    let data_b = b"hash test B";
    let hash_b = storage_primitives::blake2_256(data_b);
    storage
        .store_node(1, hash_b, data_b.to_vec(), None)
        .unwrap();

    let root = build_padded_merkle_tree(storage.as_ref(), 1, &[hash_a, hash_b]);

    let hashes = storage.collect_chunk_hashes(root);
    assert_eq!(hashes.len(), 2);
    assert_eq!(hashes[0], hash_a);
    assert_eq!(hashes[1], hash_b);
}

#[test]
fn test_calculate_tree_size() {
    let storage = test_storage();
    storage.init_bucket(1, u64::MAX);

    let data_a = b"five!"; // 5 bytes
    let hash_a = storage_primitives::blake2_256(data_a);
    storage
        .store_node(1, hash_a, data_a.to_vec(), None)
        .unwrap();

    let data_b = b"seven!!"; // 7 bytes
    let hash_b = storage_primitives::blake2_256(data_b);
    storage
        .store_node(1, hash_b, data_b.to_vec(), None)
        .unwrap();

    let root = build_padded_merkle_tree(storage.as_ref(), 1, &[hash_a, hash_b]);

    let size = storage.calculate_tree_size(root);
    assert_eq!(size, 12); // 5 + 7
}

#[test]
fn test_calculate_tree_size_single_leaf() {
    let storage = test_storage();
    storage.init_bucket(1, u64::MAX);

    let data = b"single leaf data";
    let hash = storage_primitives::blake2_256(data);
    storage.store_node(1, hash, data.to_vec(), None).unwrap();

    // Single leaf, no tree needed
    let size = storage.calculate_tree_size(hash);
    assert_eq!(size, data.len() as u64);
}

#[test]
fn test_calculate_tree_size_nonexistent() {
    let storage = test_storage();
    // Non-existent root returns 0
    let size = storage.calculate_tree_size(H256::repeat_byte(0xFF));
    assert_eq!(size, 0);
}

// ─────────────────────────────────────────────────────────────────────────────
// get_chunk_at_index
// ─────────────────────────────────────────────────────────────────────────────

#[test]
fn test_get_chunk_at_index_valid() {
    let storage = test_storage();
    storage.init_bucket(1, u64::MAX);

    let data_a = b"index test A";
    let hash_a = storage_primitives::blake2_256(data_a);
    storage
        .store_node(1, hash_a, data_a.to_vec(), None)
        .unwrap();

    let data_b = b"index test B";
    let hash_b = storage_primitives::blake2_256(data_b);
    storage
        .store_node(1, hash_b, data_b.to_vec(), None)
        .unwrap();

    let root = build_padded_merkle_tree(storage.as_ref(), 1, &[hash_a, hash_b]);

    // Get chunk at index 0
    let (chunk_data, proof) = storage.get_chunk_at_index(root, 0).unwrap();
    assert_eq!(chunk_data, data_a);
    assert!(!proof.siblings.is_empty());

    // Get chunk at index 1
    let (chunk_data, proof) = storage.get_chunk_at_index(root, 1).unwrap();
    assert_eq!(chunk_data, data_b);
    assert!(!proof.siblings.is_empty());
}

#[test]
fn test_get_chunk_at_index_out_of_bounds() {
    let storage = test_storage();
    storage.init_bucket(1, u64::MAX);

    let data = b"single chunk";
    let hash = storage_primitives::blake2_256(data);
    storage.store_node(1, hash, data.to_vec(), None).unwrap();

    // Index 1 is out of bounds (only 1 leaf at index 0)
    let result = storage.get_chunk_at_index(hash, 1);
    assert!(result.is_err());
}

#[test]
fn test_get_chunk_at_index_nonexistent_root() {
    let storage = test_storage();
    // Non-existent root → empty chunk hashes → any index is out of bounds
    let result = storage.get_chunk_at_index(H256::repeat_byte(0xFF), 0);
    assert!(result.is_err());
}

// ─────────────────────────────────────────────────────────────────────────────
// collect_chunks with H256::zero() root
// ─────────────────────────────────────────────────────────────────────────────

#[test]
fn test_collect_chunks_zero_root() {
    let storage = test_storage();
    // H256::zero() should be skipped → empty result
    let chunks = storage.collect_chunks(H256::zero());
    assert!(chunks.is_empty());
}

#[test]
fn test_collect_chunk_hashes_zero_root() {
    let storage = test_storage();
    let hashes = storage.collect_chunk_hashes(H256::zero());
    assert!(hashes.is_empty());
}
