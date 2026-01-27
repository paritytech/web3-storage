//! Merkle Mountain Range implementation.
//!
//! This is a simplified MMR implementation for the provider node.
//! Production would use a more optimized implementation.

use sp_core::H256;
use storage_primitives::{blake2_256, hash_children};

/// A Merkle Mountain Range for storing bucket data.
#[derive(Debug, Clone)]
pub struct Mmr {
    /// All nodes in the MMR, indexed by position
    nodes: Vec<H256>,
    /// Number of leaves
    leaf_count: u64,
}

impl Mmr {
    /// Create a new empty MMR.
    pub fn new() -> Self {
        Self {
            nodes: Vec::new(),
            leaf_count: 0,
        }
    }

    /// Get the current root hash.
    pub fn root(&self) -> H256 {
        if self.nodes.is_empty() {
            return H256::zero();
        }

        // Bag the peaks
        let peaks = self.peaks();
        if peaks.is_empty() {
            return H256::zero();
        }

        peaks
            .iter()
            .rev()
            .fold(None, |acc: Option<H256>, &peak| {
                Some(match acc {
                    None => peak,
                    Some(right) => hash_children(peak, right),
                })
            })
            .unwrap_or(H256::zero())
    }

    /// Get the peaks of the MMR.
    pub fn peaks(&self) -> Vec<H256> {
        if self.nodes.is_empty() {
            return vec![];
        }

        let mut peaks = Vec::new();
        let mut pos = 0u64;
        let mut height = 0u32;

        while pos < self.nodes.len() as u64 {
            let peak_height = Self::peak_height_at(self.leaf_count, height);
            if peak_height > 0 {
                let peak_size = (1u64 << peak_height) - 1;
                let peak_pos = pos + peak_size - 1;
                if peak_pos < self.nodes.len() as u64 {
                    peaks.push(self.nodes[peak_pos as usize]);
                }
                pos += peak_size;
            }
            height += 1;
            if height > 64 {
                break;
            }
        }

        peaks
    }

    /// Append a leaf to the MMR.
    pub fn push(&mut self, leaf_hash: H256) -> u64 {
        let leaf_pos = self.nodes.len() as u64;
        self.nodes.push(leaf_hash);
        self.leaf_count += 1;

        // Merge with sibling peaks if needed
        let mut pos = leaf_pos;
        let mut current_hash = leaf_hash;
        let mut height = 0u32;

        while Self::has_sibling(pos, height, self.nodes.len() as u64) {
            let sibling_pos = Self::sibling_pos(pos, height);
            let sibling_hash = self.nodes[sibling_pos as usize];

            // Parent is always to the right of the rightmost child
            let parent_hash = if sibling_pos < pos {
                hash_children(sibling_hash, current_hash)
            } else {
                hash_children(current_hash, sibling_hash)
            };

            self.nodes.push(parent_hash);
            current_hash = parent_hash;
            pos = self.nodes.len() as u64 - 1;
            height += 1;
        }

        leaf_pos
    }

    /// Get the number of leaves.
    pub fn leaf_count(&self) -> u64 {
        self.leaf_count
    }

    /// Get a node by position.
    pub fn get(&self, pos: u64) -> Option<H256> {
        self.nodes.get(pos as usize).copied()
    }

    /// Generate a proof for a leaf at the given index.
    pub fn proof(&self, leaf_index: u64) -> Option<MmrProof> {
        if leaf_index >= self.leaf_count {
            return None;
        }

        let leaf_pos = Self::leaf_index_to_pos(leaf_index);
        let mut siblings = Vec::new();
        let mut pos = leaf_pos;
        let mut height = 0u32;

        while Self::has_sibling(pos, height, self.nodes.len() as u64) {
            let sibling_pos = Self::sibling_pos(pos, height);
            if let Some(sibling) = self.nodes.get(sibling_pos as usize) {
                siblings.push(*sibling);
            }
            pos = Self::parent_pos(pos, height);
            height += 1;
        }

        Some(MmrProof {
            leaf_index,
            siblings,
            peaks: self.peaks(),
        })
    }

