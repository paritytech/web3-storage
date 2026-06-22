// SPDX-License-Identifier: GPL-3.0-only

//! In-memory storage backend for the provider node.
//!
//! This provides a simple in-memory storage implementation.
//! Production implementations would use disk-based storage.

use super::{BucketInfo, StorageBackend, StoredNode};
use crate::error::Error;
use crate::mmr::Mmr;
use crate::types::*;
use codec::Encode;
use dashmap::DashMap;
use parking_lot::RwLock;
use sp_core::H256;
use std::collections::HashMap;
use storage_primitives::{blake2_256, BucketId, MmrLeaf};

/// Bucket state managed by this provider.
#[derive(Debug, Clone)]
struct BucketState {
    mmr_root: H256,
    start_seq: u64,
    leaves: Vec<MmrLeaf>,
    /// The actual MMR structure for proof generation
    mmr: Mmr,
    used_bytes: u64,
    max_bytes: u64,
}

impl BucketState {
    fn new(max_bytes: u64) -> Self {
        Self {
            mmr_root: H256::zero(),
            start_seq: 0,
            leaves: Vec::new(),
            mmr: Mmr::new(),
            used_bytes: 0,
            max_bytes,
        }
    }

    fn leaf_count(&self) -> u64 {
        self.leaves.len() as u64
    }
}

/// In-memory storage backend.
pub struct Storage {
    /// Content-addressed node storage: hash -> node
    nodes: DashMap<H256, StoredNode>,
    /// Bucket states
    buckets: RwLock<HashMap<BucketId, BucketState>>,
}

impl Storage {
    /// Create a new storage instance.
    pub fn new() -> Self {
        Self {
            nodes: DashMap::new(),
            buckets: RwLock::new(HashMap::new()),
        }
    }

    /// Initialize a bucket with the given quota.
    pub fn init_bucket(&self, bucket_id: BucketId, max_bytes: u64) {
        let mut buckets = self.buckets.write();
        buckets
            .entry(bucket_id)
            .or_insert_with(|| BucketState::new(max_bytes));
    }

    /// List all buckets.
    pub fn list_buckets(&self) -> Vec<BucketSummary> {
        self.buckets
            .read()
            .iter()
            .map(|(id, state)| BucketSummary {
                bucket_id: *id,
                mmr_root: format!("0x{}", hex::encode(state.mmr_root.as_bytes())),
                start_seq: state.start_seq,
                leaf_count: state.leaf_count(),
            })
            .collect()
    }

    /// Get storage statistics per bucket.
    pub fn get_bucket_stats(&self) -> Vec<BucketStats> {
        self.buckets
            .read()
            .iter()
            .map(|(id, state)| BucketStats {
                bucket_id: *id,
                leaf_count: state.leaf_count(),
                node_count: 0, // Would need to track per-bucket
                bytes_stored: state.used_bytes,
            })
            .collect()
    }

    /// Get total node count.
    pub fn total_nodes(&self) -> u64 {
        self.nodes.len() as u64
    }

    /// Get total bytes stored across all nodes.
    pub fn total_bytes(&self) -> u64 {
        self.nodes.iter().map(|n| n.data.len() as u64).sum()
    }

    /// Store a node (chunk or internal node).
    pub fn store_node(
        &self,
        bucket_id: BucketId,
        expected_hash: H256,
        data: Vec<u8>,
        children: Option<Vec<H256>>,
    ) -> Result<(), Error> {
        // Verify hash
        let actual_hash = blake2_256(&data);
        if actual_hash != expected_hash {
            return Err(Error::InvalidHash {
                expected: format!("0x{}", hex::encode(expected_hash.as_bytes())),
                actual: format!("0x{}", hex::encode(actual_hash.as_bytes())),
            });
        }

        // If internal node, verify children exist (skip H256::zero() used for Merkle tree padding)
        if let Some(ref child_hashes) = children {
            let missing: Vec<String> = child_hashes
                .iter()
                .filter(|h| **h != H256::zero() && !self.nodes.contains_key(*h))
                .map(|h| format!("0x{}", hex::encode(h.as_bytes())))
                .collect();

            if !missing.is_empty() {
                return Err(Error::ChildrenMissing(missing));
            }
        }

        // Check quota
        {
            let buckets = self.buckets.read();
            let bucket = buckets
                .get(&bucket_id)
                .ok_or(Error::BucketNotFound(bucket_id))?;

            let new_size = bucket.used_bytes.saturating_add(data.len() as u64);
            if new_size > bucket.max_bytes {
                return Err(Error::QuotaExceeded {
                    used: bucket.used_bytes,
                    max: bucket.max_bytes,
                });
            }
        }

        // Store node if not already present
        if !self.nodes.contains_key(&expected_hash) {
            let data_len = data.len() as u64;

            self.nodes
                .insert(expected_hash, StoredNode { data, children });

            // Update quota
            let mut buckets = self.buckets.write();
            if let Some(bucket) = buckets.get_mut(&bucket_id) {
                bucket.used_bytes = bucket.used_bytes.saturating_add(data_len);
            }
        }

        Ok(())
    }

