//! Merkle Mountain Range implementation.
//!
//! An MMR is an append-only data structure consisting of multiple perfect
//! binary trees (peaks). When a new leaf is added, peaks of the same height
//! are merged until no two peaks have the same height.
//!
//! The root is computed by "bagging" the peaks from right to left.

use sp_core::H256;
use storage_primitives::hash_children;

/// A Merkle Mountain Range for storing bucket data.
#[derive(Debug, Clone)]
pub struct Mmr {
    /// All nodes in the MMR, indexed by position
    nodes: Vec<H256>,
    /// Number of leaves
    leaf_count: u64,
    /// Current peaks (one per set bit in leaf_count), stored as (height, position, hash)
    peaks: Vec<(u32, u64, H256)>,
}

impl Mmr {
    /// Create a new empty MMR.
    pub fn new() -> Self {
        Self {
            nodes: Vec::new(),
            leaf_count: 0,
            peaks: Vec::new(),
        }
    }

    /// Get the current root hash.
    pub fn root(&self) -> H256 {
        if self.peaks.is_empty() {
            return H256::zero();
        }

        // Bag the peaks from right to left
        self.peaks
            .iter()
            .rev()
            .map(|(_, _, hash)| *hash)
            .fold(None, |acc: Option<H256>, peak| {
                Some(match acc {
                    None => peak,
                    Some(right) => hash_children(peak, right),
                })
            })
            .unwrap_or(H256::zero())
    }

    /// Get the peak hashes of the MMR.
    pub fn peak_hashes(&self) -> Vec<H256> {
        self.peaks.iter().map(|(_, _, hash)| *hash).collect()
    }

    /// Get the peak hashes of the MMR (alias for peak_hashes).
    pub fn peaks(&self) -> Vec<H256> {
        self.peak_hashes()
    }

    /// Append a leaf to the MMR.
    pub fn push(&mut self, leaf_hash: H256) -> u64 {
        let leaf_pos = self.nodes.len() as u64;
        self.nodes.push(leaf_hash);
        self.leaf_count += 1;

        // Add new peak at height 0
        let mut current_height = 0u32;
        let mut current_pos = leaf_pos;
        let mut current_hash = leaf_hash;

        // Merge with existing peaks of the same height
        while !self.peaks.is_empty() {
            let (top_height, _top_pos, top_hash) = self.peaks.last().unwrap();

            if *top_height != current_height {
                break;
            }

            // Merge: left sibling is the existing peak, right is current
            let parent_hash = hash_children(*top_hash, current_hash);
            let parent_pos = self.nodes.len() as u64;
            self.nodes.push(parent_hash);

            // Remove the merged peak and continue with parent
            self.peaks.pop();
            current_height += 1;
            current_pos = parent_pos;
            current_hash = parent_hash;
        }

        // Add the new/merged peak
        self.peaks.push((current_height, current_pos, current_hash));

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

        // Find which peak contains this leaf and build the proof path
        let mut siblings = Vec::new();
        let mut current_leaf_index = leaf_index;
        let mut _leaves_before = 0u64;

        // Find the peak containing this leaf
        for &(height, peak_pos, _) in &self.peaks {
            let peak_leaf_count = 1u64 << height;

            if current_leaf_index < peak_leaf_count {
                // This peak contains our leaf
                // Build proof within this perfect binary tree
                self.build_tree_proof(
                    peak_pos,
                    height,
                    current_leaf_index,
                    &mut siblings,
                );
                break;
            }

            _leaves_before += peak_leaf_count;
            current_leaf_index -= peak_leaf_count;
        }

        Some(MmrProof {
            leaf_index,
            siblings,
            peaks: self.peak_hashes(),
        })
    }

    /// Build a proof path within a perfect binary tree.
    /// Returns siblings from leaf up to the root of the subtree.
    fn build_tree_proof(
        &self,
        tree_root_pos: u64,
        tree_height: u32,
        leaf_index_in_tree: u64,
        siblings: &mut Vec<H256>,
    ) {
        if tree_height == 0 {
            // Single leaf tree, no siblings needed
            return;
        }

        // Calculate positions in the perfect binary tree
        // The tree is stored in post-order: left subtree, right subtree, root
        let left_subtree_size = (1u64 << tree_height) - 1;
        let right_subtree_size = left_subtree_size;

        let left_subtree_root = tree_root_pos - 1 - right_subtree_size;
        let right_subtree_root = tree_root_pos - 1;

        let left_leaf_count = 1u64 << (tree_height - 1);

        if leaf_index_in_tree < left_leaf_count {
            // Leaf is in left subtree
            // Sibling is the right subtree root
            if let Some(sibling) = self.nodes.get(right_subtree_root as usize) {
                siblings.push(*sibling);
            }
            // Recurse into left subtree
            self.build_tree_proof(
                left_subtree_root,
                tree_height - 1,
                leaf_index_in_tree,
                siblings,
            );
        } else {
            // Leaf is in right subtree
            // Sibling is the left subtree root
            if let Some(sibling) = self.nodes.get(left_subtree_root as usize) {
                siblings.push(*sibling);
            }
            // Recurse into right subtree
            self.build_tree_proof(
                right_subtree_root,
                tree_height - 1,
                leaf_index_in_tree - left_leaf_count,
                siblings,
            );
        }
    }

