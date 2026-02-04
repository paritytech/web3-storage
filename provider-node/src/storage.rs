//! Storage backend for the provider node.
//!
//! This provides a simple in-memory storage implementation.
//! Production implementations would use disk-based storage.

use crate::error::Error;
use crate::types::*;
use dashmap::DashMap;
use parking_lot::RwLock;
use sp_core::H256;
use std::collections::HashMap;
use storage_primitives::{blake2_256, BucketId, MmrLeaf};

/// A stored node (chunk or internal node).
#[derive(Debug, Clone)]
pub struct StoredNode {
    /// The raw data
    pub data: Vec<u8>,
    /// Child hashes for internal nodes
    pub children: Option<Vec<H256>>,
}

/// Bucket state managed by this provider.
#[derive(Debug, Clone)]
pub struct BucketState {
    /// Current MMR root
    pub mmr_root: H256,
    /// Start sequence number
    pub start_seq: u64,
    /// MMR leaves
    pub leaves: Vec<MmrLeaf>,
    /// Quota used in bytes
    pub used_bytes: u64,
    /// Maximum quota for this bucket
    pub max_bytes: u64,
}

impl BucketState {
    pub fn new(max_bytes: u64) -> Self {
        Self {
            mmr_root: H256::zero(),
            start_seq: 0,
            leaves: Vec::new(),
            used_bytes: 0,
            max_bytes,
        }
    }

    pub fn leaf_count(&self) -> u64 {
        self.leaves.len() as u64
    }
}

/// In-memory storage backend.
pub struct Storage {
    /// Content-addressed node storage: hash -> node
    nodes: DashMap<H256, StoredNode>,
    /// Bucket states
    buckets: RwLock<HashMap<BucketId, BucketState>>,
    /// Mapping from data_root to bucket_id for lookups
    root_to_bucket: DashMap<H256, BucketId>,
}

impl Storage {
    /// Create a new storage instance.
    pub fn new() -> Self {
        Self {
            nodes: DashMap::new(),
            buckets: RwLock::new(HashMap::new()),
            root_to_bucket: DashMap::new(),
        }
    }

    /// Initialize a bucket with the given quota.
    pub fn init_bucket(&self, bucket_id: BucketId, max_bytes: u64) {
        let mut buckets = self.buckets.write();
        buckets
            .entry(bucket_id)
            .or_insert_with(|| BucketState::new(max_bytes));
    }

    /// Get bucket state.
    pub fn get_bucket(&self, bucket_id: BucketId) -> Option<BucketState> {
        self.buckets.read().get(&bucket_id).cloned()
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

        // If internal node, verify children exist
        if let Some(ref child_hashes) = children {
            let missing: Vec<String> = child_hashes
                .iter()
                .filter(|h| !self.nodes.contains_key(*h))
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

            bucket.leaves.push(leaf);

            // Track root -> bucket mapping
            self.root_to_bucket.insert(*root, bucket_id);
        }

        // Recalculate MMR root
        bucket.mmr_root = self.calculate_mmr_root(&bucket.leaves);

        Ok((bucket.mmr_root, bucket.start_seq, leaf_indices))
    }

    /// Calculate the size of a content tree.
    fn calculate_tree_size(&self, root: H256) -> u64 {
        let mut size = 0u64;
        let mut stack = vec![root];

        while let Some(hash) = stack.pop() {
            if let Some(node) = self.nodes.get(&hash) {
                if let Some(ref children) = node.children {
                    // Internal node - traverse children
                    stack.extend(children.iter().cloned());
                } else {
                    // Leaf chunk - add size
                    size = size.saturating_add(node.data.len() as u64);
                }
            }
        }

        size
    }

