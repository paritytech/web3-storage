// SPDX-License-Identifier: Apache-2.0

//! Merkle Mountain Range over the data roots of a bucket.
//!
//! Positions are assigned sequentially as nodes are added. Leaf positions
//! follow the formula `leaf_pos(k) = 2*k - popcount(k)` (0-indexed).
//! After inserting n leaves, the number of parent merges equals
//! `n.trailing_zeros()`.

use crate::{blake2_256, hash_children, hash_leaf, MerkleProof};
use alloc::vec::Vec;
use codec::{Decode, DecodeWithMemTracking, Encode, MaxEncodedLen};
use scale_info::TypeInfo;
use sp_core::H256;

/// MMR leaf containing data root and size information.
#[derive(
    Clone, PartialEq, Eq, Encode, Decode, DecodeWithMemTracking, TypeInfo, MaxEncodedLen, Debug,
)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct MmrLeaf {
    /// Merkle root of chunk tree
    pub data_root: H256,
    /// Size of content under this data_root
    pub data_size: u64,
    /// Cumulative unique bytes in MMR at this point
    pub total_size: u64,
}

impl MmrLeaf {
    /// The hash this leaf has in the MMR: `hash_leaf` of its SCALE encoding.
    pub fn hash(&self) -> H256 {
        hash_leaf(&self.encode())
    }
}

/// MMR proof for verifying leaf inclusion.
#[derive(Clone, PartialEq, Eq, Encode, Decode, DecodeWithMemTracking, TypeInfo, Debug)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct MmrProof {
    /// Peaks of the MMR
    pub peaks: Vec<H256>,
    /// The leaf being proven
    pub leaf: MmrLeaf,
    /// Proof from leaf to peak
    pub leaf_proof: MerkleProof,
}

/// Prefix of the preimage that combines two MMR peaks.
const PEAK_PREFIX: u8 = 0x02;

/// Combine MMR peaks into the MMR root.
///
/// Peaks fold from right to left with `blake2_256(0x02 ++ peak ++ rest)`.
/// One peak is the root itself and no peaks give the zero root.
pub fn bag_peaks(peaks: &[H256]) -> H256 {
    peaks
        .iter()
        .rev()
        .copied()
        .reduce(|rest, peak| {
            let mut data = [0u8; 65];
            data[0] = PEAK_PREFIX;
            data[1..33].copy_from_slice(peak.as_bytes());
            data[33..].copy_from_slice(rest.as_bytes());
            blake2_256(&data)
        })
        .unwrap_or_default()
}

/// `true` if `proof.leaf` is part of the MMR with the given `root`.
///
/// The leaf hash goes up through `proof.leaf_proof` to one of `proof.peaks`,
/// and the peaks bag to `root`. Rejects proofs whose `siblings` and `path`
/// lengths differ.
pub fn verify_mmr_proof(proof: &MmrProof, root: &H256) -> bool {
    if proof.leaf_proof.siblings.len() != proof.leaf_proof.path.len() {
        return false;
    }

    // Hash up from the leaf through the Merkle proof to reach a peak
    let mut current = proof.leaf.hash();
    for (sibling, is_right) in proof.leaf_proof.siblings.iter().zip(&proof.leaf_proof.path) {
        current = if *is_right {
            hash_children(*sibling, current)
        } else {
            hash_children(current, *sibling)
        };
    }

    // Current should be one of the peaks, and the peaks should bag to the root
    proof.peaks.contains(&current) && bag_peaks(&proof.peaks) == *root
}

/// A Merkle Mountain Range for storing bucket data.
#[derive(Debug, Clone, Default)]
pub struct Mmr {
    /// All nodes in the MMR, indexed by position
    nodes: Vec<H256>,
    /// Number of leaves
    leaf_count: u64,
}

impl Mmr {
    /// Create a new empty MMR.
    pub fn new() -> Self {
        Self::default()
    }

    /// Create an MMR by appending `leaf_hashes` in order.
    pub fn from_leaf_hashes(leaf_hashes: impl IntoIterator<Item = H256>) -> Self {
        let mut mmr = Self::new();
        for leaf_hash in leaf_hashes {
            mmr.push(leaf_hash);
        }
        mmr
    }

