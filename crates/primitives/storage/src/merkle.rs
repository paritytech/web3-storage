// SPDX-License-Identifier: Apache-2.0

//! Merkle tree over the chunks of one data root.
//!
//! The tree is the zero-padded balanced binary tree: the leaves are padded with
//! `H256::zero()` up to the next power of two and every internal node is
//! `hash_children(left, right)`. No leaves give `H256::zero()` and one leaf is
//! its own root. The provider builds, stores and proves against this shape, and
//! the pallet verifies proofs against it.

use crate::blake2_256;
use alloc::vec::Vec;
use codec::{Decode, DecodeWithMemTracking, Encode};
use scale_info::TypeInfo;
use sp_core::H256;

/// Merkle proof for verifying inclusion.
#[derive(Clone, PartialEq, Eq, Encode, Decode, DecodeWithMemTracking, TypeInfo, Debug)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct MerkleProof {
    /// Sibling hashes from leaf to root
    pub siblings: Vec<H256>,
    /// Path bits (false = left, true = right)
    pub path: Vec<bool>,
}

/// An internal node of the padded tree.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct MerkleNode {
    /// `hash_children(left, right)`.
    pub hash: H256,
    /// Left child hash.
    pub left: H256,
    /// Right child hash. `H256::zero()` for a padding slot.
    pub right: H256,
}

/// Hash of the two children of an internal node: `blake2_256(left || right)`.
pub fn hash_children(left: H256, right: H256) -> H256 {
    let mut data = [0u8; 64];
    data[..32].copy_from_slice(left.as_bytes());
    data[32..].copy_from_slice(right.as_bytes());
    blake2_256(&data)
}

/// Every level of the padded tree, from the padded leaves (first) to the root
/// (last, one element). Empty for no leaves.
fn padded_levels(leaf_hashes: &[H256]) -> Vec<Vec<H256>> {
    if leaf_hashes.is_empty() {
        return Vec::new();
    }

    let mut level = leaf_hashes.to_vec();
    level.resize(leaf_hashes.len().next_power_of_two(), H256::zero());

    let mut levels = Vec::new();
    while level.len() > 1 {
        let next = level
            .chunks(2)
            .map(|pair| hash_children(pair[0], pair[1]))
            .collect();
        levels.push(core::mem::replace(&mut level, next));
    }
    levels.push(level);
    levels
}

/// Root of the zero-padded balanced binary tree over `leaf_hashes`.
///
/// No leaves give `H256::zero()` and one leaf gives that leaf.
pub fn padded_merkle_root(leaf_hashes: &[H256]) -> H256 {
    padded_levels(leaf_hashes)
        .last()
        .map_or_else(H256::zero, |root| root[0])
}

/// Root and internal nodes of the padded tree over `leaf_hashes`.
///
/// Nodes come children before parents, so a store that requires children to
/// exist can insert them in order. No leaves or one leaf give no nodes.
pub fn padded_merkle_tree(leaf_hashes: &[H256]) -> (H256, Vec<MerkleNode>) {
    let levels = padded_levels(leaf_hashes);
    let nodes = levels
        .windows(2)
        .flat_map(|pair| {
            pair[0]
                .chunks(2)
                .zip(&pair[1])
                .map(|(children, hash)| MerkleNode {
                    hash: *hash,
                    left: children[0],
                    right: children[1],
                })
        })
        .collect();
    let root = levels.last().map_or_else(H256::zero, |root| root[0]);
    (root, nodes)
}

/// Build a Merkle proof for slot `index` of the padded tree over
/// `leaf_hashes`. The proof is empty for a single leaf.
///
/// A padding slot (`leaf_hashes.len() <= index < padded length`) gets the path
/// from its zero leaf. `None` if `index` is past the padded length or there
/// are no leaves.
pub fn build_merkle_proof(leaf_hashes: &[H256], index: usize) -> Option<MerkleProof> {
    if leaf_hashes.is_empty() || index >= leaf_hashes.len().next_power_of_two() {
        return None;
    }

    let levels = padded_levels(leaf_hashes);
    let mut siblings = Vec::new();
    let mut path = Vec::new();
    let mut idx = index;

    // The last level is the root, which has no sibling.
    for level in &levels[..levels.len() - 1] {
        let is_right = idx % 2 == 1;
        siblings.push(level[if is_right { idx - 1 } else { idx + 1 }]);
        path.push(is_right);
        idx /= 2;
    }

    Some(MerkleProof { siblings, path })
}

/// `true` if `proof` links `leaf_hash` at leaf `index` to `root`.
///
/// Rejects proofs whose `siblings` and `path` lengths differ or whose path bits
/// do not match `index`.
pub fn verify_merkle_proof(leaf_hash: H256, index: u64, proof: &MerkleProof, root: &H256) -> bool {
    if proof.siblings.len() != proof.path.len() {
        return false;
    }

    let mut current = leaf_hash;
    let mut current_index = index;

    for (sibling, is_right) in proof.siblings.iter().zip(proof.path.iter()) {
        let expected_right = current_index % 2 == 1;
        if *is_right != expected_right {
            return false;
        }

        current = if *is_right {
            hash_children(*sibling, current)
        } else {
            hash_children(current, *sibling)
        };

        current_index /= 2;
    }

    current == *root
}