    /// Get a node by hash.
    pub fn get_node(&self, hash: &H256) -> Option<StoredNode> {
        self.nodes.get(hash).map(|n| n.clone())
    }

    /// Check which hashes exist.
    pub fn check_exists(&self, _bucket_id: BucketId, hashes: &[H256]) -> (Vec<H256>, Vec<H256>) {
        let mut exists = Vec::new();
        let mut missing = Vec::new();

        for hash in hashes {
            if self.nodes.contains_key(hash) {
                exists.push(*hash);
            } else {
                missing.push(*hash);
            }
        }

        (exists, missing)
    }

    /// Commit data roots to the bucket's MMR.
    pub fn commit(
        &self,
        bucket_id: BucketId,
        data_roots: Vec<H256>,
    ) -> Result<(H256, u64, Vec<u64>), Error> {
        // Verify all roots exist
        for root in &data_roots {
            if !self.nodes.contains_key(root) {
                return Err(Error::RootNotFound(format!(
                    "0x{}",
                    hex::encode(root.as_bytes())
                )));
            }
        }

        let mut buckets = self.buckets.write();
        let bucket = buckets
            .get_mut(&bucket_id)
            .ok_or(Error::BucketNotFound(bucket_id))?;

        let start_index = bucket.leaves.len() as u64;
        let mut leaf_indices = Vec::new();

        for (i, root) in data_roots.iter().enumerate() {
            let leaf_index = start_index + i as u64;
            leaf_indices.push(leaf_index);

            // Calculate data size from the tree
            let data_size = self.calculate_tree_size(*root);
            let total_size = bucket
                .leaves
                .last()
                .map(|l| l.total_size)
                .unwrap_or(0)
                .saturating_add(data_size);

            let leaf = MmrLeaf {
                data_root: *root,
                data_size,
                total_size,
            };

            // Push the SCALE-encoded leaf hash into the MMR (matches pallet's verify_mmr_proof)
            bucket.mmr.push(blake2_256(&leaf.encode()));
            bucket.leaves.push(leaf);
        }

        // Derive MMR root from the actual MMR structure
        bucket.mmr_root = bucket.mmr.root();

        Ok((bucket.mmr_root, bucket.start_seq, leaf_indices))
    }

    /// Get MMR proof for a leaf, suitable for on-chain verification.
    pub fn get_mmr_proof(
        &self,
        bucket_id: BucketId,
        leaf_index: u64,
    ) -> Result<storage_primitives::MmrProof, Error> {
        let buckets = self.buckets.read();
        let bucket = buckets
            .get(&bucket_id)
            .ok_or(Error::BucketNotFound(bucket_id))?;

        let leaf = bucket
            .leaves
            .get(leaf_index as usize)
            .ok_or(Error::NodeNotFound(format!("leaf_{leaf_index}")))?
            .clone();

        let (siblings, path, peaks) = bucket
            .mmr
            .proof_with_path(leaf_index)
            .ok_or(Error::NodeNotFound(format!("mmr_proof_{leaf_index}")))?;

        Ok(storage_primitives::MmrProof {
            peaks,
            leaf,
            leaf_proof: storage_primitives::MerkleProof { siblings, path },
        })
    }