    /// Get the current root hash (bagged peaks).
    pub fn root(&self) -> H256 {
        bag_peaks(&self.peaks())
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

    /// Proof from the leaf at `leaf_index` up to its peak, and the peaks of the MMR.
    ///
    /// At each level, `path = true` means the current node is the right child
    /// (its sibling is to its left). Together with the leaf, these are the
    /// parts of an [`MmrProof`]. `None` if `leaf_index` is out of range.
    pub fn leaf_proof(&self, leaf_index: u64) -> Option<(MerkleProof, Vec<H256>)> {
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

        Some((MerkleProof { siblings, path }, self.peaks()))
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

#[cfg(test)]
mod tests {
    use super::*;
    use alloc::format;

    fn mmr_leaves(n: u64) -> Vec<MmrLeaf> {
        (0..n)
            .map(|i| MmrLeaf {
                data_root: blake2_256(format!("root{i}").as_bytes()),
                data_size: 100 * (i + 1),
                total_size: 100 * (i + 1),
            })
            .collect()
    }

    fn mmr_of(leaves: &[MmrLeaf]) -> Mmr {
        Mmr::from_leaf_hashes(leaves.iter().map(MmrLeaf::hash))
    }

    #[test]
    fn test_mmr_basic() {
        let mut mmr = Mmr::new();
        assert_eq!(mmr.root(), H256::zero());

        for i in 1..=3 {
            mmr.push(blake2_256(format!("leaf{i}").as_bytes()));
            assert_eq!(mmr.leaf_count(), i);
        }

        assert_ne!(mmr.root(), H256::zero());
    }

    #[test]
    fn test_mmr_node_count() {
        // Verify total nodes = 2*n - popcount(n)
        let mut mmr = Mmr::new();
        for i in 1u64..=8 {
            mmr.push(blake2_256(format!("leaf{i}").as_bytes()));
            let expected_nodes = 2 * i - i.count_ones() as u64;
            assert_eq!(
                mmr.nodes.len() as u64,
                expected_nodes,
                "node count wrong after {i} leaves"
            );
        }
    }

    #[test]
    fn from_leaf_hashes_matches_pushes() {
        let hashes: Vec<H256> = (0..6)
            .map(|i| blake2_256(format!("leaf{i}").as_bytes()))
            .collect();
        let mut pushed = Mmr::new();
        for hash in &hashes {
            pushed.push(*hash);
        }
        let built = Mmr::from_leaf_hashes(hashes);
        assert_eq!(built.root(), pushed.root());
        assert_eq!(built.peaks(), pushed.peaks());
        assert_eq!(built.leaf_count(), 6);
    }

    #[test]
    fn proofs_verify_for_every_leaf() {
        for n in 1..=9 {
            let leaves = mmr_leaves(n);
            let mmr = mmr_of(&leaves);
            let root = mmr.root();

            for (i, leaf) in leaves.iter().enumerate() {
                let (leaf_proof, peaks) = mmr.leaf_proof(i as u64).expect("proof should exist");
                assert_eq!(leaf_proof.siblings.len(), leaf_proof.path.len());
                let proof = MmrProof {
                    peaks,
                    leaf: leaf.clone(),
                    leaf_proof,
                };
                assert!(verify_mmr_proof(&proof, &root), "{n} leaves, leaf {i}");
            }
        }
    }

    #[test]
    fn leaf_proof_out_of_range_is_none() {
        let mmr = mmr_of(&mmr_leaves(3));
        assert!(mmr.leaf_proof(3).is_none());
        assert!(Mmr::new().leaf_proof(0).is_none());
    }

    #[test]
    fn verify_rejects_wrong_leaf_root_and_peaks() {
        let leaves = mmr_leaves(5);
        let mmr = mmr_of(&leaves);
        let root = mmr.root();
        let (leaf_proof, peaks) = mmr.leaf_proof(2).unwrap();
        let proof = MmrProof {
            peaks,
            leaf: leaves[2].clone(),
            leaf_proof,
        };
        assert!(verify_mmr_proof(&proof, &root));

        let mut wrong_leaf = proof.clone();
        wrong_leaf.leaf = leaves[3].clone();
        assert!(!verify_mmr_proof(&wrong_leaf, &root));

        assert!(!verify_mmr_proof(&proof, &H256::from_low_u64_be(9)));

        let mut wrong_peaks = proof.clone();
        wrong_peaks.peaks[0] = H256::from_low_u64_be(9);
        assert!(!verify_mmr_proof(&wrong_peaks, &root));
    }

    #[test]
    fn verify_rejects_siblings_and_path_of_different_length() {
        let leaves = mmr_leaves(5);
        let mmr = mmr_of(&leaves);
        let (leaf_proof, peaks) = mmr.leaf_proof(0).unwrap();
        assert!(!leaf_proof.siblings.is_empty());

        let mut proof = MmrProof {
            peaks,
            leaf: leaves[0].clone(),
            leaf_proof,
        };
        assert!(verify_mmr_proof(&proof, &mmr.root()));

        proof.leaf_proof.path.pop();
        assert!(!verify_mmr_proof(&proof, &mmr.root()));

        proof.leaf_proof.path.push(false);
        proof.leaf_proof.siblings.push(H256::zero());
        assert!(!verify_mmr_proof(&proof, &mmr.root()));
    }
}