#[cfg(test)]
mod tests {
    use super::*;

    fn leaves(n: usize) -> Vec<H256> {
        (1..=n).map(|i| H256::from_low_u64_be(i as u64)).collect()
    }

    #[test]
    fn root_of_empty_and_single_leaf() {
        assert_eq!(padded_merkle_root(&[]), H256::zero());
        let leaf = leaves(1)[0];
        assert_eq!(padded_merkle_root(&[leaf]), leaf);
    }

    #[test]
    fn root_of_two_leaves_hashes_the_pair() {
        let l = leaves(2);
        assert_eq!(padded_merkle_root(&l), hash_children(l[0], l[1]));
    }

    #[test]
    fn root_of_three_leaves_pads_with_zero() {
        let l = leaves(3);
        let expected = hash_children(hash_children(l[0], l[1]), hash_children(l[2], H256::zero()));
        assert_eq!(padded_merkle_root(&l), expected);
    }

    #[test]
    fn tree_root_equals_padded_root() {
        for n in [0, 1, 2, 3, 5, 8] {
            let l = leaves(n);
            assert_eq!(
                padded_merkle_tree(&l).0,
                padded_merkle_root(&l),
                "{n} leaves"
            );
        }
    }

    #[test]
    fn tree_nodes_come_children_before_parents() {
        let l = leaves(3);
        let (root, nodes) = padded_merkle_tree(&l);
        let left = hash_children(l[0], l[1]);
        let right = hash_children(l[2], H256::zero());
        assert_eq!(
            nodes,
            [
                MerkleNode {
                    hash: left,
                    left: l[0],
                    right: l[1]
                },
                MerkleNode {
                    hash: right,
                    left: l[2],
                    right: H256::zero()
                },
                MerkleNode {
                    hash: root,
                    left,
                    right
                },
            ]
        );
    }

    #[test]
    fn tree_of_zero_or_one_leaf_has_no_nodes() {
        assert!(padded_merkle_tree(&[]).1.is_empty());
        assert!(padded_merkle_tree(&leaves(1)).1.is_empty());
    }

    #[test]
    fn proofs_verify_against_root() {
        for n in [2, 3, 5, 8] {
            let l = leaves(n);
            let root = padded_merkle_root(&l);
            for (i, leaf) in l.iter().enumerate() {
                let proof = build_merkle_proof(&l, i).unwrap();
                assert!(
                    verify_merkle_proof(*leaf, i as u64, &proof, &root),
                    "{n} leaves, index {i}"
                );
            }
        }
    }

    #[test]
    fn proof_of_a_single_leaf_is_empty() {
        let proof = build_merkle_proof(&leaves(1), 0).unwrap();
        assert!(proof.siblings.is_empty() && proof.path.is_empty());
    }

    #[test]
    fn no_proof_for_an_index_past_the_padded_tree() {
        assert!(build_merkle_proof(&[], 0).is_none());
        assert!(build_merkle_proof(&leaves(1), 1).is_none());
        assert!(build_merkle_proof(&leaves(3), 4).is_none());
    }

    #[test]
    fn padding_slot_proof_verifies_with_zero_leaf() {
        let l = leaves(3);
        let root = padded_merkle_root(&l);
        // Index 3 is a padding slot of the 4-slot tree over 3 leaves.
        let proof = build_merkle_proof(&l, 3).unwrap();
        assert!(verify_merkle_proof(H256::zero(), 3, &proof, &root));
    }

    #[test]
    fn verify_accepts_an_empty_proof_only_for_leaf_equal_root() {
        let leaf = leaves(1)[0];
        let empty = MerkleProof {
            siblings: Vec::new(),
            path: Vec::new(),
        };
        assert!(verify_merkle_proof(leaf, 0, &empty, &leaf));
        assert!(!verify_merkle_proof(
            leaf,
            0,
            &empty,
            &H256::from_low_u64_be(9)
        ));
    }

    #[test]
    fn verify_rejects_wrong_leaf_root_and_index() {
        let l = leaves(5);
        let root = padded_merkle_root(&l);
        let proof = build_merkle_proof(&l, 2).unwrap();
        assert!(verify_merkle_proof(l[2], 2, &proof, &root));

        assert!(!verify_merkle_proof(l[3], 2, &proof, &root));
        assert!(!verify_merkle_proof(
            l[2],
            2,
            &proof,
            &H256::from_low_u64_be(9)
        ));
        // Index 3 has a different parity at the first level than the proof's path.
        assert!(!verify_merkle_proof(l[2], 3, &proof, &root));
    }

    #[test]
    fn verify_rejects_siblings_and_path_of_different_length() {
        let l = leaves(5);
        let root = padded_merkle_root(&l);

        let mut short_path = build_merkle_proof(&l, 2).unwrap();
        short_path.path.pop();
        assert!(!verify_merkle_proof(l[2], 2, &short_path, &root));

        let mut short_siblings = build_merkle_proof(&l, 2).unwrap();
        short_siblings.siblings.pop();
        assert!(!verify_merkle_proof(l[2], 2, &short_siblings, &root));
    }
}