    /// Delete data before a sequence number.
    pub fn delete_before(
        &self,
        bucket_id: BucketId,
        new_start_seq: u64,
    ) -> Result<(H256, u64, u64), Error> {
        let mut buckets = self.buckets.write();
        let bucket = buckets
            .get_mut(&bucket_id)
            .ok_or(Error::BucketNotFound(bucket_id))?;

        // Remove leaves before new_start_seq
        let remove_count = (new_start_seq - bucket.start_seq) as usize;
        if remove_count > 0 && remove_count <= bucket.leaves.len() {
            bucket.leaves.drain(0..remove_count);
            bucket.start_seq = new_start_seq;
        }

        // Rebuild the MMR from remaining leaves
        bucket.mmr = Mmr::new();
        for leaf in &bucket.leaves {
            bucket.mmr.push(blake2_256(&leaf.encode()));
        }
        bucket.mmr_root = bucket.mmr.root();

        Ok((bucket.mmr_root, bucket.start_seq, bucket.leaf_count()))
    }

    /// Get MMR peaks.
    pub fn get_mmr_peaks(&self, bucket_id: BucketId) -> Result<(H256, Vec<H256>), Error> {
        let buckets = self.buckets.read();
        let bucket = buckets
            .get(&bucket_id)
            .ok_or(Error::BucketNotFound(bucket_id))?;

        Ok((bucket.mmr_root, bucket.mmr.peaks()))
    }
}

impl Default for Storage {
    fn default() -> Self {
        Self::new()
    }
}

impl StorageBackend for Storage {
    fn init_bucket(&self, bucket_id: BucketId, max_bytes: u64) -> Result<(), Error> {
        self.init_bucket(bucket_id, max_bytes);
        Ok(())
    }

    fn get_bucket(&self, bucket_id: BucketId) -> Option<BucketInfo> {
        let buckets = self.buckets.read();
        buckets.get(&bucket_id).map(|state| BucketInfo {
            mmr_root: state.mmr_root,
            start_seq: state.start_seq,
            leaf_count: state.leaf_count(),
        })
    }

    fn list_buckets(&self) -> Vec<BucketSummary> {
        self.list_buckets()
    }

    fn get_bucket_stats(&self) -> Vec<BucketStats> {
        self.get_bucket_stats()
    }

    fn total_nodes(&self) -> u64 {
        self.total_nodes()
    }

    fn total_bytes(&self) -> u64 {
        self.total_bytes()
    }

    fn store_node(
        &self,
        bucket_id: BucketId,
        expected_hash: H256,
        data: Vec<u8>,
        children: Option<Vec<H256>>,
    ) -> Result<(), Error> {
        self.store_node(bucket_id, expected_hash, data, children)
    }

    fn get_node(&self, hash: &H256) -> Option<StoredNode> {
        self.get_node(hash)
    }

    fn check_exists(&self, bucket_id: BucketId, hashes: &[H256]) -> (Vec<H256>, Vec<H256>) {
        self.check_exists(bucket_id, hashes)
    }

    fn commit(
        &self,
        bucket_id: BucketId,
        data_roots: Vec<H256>,
    ) -> Result<(H256, u64, Vec<u64>), Error> {
        self.commit(bucket_id, data_roots)
    }

    fn delete_before(
        &self,
        bucket_id: BucketId,
        new_start_seq: u64,
    ) -> Result<(H256, u64, u64), Error> {
        self.delete_before(bucket_id, new_start_seq)
    }

    fn get_mmr_proof(
        &self,
        bucket_id: BucketId,
        leaf_index: u64,
    ) -> Result<storage_primitives::MmrProof, Error> {
        self.get_mmr_proof(bucket_id, leaf_index)
    }

    fn get_mmr_peaks(&self, bucket_id: BucketId) -> Result<(H256, Vec<H256>), Error> {
        self.get_mmr_peaks(bucket_id)
    }

