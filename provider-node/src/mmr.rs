//! Merkle Mountain Range implementation.
//!
//! Positions are assigned sequentially as nodes are added. Leaf positions
//! follow the formula `leaf_pos(k) = 2*k - popcount(k)` (0-indexed).
//! After inserting n leaves, the number of parent merges equals
//! `n.trailing_zeros()`.

use sp_core::H256;
use storage_primitives::hash_children;

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

    /// Get the current root hash (bagged peaks).
    pub fn root(&self) -> H256 {
        if self.nodes.is_empty() {
            return H256::zero();
        }

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

    /// Get the peaks of the MMR (left to right, highest to lowest height).
    pub fn peaks(&self) -> Vec<H256> {
        let mut peaks = Vec::new();
        let mut pos = 0u64;
        let mut remaining = self.leaf_count;

        while remaining > 0 {
            let h = 63 - remaining.leading_zeros();
            let subtree_leaves = 1u64 << h;
            let subtree_nodes = (1u64 << (h + 1)) - 1;

            let peak_pos = pos + subtree_nodes - 1;
            peaks.push(self.nodes[peak_pos as usize]);

            pos += subtree_nodes;
            remaining -= subtree_leaves;
        }

        peaks
    }

    /// Append a leaf to the MMR.
    pub fn push(&mut self, leaf_hash: H256) -> u64 {
        let leaf_pos = self.nodes.len() as u64;
        self.nodes.push(leaf_hash);
        self.leaf_count += 1;

        let merges = self.leaf_count.trailing_zeros();
        let mut current_hash = leaf_hash;

        for h in 0..merges {
            let current_pos = self.nodes.len() as u64 - 1;
            let left_sibling_offset = (1u64 << (h + 1)) - 1;
            let left_sibling_pos = current_pos - left_sibling_offset;
            let left_sibling_hash = self.nodes[left_sibling_pos as usize];

            let parent_hash = hash_children(left_sibling_hash, current_hash);
            self.nodes.push(parent_hash);
            current_hash = parent_hash;
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
        let (siblings, path, peaks) = self.proof_with_path(leaf_index)?;
        Some(MmrProof {
            leaf_index,
            siblings,
            path,
            peaks,
        })
    }

    /// Generate a proof with path bits for a leaf at the given index.
    ///
    /// Returns `(siblings, path_bits, peaks)` suitable for constructing
    /// a `storage_primitives::MmrProof`. At each level, `is_right = true`
    /// means the current node is the right child (sibling is to its left).
    pub fn proof_with_path(&self, leaf_index: u64) -> Option<(Vec<H256>, Vec<bool>, Vec<H256>)> {
        if leaf_index >= self.leaf_count {
            return None;
        }

        let (peak_height, local_leaf_index) = self.locate_leaf(leaf_index);
        let leaf_pos = Self::leaf_index_to_pos(leaf_index);

        let mut siblings = Vec::new();
        let mut path = Vec::new();
        let mut pos = leaf_pos;

        for h in 0..peak_height {
            let is_right = (local_leaf_index >> h) & 1 == 1;
            let subtree_size = (1u64 << (h + 1)) - 1;

            let sibling_pos = if is_right {
                pos - subtree_size
            } else {
                pos + subtree_size
            };

            siblings.push(self.nodes[sibling_pos as usize]);
            path.push(is_right);

            // Move to parent
            pos = if is_right {
                pos + 1
            } else {
                pos + subtree_size + 1
            };
        }

        Some((siblings, path, self.peaks()))
    }

    /// Verify a proof against an MMR root.
    pub fn verify_proof(root: H256, leaf_hash: H256, proof: &MmrProof) -> bool {
        if proof.siblings.len() != proof.path.len() {
            return false;
        }

        let mut current = leaf_hash;

        for (sibling, is_right) in proof.siblings.iter().zip(proof.path.iter()) {
            current = if *is_right {
                hash_children(*sibling, current)
            } else {
                hash_children(current, *sibling)
            };
        }

        if !proof.peaks.contains(&current) {
            return false;
        }

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

    /// Determine which peak subtree a leaf belongs to.
    ///
    /// Returns `(peak_height, local_leaf_index)` where peak_height is the
    /// height of the peak's perfect binary subtree and local_leaf_index is
    /// the leaf's 0-based index within that subtree.
    fn locate_leaf(&self, leaf_index: u64) -> (u32, u64) {
        let mut remaining = self.leaf_count;
        let mut leaf_offset = 0u64;

        while remaining > 0 {
            let h = 63 - remaining.leading_zeros();
            let subtree_leaves = 1u64 << h;

            if leaf_index < leaf_offset + subtree_leaves {
                return (h, leaf_index - leaf_offset);
            }

            leaf_offset += subtree_leaves;
            remaining -= subtree_leaves;
        }

        unreachable!("leaf_index should be < leaf_count")
    }

    /// Convert a 0-based leaf index to its position in the nodes array.
    fn leaf_index_to_pos(leaf_index: u64) -> u64 {
        2 * leaf_index - (leaf_index.count_ones() as u64)
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
    /// Path bits (true = current node is right child)
    pub path: Vec<bool>,
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
    fn test_mmr_node_count() {
        // Verify total nodes = 2*n - popcount(n)
        let mut mmr = Mmr::new();
        for i in 1u64..=8 {
            mmr.push(blake2_256(format!("leaf{}", i).as_bytes()));
            let expected_nodes = 2 * i - i.count_ones() as u64;
            assert_eq!(
                mmr.nodes.len() as u64, expected_nodes,
                "node count wrong after {} leaves",
                i
            );
        }
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
    fn test_proof_with_path() {
        let mut mmr = Mmr::new();

        let leaves: Vec<H256> = (0..5)
            .map(|i| blake2_256(format!("leaf{}", i).as_bytes()))
            .collect();

        for leaf in &leaves {
            mmr.push(*leaf);
        }

        let root = mmr.root();

        for (i, leaf) in leaves.iter().enumerate() {
            let (siblings, path, peaks) =
                mmr.proof_with_path(i as u64).expect("proof should exist");

            // Verify via Mmr::verify_proof
            let proof = mmr.proof(i as u64).expect("proof should exist");
            assert!(
                Mmr::verify_proof(root, *leaf, &proof),
                "basic proof should verify for leaf {}",
                i
            );

            assert_eq!(
                siblings.len(),
                path.len(),
                "siblings and path length mismatch for leaf {}",
                i
            );
        }
    }

    #[test]
    fn test_proof_with_path_primitives_verify() {
        use codec::Encode;

        // This test mirrors how the pallet verifies: push blake2_256(&leaf.encode())
        // into MMR, then verify with storage_primitives::verify_mmr_proof
        let mut mmr = Mmr::new();

        let mmr_leaves: Vec<storage_primitives::MmrLeaf> = (0..5)
            .map(|i| storage_primitives::MmrLeaf {
                data_root: blake2_256(format!("root{}", i).as_bytes()),
                data_size: 100 * (i as u64 + 1),
                total_size: 100 * (i as u64 + 1),
            })
            .collect();

        for leaf in &mmr_leaves {
            mmr.push(blake2_256(&leaf.encode()));
        }

        let root = mmr.root();

        for (i, leaf) in mmr_leaves.iter().enumerate() {
            let (siblings, path, peaks) =
                mmr.proof_with_path(i as u64).expect("proof should exist");

            let mmr_proof = storage_primitives::MmrProof {
                peaks,
                leaf: leaf.clone(),
                leaf_proof: storage_primitives::MerkleProof { siblings, path },
            };

            assert!(
                storage_primitives::verify_mmr_proof(&mmr_proof, &root),
                "verify_mmr_proof failed for leaf {}",
                i
            );
        }
    }
}