    /// Verify a proof against an MMR root.
    pub fn verify_proof(root: H256, leaf_hash: H256, proof: &MmrProof) -> bool {
        // Hash up from leaf through siblings to reach a peak
        let mut current = leaf_hash;
        let mut pos_in_tree = proof.leaf_index;

        // The siblings are from leaf level up, but we need to process them
        // in reverse order (from closest to leaf to farthest)
        // Actually they're already in the right order: closest sibling first
        for sibling in proof.siblings.iter().rev() {
            // Determine if we're the left or right child
            let is_left = pos_in_tree % 2 == 0;
            current = if is_left {
                hash_children(current, *sibling)
            } else {
                hash_children(*sibling, current)
            };
            pos_in_tree /= 2;
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
    /// Sibling hashes on the path to the peak (from root down to leaf level)
    pub siblings: Vec<H256>,
    /// Peaks of the MMR
    pub peaks: Vec<H256>,
}

#[cfg(test)]
mod tests {
    use super::*;
    use storage_primitives::blake2_256;

    #[test]
    fn test_mmr_basic() {
        let mut mmr = Mmr::new();

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
    fn test_mmr_peaks_count() {
        let mut mmr = Mmr::new();

        // Number of peaks equals number of 1s in binary representation of leaf_count
        mmr.push(blake2_256(b"leaf0"));
        assert_eq!(mmr.peak_hashes().len(), 1); // 1 = 0b1

        mmr.push(blake2_256(b"leaf1"));
        assert_eq!(mmr.peak_hashes().len(), 1); // 2 = 0b10

        mmr.push(blake2_256(b"leaf2"));
        assert_eq!(mmr.peak_hashes().len(), 2); // 3 = 0b11

        mmr.push(blake2_256(b"leaf3"));
        assert_eq!(mmr.peak_hashes().len(), 1); // 4 = 0b100

        mmr.push(blake2_256(b"leaf4"));
        assert_eq!(mmr.peak_hashes().len(), 2); // 5 = 0b101

        mmr.push(blake2_256(b"leaf5"));
        assert_eq!(mmr.peak_hashes().len(), 2); // 6 = 0b110

        mmr.push(blake2_256(b"leaf6"));
        assert_eq!(mmr.peak_hashes().len(), 3); // 7 = 0b111

        mmr.push(blake2_256(b"leaf7"));
        assert_eq!(mmr.peak_hashes().len(), 1); // 8 = 0b1000
    }

    #[test]
    fn test_mmr_root_consistency() {
        let mut mmr = Mmr::new();

        let leaves: Vec<H256> = (0..8)
            .map(|i| blake2_256(format!("leaf{}", i).as_bytes()))
            .collect();

        for leaf in &leaves {
            mmr.push(*leaf);
        }

        // With 8 leaves (power of 2), we should have 1 peak
        assert_eq!(mmr.peak_hashes().len(), 1);

        // The root should equal the single peak
        let peaks = mmr.peak_hashes();
        assert_eq!(mmr.root(), peaks[0]);
    }

    #[test]
    fn test_mmr_proof_single_leaf() {
        let mut mmr = Mmr::new();
        let leaf = blake2_256(b"only_leaf");
        mmr.push(leaf);

        let root = mmr.root();
        let proof = mmr.proof(0).expect("proof should exist");

        // Single leaf: no siblings needed, leaf is the peak
        assert!(proof.siblings.is_empty());
        assert!(Mmr::verify_proof(root, leaf, &proof));
    }

    #[test]
    fn test_mmr_proof_two_leaves() {
        let mut mmr = Mmr::new();
        let leaf0 = blake2_256(b"leaf0");
        let leaf1 = blake2_256(b"leaf1");

        mmr.push(leaf0);
        mmr.push(leaf1);

        let root = mmr.root();

        // Proof for leaf 0
        let proof0 = mmr.proof(0).expect("proof should exist");
        assert!(Mmr::verify_proof(root, leaf0, &proof0), "leaf 0 should verify");

        // Proof for leaf 1
        let proof1 = mmr.proof(1).expect("proof should exist");
        assert!(Mmr::verify_proof(root, leaf1, &proof1), "leaf 1 should verify");
    }

    #[test]
    fn test_mmr_proof_power_of_two() {
        let mut mmr = Mmr::new();

        let leaves: Vec<H256> = (0..4)
            .map(|i| blake2_256(format!("leaf{}", i).as_bytes()))
            .collect();

        for leaf in &leaves {
            mmr.push(*leaf);
        }

        let root = mmr.root();

        for (i, leaf) in leaves.iter().enumerate() {
            let proof = mmr.proof(i as u64).expect("proof should exist");
            assert!(
                Mmr::verify_proof(root, *leaf, &proof),
                "proof should verify for leaf {}",
                i
            );
        }
    }

    #[test]
    fn test_mmr_proof_five_leaves() {
        let mut mmr = Mmr::new();

        let leaves: Vec<H256> = (0..5)
            .map(|i| blake2_256(format!("leaf{}", i).as_bytes()))
            .collect();

        for leaf in &leaves {
            mmr.push(*leaf);
        }

        let root = mmr.root();

        for (i, leaf) in leaves.iter().enumerate() {
            let proof = mmr.proof(i as u64).expect("proof should exist");
            assert!(
                Mmr::verify_proof(root, *leaf, &proof),
                "proof should verify for leaf {}",
                i
            );
        }
    }

    #[test]
    fn test_mmr_invalid_proof() {
        let mut mmr = Mmr::new();

        let leaves: Vec<H256> = (0..4)
            .map(|i| blake2_256(format!("leaf{}", i).as_bytes()))
            .collect();

        for leaf in &leaves {
            mmr.push(*leaf);
        }

        let root = mmr.root();
        let proof = mmr.proof(0).expect("proof should exist");

        // Using wrong leaf should fail
        let wrong_leaf = blake2_256(b"wrong");
        assert!(!Mmr::verify_proof(root, wrong_leaf, &proof));
    }
}