    fn get_data_roots_from(&self, bucket_id: BucketId, from_leaf: u64) -> Result<Vec<H256>, Error> {
        let buckets = self.buckets.read();
        let bucket = buckets
            .get(&bucket_id)
            .ok_or(Error::BucketNotFound(bucket_id))?;
        Ok(bucket
            .leaves
            .iter()
            .skip(from_leaf as usize)
            .map(|l| l.data_root)
            .collect())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use storage_primitives::{hash_children, verify_merkle_proof, verify_mmr_proof};

    /// Helper: create a storage, upload chunks, build a padded Merkle tree, and commit.
    fn setup_bucket_with_chunks(chunk_data: &[&[u8]]) -> (Storage, BucketId, H256, H256) {
        let storage = Storage::new();
        let bucket_id: BucketId = 1;
        storage.init_bucket(bucket_id, u64::MAX);

        // Upload leaf chunks
        let chunk_hashes: Vec<H256> = chunk_data
            .iter()
            .map(|data| {
                let hash = blake2_256(data);
                storage
                    .store_node(bucket_id, hash, data.to_vec(), None)
                    .unwrap();
                hash
            })
            .collect();

        // Build padded Merkle tree (same algorithm as client)
        let data_root = build_padded_merkle_tree(&storage, bucket_id, &chunk_hashes);

        // Commit to MMR
        let (mmr_root, _, _) = storage.commit(bucket_id, vec![data_root]).unwrap();

        (storage, bucket_id, data_root, mmr_root)
    }

    /// Build a balanced Merkle tree with power-of-2 padding (mirrors client logic).
    fn build_padded_merkle_tree(storage: &Storage, bucket_id: BucketId, leaves: &[H256]) -> H256 {
        if leaves.is_empty() {
            return H256::zero();
        }
        if leaves.len() == 1 {
            return leaves[0];
        }

        let padded_len = leaves.len().next_power_of_two();
        let mut current_level = leaves.to_vec();
        current_level.resize(padded_len, H256::zero());

        while current_level.len() > 1 {
            let mut next_level = Vec::new();
            for pair in current_level.chunks(2) {
                let parent = hash_children(pair[0], pair[1]);
                let mut node_data = Vec::new();
                node_data.extend_from_slice(pair[0].as_bytes());
                node_data.extend_from_slice(pair[1].as_bytes());
                // Ignore errors for nodes that may already exist
                let _ =
                    storage.store_node(bucket_id, parent, node_data, Some(vec![pair[0], pair[1]]));
                next_level.push(parent);
            }
            current_level = next_level;
        }

        current_level[0]
    }

    #[test]
    fn test_merkle_proof_single_chunk() {
        let data = b"hello world";
        let (storage, _, data_root, _) = setup_bucket_with_chunks(&[data]);

        // Single chunk: data_root IS the chunk hash, proof is empty
        let (chunk_data, proof) = storage.get_chunk_at_index(data_root, 0).unwrap();
        assert_eq!(chunk_data, data);

        let chunk_hash = blake2_256(&chunk_data);
        assert!(verify_merkle_proof(chunk_hash, 0, &proof, &data_root));
    }

    #[test]
    fn test_merkle_proof_two_chunks() {
        let chunks: Vec<&[u8]> = vec![b"chunk0", b"chunk1"];
        let (storage, _, data_root, _) = setup_bucket_with_chunks(&chunks);

        for i in 0..2 {
            let (chunk_data, proof) = storage.get_chunk_at_index(data_root, i).unwrap();
            assert_eq!(chunk_data, chunks[i as usize]);

            let chunk_hash = blake2_256(&chunk_data);
            assert!(
                verify_merkle_proof(chunk_hash, i, &proof, &data_root),
                "Merkle proof failed for chunk {i}"
            );
        }
    }

    #[test]
    fn test_merkle_proof_three_chunks() {
        let chunks: Vec<&[u8]> = vec![b"chunk0", b"chunk1", b"chunk2"];
        let (storage, _, data_root, _) = setup_bucket_with_chunks(&chunks);

        for i in 0..3 {
            let (chunk_data, proof) = storage.get_chunk_at_index(data_root, i).unwrap();
            assert_eq!(chunk_data, chunks[i as usize]);

            let chunk_hash = blake2_256(&chunk_data);
            assert!(
                verify_merkle_proof(chunk_hash, i, &proof, &data_root),
                "Merkle proof failed for chunk {} (siblings={}, path={:?})",
                i,
                proof.siblings.len(),
                proof.path
            );
        }
    }

    #[test]
    fn test_merkle_proof_five_chunks() {
        let chunks: Vec<&[u8]> = vec![b"a", b"b", b"c", b"d", b"e"];
        let (storage, _, data_root, _) = setup_bucket_with_chunks(&chunks);

        for i in 0..5 {
            let (chunk_data, proof) = storage.get_chunk_at_index(data_root, i).unwrap();
            assert_eq!(chunk_data, chunks[i as usize]);

            let chunk_hash = blake2_256(&chunk_data);
            assert!(
                verify_merkle_proof(chunk_hash, i, &proof, &data_root),
                "Merkle proof failed for chunk {i}"
            );
        }
    }

    #[test]
    fn test_mmr_proof_single_leaf() {
        let data = b"hello world";
        let (storage, bucket_id, _, mmr_root) = setup_bucket_with_chunks(&[data]);

        let mmr_proof = storage.get_mmr_proof(bucket_id, 0).unwrap();
        assert!(
            verify_mmr_proof(&mmr_proof, &mmr_root),
            "MMR proof failed for single leaf"
        );
    }

    #[test]
    fn test_mmr_proof_multiple_leaves() {
        let storage = Storage::new();
        let bucket_id: BucketId = 1;
        storage.init_bucket(bucket_id, u64::MAX);

        // Commit several data roots
        let mut data_roots = Vec::new();
        for i in 0..5 {
            let data = format!("data_{i}");
            let hash = blake2_256(data.as_bytes());
            storage
                .store_node(bucket_id, hash, data.into_bytes(), None)
                .unwrap();
            data_roots.push(hash);
        }

        let (mmr_root, _, _) = storage.commit(bucket_id, data_roots).unwrap();

        // Verify MMR proof for each leaf
        for i in 0..5u64 {
            let mmr_proof = storage.get_mmr_proof(bucket_id, i).unwrap();
            assert!(
                verify_mmr_proof(&mmr_proof, &mmr_root),
                "MMR proof failed for leaf {i}"
            );
        }
    }

    #[test]
    fn test_full_challenge_proof_flow() {
        // Simulate the full challenge flow: upload → commit → generate both proofs → verify
        let chunks: Vec<&[u8]> = vec![b"chunk_a", b"chunk_b", b"chunk_c"];
        let (storage, bucket_id, data_root, mmr_root) = setup_bucket_with_chunks(&chunks);

        let leaf_index = 0u64;
        let chunk_index = 1u64; // Challenge chunk_b

        // Generate MMR proof
        let mmr_proof = storage.get_mmr_proof(bucket_id, leaf_index).unwrap();
        assert_eq!(mmr_proof.leaf.data_root, data_root);

        // Generate chunk proof
        let (chunk_data, chunk_proof) = storage.get_chunk_at_index(data_root, chunk_index).unwrap();
        assert_eq!(chunk_data, b"chunk_b");

        // Verify both proofs (same checks as pallet)
        let chunk_hash = blake2_256(&chunk_data);
        assert!(
            verify_merkle_proof(
                chunk_hash,
                chunk_index,
                &chunk_proof,
                &mmr_proof.leaf.data_root
            ),
            "Chunk Merkle proof failed"
        );
        assert!(verify_mmr_proof(&mmr_proof, &mmr_root), "MMR proof failed");
    }

    #[test]
    fn test_offer_want_collect_all_node_hashes() {
        // A leaf with 4 chunks has a 3-level tree: 4 chunks + 3 internal = 7 nodes.
        let (storage, bucket_id, data_root, _) =
            setup_bucket_with_chunks(&[b"a", b"b", b"c", b"d"]);

        let all = storage.collect_all_node_hashes(data_root);
        // data_root itself + 2 mid-level parents + 4 chunks = 7
        assert_eq!(all.len(), 7, "expected 7 nodes (4 chunks + 3 internal)");
        assert!(all.contains(&data_root), "root must be in the set");

        // Every hash in the set is actually retrievable
        for h in &all {
            assert!(storage.get_node(h).is_some(), "node {h:?} must exist");
        }

        // Identical state → all hashes already present → want = empty
        let (_, want) = storage.check_exists(bucket_id, &all);
        assert!(want.is_empty(), "no gaps when the requester has everything");
    }

    #[test]
    fn test_get_data_roots_from() {
        let storage = Storage::new();
        let bucket_id: BucketId = 1;
        storage.init_bucket(bucket_id, u64::MAX);

        // Commit 3 single-chunk leaves
        let roots: Vec<H256> = (0..3u8)
            .map(|i| {
                let data = vec![i; 8];
                let hash = blake2_256(&data);
                storage.store_node(bucket_id, hash, data, None).unwrap();
                hash
            })
            .collect();
        storage.commit(bucket_id, roots.clone()).unwrap();

        // from_leaf=0 → all 3
        let all = storage.get_data_roots_from(bucket_id, 0).unwrap();
        assert_eq!(all, roots);

        // from_leaf=2 → only the last one
        let tail = storage.get_data_roots_from(bucket_id, 2).unwrap();
        assert_eq!(tail, vec![roots[2]]);

        // from_leaf=3 → empty (no new leaves)
        let empty = storage.get_data_roots_from(bucket_id, 3).unwrap();
        assert!(empty.is_empty());
    }
}