    /// Calculate MMR root from leaves (simplified).
    fn calculate_mmr_root(&self, leaves: &[MmrLeaf]) -> H256 {
        if leaves.is_empty() {
            return H256::zero();
        }

        // Simplified: hash all leaves together
        // Real implementation would build proper MMR structure
        let mut data = Vec::new();
        for leaf in leaves {
            data.extend_from_slice(leaf.data_root.as_bytes());
            data.extend_from_slice(&leaf.data_size.to_le_bytes());
            data.extend_from_slice(&leaf.total_size.to_le_bytes());
        }

        blake2_256(&data)
    }

    /// Get MMR proof for a leaf (simplified).
    pub fn get_mmr_proof(
        &self,
        bucket_id: BucketId,
        leaf_index: u64,
    ) -> Result<(MmrLeaf, Vec<H256>), Error> {
        let buckets = self.buckets.read();
        let bucket = buckets
            .get(&bucket_id)
            .ok_or(Error::BucketNotFound(bucket_id))?;

        let leaf = bucket
            .leaves
            .get(leaf_index as usize)
            .ok_or(Error::NodeNotFound(format!("leaf_{}", leaf_index)))?
            .clone();

        // Simplified proof (real implementation would compute actual MMR proof)
        let proof = vec![bucket.mmr_root];

        Ok((leaf, proof))
    }

    /// Get chunk at index from a data root.
    pub fn get_chunk_at_index(
        &self,
        data_root: H256,
        chunk_index: u64,
    ) -> Result<(Vec<u8>, Vec<H256>), Error> {
        let node = self.nodes.get(&data_root).ok_or_else(|| {
            Error::RootNotFound(format!("0x{}", hex::encode(data_root.as_bytes())))
        })?;

        // For simplicity, traverse to find the chunk
        // Real implementation would have proper indexing
        let chunks = self.collect_chunks(data_root);

        let chunk = chunks
            .get(chunk_index as usize)
            .ok_or_else(|| Error::NodeNotFound(format!("chunk_{}", chunk_index)))?
            .clone();

        // Simplified proof
        let proof = vec![data_root];

        Ok((chunk, proof))
    }

    /// Collect all leaf chunks under a root.
    fn collect_chunks(&self, root: H256) -> Vec<Vec<u8>> {
        let mut chunks = Vec::new();
        let mut stack = vec![root];

        while let Some(hash) = stack.pop() {
            if let Some(node) = self.nodes.get(&hash) {
                if let Some(ref children) = node.children {
                    // Internal node - push children in reverse order
                    for child in children.iter().rev() {
                        stack.push(*child);
                    }
                } else {
                    // Leaf chunk
                    chunks.push(node.data.clone());
                }
            }
        }

        chunks
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

        // Recalculate MMR root
        bucket.mmr_root = self.calculate_mmr_root(&bucket.leaves);

        Ok((bucket.mmr_root, bucket.start_seq, bucket.leaf_count()))
    }

    /// Get MMR peaks.
    pub fn get_mmr_peaks(&self, bucket_id: BucketId) -> Result<(H256, Vec<H256>), Error> {
        let buckets = self.buckets.read();
        let bucket = buckets
            .get(&bucket_id)
            .ok_or(Error::BucketNotFound(bucket_id))?;

        // Simplified: return root as only peak
        // Real implementation would compute actual MMR peaks
        Ok((bucket.mmr_root, vec![bucket.mmr_root]))
    }
}

impl Default for Storage {
    fn default() -> Self {
        Self::new()
    }
}

/// Hex encoding utility (simple implementation).
mod hex {
    pub fn encode(bytes: &[u8]) -> String {
        bytes.iter().map(|b| format!("{:02x}", b)).collect()
    }

    pub fn decode(s: &str) -> Result<Vec<u8>, &'static str> {
        let s = s.strip_prefix("0x").unwrap_or(s);
        if s.len() % 2 != 0 {
            return Err("invalid hex length");
        }

        (0..s.len())
            .step_by(2)
            .map(|i| u8::from_str_radix(&s[i..i + 2], 16).map_err(|_| "invalid hex"))
            .collect()
    }
}

pub use hex::{decode as hex_decode, encode as hex_encode};