    /// Verify a proof against an MMR root.
    ///
    /// This verifies that:
    /// 1. The leaf hashes up through siblings to reach a peak
    /// 2. The peaks bag to the expected root
    pub fn verify_proof(root: H256, leaf_hash: H256, proof: &MmrProof) -> bool {
        // Hash up from leaf through siblings to reach a peak
        let mut current = leaf_hash;
        let mut pos = Self::leaf_index_to_pos(proof.leaf_index);
        let mut height = 0u32;

        for sibling in &proof.siblings {
            // Determine if sibling is on left or right based on position
            let sibling_pos = Self::sibling_pos(pos, height);
            current = if sibling_pos < pos {
                hash_children(*sibling, current)
            } else {
                hash_children(current, *sibling)
            };
            pos = Self::parent_pos(pos, height);
            height += 1;
        }

        // Current should now be one of the peaks
        if !proof.peaks.contains(&current) {
            return false;
        }

        // Verify that peaks bag to the root
        let bagged_root = proof
            .peaks
            .iter()
            .rev()
            .fold(None, |acc: Option<H256>, &peak| {
                Some(match acc {
                    None => peak,
                    Some(right) => hash_children(peak, right),
                })
            })
            .unwrap_or(H256::zero());

        bagged_root == root
    }

    // Helper functions

    fn peak_height_at(leaf_count: u64, index: u32) -> u32 {
        let bits = leaf_count;
        if index >= 64 {
            return 0;
        }
        if bits & (1u64 << index) != 0 {
            index + 1
        } else {
            0
        }
    }

    fn has_sibling(pos: u64, height: u32, total_nodes: u64) -> bool {
        let sibling = Self::sibling_pos(pos, height);
        sibling < total_nodes && sibling != pos
    }

    fn sibling_pos(pos: u64, height: u32) -> u64 {
        let offset = 1u64 << height;
        if (pos / offset) % 2 == 0 {
            pos + offset
        } else {
            pos.saturating_sub(offset)
        }
    }

    fn parent_pos(pos: u64, height: u32) -> u64 {
        let offset = 1u64 << height;
        let sibling = Self::sibling_pos(pos, height);
        core::cmp::max(pos, sibling) + 1
    }

    fn leaf_index_to_pos(leaf_index: u64) -> u64 {
        // Simplified: each leaf adds 1 position plus parents
        // This is a rough approximation
        let mut pos = 0u64;
        for i in 0..leaf_index {
            pos += 1;
            let mut height = 0u32;
            let mut idx = i + 1;
            while idx % 2 == 0 {
                pos += 1;
                idx /= 2;
                height += 1;
            }
        }
        pos
    }
}

impl Default for Mmr {
    fn default() -> Self {
        Self::new()
    }
}

/// Proof of inclusion in an MMR.
#[derive(Debug, Clone)]
pub struct MmrProof {
    /// Index of the leaf in the MMR
    pub leaf_index: u64,
    /// Sibling hashes on the path to the peak
    pub siblings: Vec<H256>,
    /// Peaks of the MMR
    pub peaks: Vec<H256>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_mmr_basic() {
        let mut mmr = Mmr::new();

        // Add some leaves
        let leaf1 = blake2_256(b"leaf1");
        let leaf2 = blake2_256(b"leaf2");
        let leaf3 = blake2_256(b"leaf3");

        mmr.push(leaf1);
        assert_eq!(mmr.leaf_count(), 1);

        mmr.push(leaf2);
        assert_eq!(mmr.leaf_count(), 2);

        mmr.push(leaf3);
        assert_eq!(mmr.leaf_count(), 3);

        let root = mmr.root();
        assert_ne!(root, H256::zero());
    }

    #[test]
    fn test_mmr_proof() {
        let mut mmr = Mmr::new();

        let leaves: Vec<H256> = (0..5)
            .map(|i| blake2_256(format!("leaf{}", i).as_bytes()))
            .collect();

        for leaf in &leaves {
            mmr.push(*leaf);
        }

        let root = mmr.root();

        // Generate and verify proof for each leaf
        for (i, leaf) in leaves.iter().enumerate() {
            let proof = mmr.proof(i as u64).expect("proof should exist");
            assert!(
                Mmr::verify_proof(root, *leaf, &proof),
                "proof should verify for leaf {}",
                i
            );
        }
    }
}
