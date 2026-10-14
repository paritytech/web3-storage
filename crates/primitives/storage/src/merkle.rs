// SPDX-License-Identifier: Apache-2.0

//! Merkle tree over the chunks of one data root.
//!
//! The tree is a binary tree in which an odd last node of a level moves up to
//! the next level unchanged, and every internal node is
//! `hash_children(left, right)`. No leaves give `H256::zero()` and one leaf is
//! its own root. The provider builds, stores and proves against this shape, and
//! the pallet verifies proofs against it.

use crate::{blake2_256, DEFAULT_CHUNK_SIZE};
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

/// An internal node of the tree.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct MerkleNode {
    /// `hash_children(left, right)`.
    pub hash: H256,
    /// Left child hash.
    pub left: H256,
    /// Right child hash.
    pub right: H256,
}

/// Hash of the two children of an internal node: `blake2_256(left || right)`.
pub fn hash_children(left: H256, right: H256) -> H256 {
    let mut data = [0u8; 64];
    data[..32].copy_from_slice(left.as_bytes());
    data[32..].copy_from_slice(right.as_bytes());
    blake2_256(&data)
}

/// Number of chunks (tree leaves) of data with `data_size` bytes.
///
/// Every chunk is `DEFAULT_CHUNK_SIZE` bytes except the last, and empty data is
/// one empty chunk.
pub fn chunk_count(data_size: u64) -> u64 {
    data_size.div_ceil(DEFAULT_CHUNK_SIZE as u64).max(1)
}

/// Every level of the tree, from the leaves (first) to the root (last, one
/// element). Empty for no leaves.
fn levels(leaf_hashes: &[H256]) -> Vec<Vec<H256>> {
    if leaf_hashes.is_empty() {
        return Vec::new();
    }

    let mut level = leaf_hashes.to_vec();
    let mut levels = Vec::new();
    while level.len() > 1 {
        let next = level
            .chunks(2)
            .map(|pair| match pair {
                [left, right] => hash_children(*left, *right),
                [last] => *last,
                _ => unreachable!("chunks(2) yields one or two items"),
            })
            .collect();
        levels.push(core::mem::replace(&mut level, next));
    }
    levels.push(level);
    levels
}

/// Root of the tree over `leaf_hashes`.
///
/// No leaves give `H256::zero()` and one leaf gives that leaf.
pub fn merkle_root(leaf_hashes: &[H256]) -> H256 {
    levels(leaf_hashes)
        .last()
        .map_or_else(H256::zero, |root| root[0])
}

/// Root and internal nodes of the tree over `leaf_hashes`.
///
/// Nodes come children before parents, so a store that requires children to
/// exist can insert them in order. No leaves or one leaf give no nodes.
pub fn merkle_tree(leaf_hashes: &[H256]) -> (H256, Vec<MerkleNode>) {
    let levels = levels(leaf_hashes);
    let nodes = levels
        .windows(2)
        .flat_map(|pair| {
            pair[0]
                .chunks(2)
                .zip(&pair[1])
                .filter_map(|(children, hash)| match children {
                    [left, right] => Some(MerkleNode {
                        hash: *hash,
                        left: *left,
                        right: *right,
                    }),
                    _ => None,
                })
        })
        .collect();
    let root = levels.last().map_or_else(H256::zero, |root| root[0]);
    (root, nodes)
}

/// Build a Merkle proof for the leaf at `index` in the tree over
/// `leaf_hashes`. The proof is empty for a single leaf.
///
/// `None` if `index` is not the index of a leaf in `leaf_hashes`.
pub fn build_merkle_proof(leaf_hashes: &[H256], index: usize) -> Option<MerkleProof> {
    if index >= leaf_hashes.len() {
        return None;
    }

    let levels = levels(leaf_hashes);
    let mut siblings = Vec::new();
    let mut path = Vec::new();
    let mut idx = index;

    // The last level is the root, which has no sibling.
    for level in &levels[..levels.len() - 1] {
        // An odd last node moves up without a sibling.
        if idx != level.len() - 1 || level.len().is_multiple_of(2) {
            let is_right = idx % 2 == 1;
            siblings.push(level[idx ^ 1]);
            path.push(is_right);
        }
        idx /= 2;
    }

    Some(MerkleProof { siblings, path })
}

/// `true` if `proof` links `leaf_hash` at leaf `index` of a tree with
/// `leaf_count` leaves to `root`.
///
/// Rejects proofs whose `siblings` and `path` lengths differ or whose path bits
/// do not match `index`, an `index` outside the tree, and proofs with a wrong
/// number of siblings for `leaf_count`.
pub fn verify_merkle_proof(
    leaf_hash: H256,
    index: u64,
    leaf_count: u64,
    proof: &MerkleProof,
    root: &H256,
) -> bool {
    if proof.siblings.len() != proof.path.len() || index >= leaf_count {
        return false;
    }

    let mut current = leaf_hash;
    let mut current_index = index;
    let mut width = leaf_count;
    let mut steps = proof.siblings.iter().zip(proof.path.iter());

    while width > 1 {
        // An odd last node moves up without a sibling.
        if current_index != width - 1 || width.is_multiple_of(2) {
            let Some((sibling, is_right)) = steps.next() else {
                return false;
            };
            if *is_right != (current_index % 2 == 1) {
                return false;
            }
            current = if *is_right {
                hash_children(*sibling, current)
            } else {
                hash_children(current, *sibling)
            };
        }
        current_index /= 2;
        width = width.div_ceil(2);
    }

    steps.next().is_none() && current == *root
}

#[cfg(test)]
mod tests {
    use super::*;

    fn leaves(n: usize) -> Vec<H256> {
        (1..=n).map(|i| H256::from_low_u64_be(i as u64)).collect()
    }

    fn proof_for(l: &[H256], index: usize) -> MerkleProof {
        build_merkle_proof(l, index).unwrap()
    }

    #[test]
    fn root_of_empty_and_single_leaf() {
        assert_eq!(merkle_root(&[]), H256::zero());
        let leaf = leaves(1)[0];
        assert_eq!(merkle_root(&[leaf]), leaf);
    }

    #[test]
    fn root_of_two_leaves_hashes_the_pair() {
        let l = leaves(2);
        assert_eq!(merkle_root(&l), hash_children(l[0], l[1]));
    }

    #[test]
    fn root_of_three_leaves_moves_the_odd_leaf_up() {
        let l = leaves(3);
        assert_eq!(
            merkle_root(&l),
            hash_children(hash_children(l[0], l[1]), l[2])
        );
    }

    #[test]
    fn root_of_five_leaves_moves_the_odd_leaf_up_two_levels() {
        let l = leaves(5);
        let left = hash_children(hash_children(l[0], l[1]), hash_children(l[2], l[3]));
        assert_eq!(merkle_root(&l), hash_children(left, l[4]));
    }

    /// Roots over `blake2_256([i])` for `i` in `1..=n`, computed with an
    /// independent implementation. The TypeScript SDK test uses the same values.
    #[test]
    fn roots_match_golden_vectors() {
        let golden = [
            "ee155ace9c40292074cb6aff8c9ccdd273c81648ff1149ef36bcea6ebb8a3e25",
            "5a3746f7ad1bece55c75e3cd11d299fe5f3e4f4fb03035e3b8d87d182d705de7",
            "e0d2cb92e481829535abbcab54c8fd72f5a8d9787d37312d3679f1a745b5effb",
            "114d5dc3c70125a2159b283e59b8ac4330014aa80cccdad51d5ee18f02c37a7e",
            "9a8a0313d2a3e6190c5619884c1fb351e52ed99f96bb93253b6dcb77765ebf63",
            "25875908add04dfb7035e7e8ff383fcc92b2586bbe6c64dc0d607902366e7b48",
            "bfd907b4476ee8bda6531809a94c59149108b1881c253746530b10c1354b4843",
            "e0ae36209c0562a4d8075c37a23572e393ab3c3e9e95b6e061f8c203b06f7f27",
            "3a60ffa28abb95e837960000c1e9644b08084c5ac838aadc103a6f5f1f840ced",
        ];
        for (i, expected) in golden.iter().enumerate() {
            let n = i + 1;
            let l: Vec<H256> = (1..=n as u8).map(|b| blake2_256(&[b])).collect();
            let mut bytes = [0u8; 32];
            for (j, byte) in bytes.iter_mut().enumerate() {
                *byte = u8::from_str_radix(&expected[2 * j..2 * j + 2], 16).unwrap();
            }
            assert_eq!(merkle_root(&l), H256(bytes), "{n} leaves");
        }
    }

    #[test]
    fn tree_root_equals_root() {
        for n in 0..=9 {
            let l = leaves(n);
            assert_eq!(merkle_tree(&l).0, merkle_root(&l), "{n} leaves");
        }
    }

    #[test]
    fn tree_nodes_come_children_before_parents() {
        let l = leaves(3);
        let (root, nodes) = merkle_tree(&l);
        let left = hash_children(l[0], l[1]);
        assert_eq!(
            nodes,
            [
                MerkleNode {
                    hash: left,
                    left: l[0],
                    right: l[1]
                },
                MerkleNode {
                    hash: root,
                    left,
                    right: l[2]
                },
            ]
        );
    }

    #[test]
    fn tree_has_one_node_per_pair() {
        // n leaves have n - 1 internal nodes in a tree without padding.
        for n in 2..=9 {
            assert_eq!(merkle_tree(&leaves(n)).1.len(), n - 1, "{n} leaves");
        }
    }

    #[test]
    fn tree_of_zero_or_one_leaf_has_no_nodes() {
        assert!(merkle_tree(&[]).1.is_empty());
        assert!(merkle_tree(&leaves(1)).1.is_empty());
    }

    #[test]
    fn proofs_verify_against_root() {
        for n in 1..=9 {
            let l = leaves(n);
            let root = merkle_root(&l);
            for (i, leaf) in l.iter().enumerate() {
                let proof = proof_for(&l, i);
                assert!(
                    verify_merkle_proof(*leaf, i as u64, n as u64, &proof, &root),
                    "{n} leaves, index {i}"
                );
            }
        }
    }

    #[test]
    fn proof_of_a_single_leaf_is_empty() {
        let proof = proof_for(&leaves(1), 0);
        assert!(proof.siblings.is_empty() && proof.path.is_empty());
    }

    #[test]
    fn proof_of_the_odd_last_leaf_skips_the_levels_it_moves_through() {
        // Leaf 2 of 3 moves up one level, so its only sibling is H(l0, l1).
        let l = leaves(3);
        let proof = proof_for(&l, 2);
        assert_eq!(proof.siblings, [hash_children(l[0], l[1])]);
        assert_eq!(proof.path, [true]);
    }

    #[test]
    fn no_proof_for_an_index_outside_the_leaves() {
        assert!(build_merkle_proof(&[], 0).is_none());
        assert!(build_merkle_proof(&leaves(1), 1).is_none());
        assert!(build_merkle_proof(&leaves(3), 3).is_none());
    }

    #[test]
    fn verify_accepts_an_empty_proof_only_for_leaf_equal_root() {
        let leaf = leaves(1)[0];
        let empty = MerkleProof {
            siblings: Vec::new(),
            path: Vec::new(),
        };
        assert!(verify_merkle_proof(leaf, 0, 1, &empty, &leaf));
        assert!(!verify_merkle_proof(
            leaf,
            0,
            1,
            &empty,
            &H256::from_low_u64_be(9)
        ));
    }

    #[test]
    fn verify_rejects_wrong_leaf_root_and_index() {
        let l = leaves(5);
        let root = merkle_root(&l);
        let proof = proof_for(&l, 2);
        assert!(verify_merkle_proof(l[2], 2, 5, &proof, &root));

        assert!(!verify_merkle_proof(l[3], 2, 5, &proof, &root));
        assert!(!verify_merkle_proof(
            l[2],
            2,
            5,
            &proof,
            &H256::from_low_u64_be(9)
        ));
        // Index 3 has a different parity at the first level than the proof's path.
        assert!(!verify_merkle_proof(l[2], 3, 5, &proof, &root));
    }

    #[test]
    fn verify_rejects_an_index_outside_the_tree() {
        let l = leaves(3);
        let root = merkle_root(&l);
        let proof = proof_for(&l, 2);
        assert!(!verify_merkle_proof(l[2], 3, 3, &proof, &root));
        assert!(!verify_merkle_proof(l[2], 0, 0, &proof, &root));
        let empty = MerkleProof {
            siblings: Vec::new(),
            path: Vec::new(),
        };
        assert!(!verify_merkle_proof(
            H256::zero(),
            0,
            0,
            &empty,
            &H256::zero()
        ));
    }

    #[test]
    fn proof_of_the_odd_last_leaf_binds_the_leaf_count() {
        // The proof of leaf 2 in a 3-leaf tree is valid for index 2 of 3 only.
        let l = leaves(3);
        let root = merkle_root(&l);
        let proof = proof_for(&l, 2);
        assert!(verify_merkle_proof(l[2], 2, 3, &proof, &root));
        // As index 1 the path bit is wrong.
        assert!(!verify_merkle_proof(l[2], 1, 3, &proof, &root));
        // With a count of 2, index 2 is outside the tree.
        assert!(!verify_merkle_proof(l[2], 2, 2, &proof, &root));
        // The root over 3 leaves is also the root over the 2 leaves
        // `[H(l0, l1), l2]`, so a prover-chosen count of 2 would accept index 1.
        // The count must come from authenticated data.
        assert!(verify_merkle_proof(l[2], 1, 2, &proof, &root));
    }

    #[test]
    fn verify_rejects_siblings_and_path_of_different_length() {
        let l = leaves(5);
        let root = merkle_root(&l);

        let mut short_path = proof_for(&l, 2);
        short_path.path.pop();
        assert!(!verify_merkle_proof(l[2], 2, 5, &short_path, &root));

        let mut short_siblings = proof_for(&l, 2);
        short_siblings.siblings.pop();
        assert!(!verify_merkle_proof(l[2], 2, 5, &short_siblings, &root));
    }

    #[test]
    fn verify_rejects_missing_and_extra_siblings() {
        let l = leaves(5);
        let root = merkle_root(&l);

        let mut missing = proof_for(&l, 2);
        missing.siblings.pop();
        missing.path.pop();
        assert!(!verify_merkle_proof(l[2], 2, 5, &missing, &root));

        let mut extra = proof_for(&l, 2);
        extra.siblings.push(H256::from_low_u64_be(9));
        extra.path.push(false);
        assert!(!verify_merkle_proof(l[2], 2, 5, &extra, &root));
    }

    #[test]
    fn chunk_count_rounds_up_and_has_a_minimum_of_one() {
        let chunk = DEFAULT_CHUNK_SIZE as u64;
        assert_eq!(chunk_count(0), 1);
        assert_eq!(chunk_count(1), 1);
        assert_eq!(chunk_count(chunk), 1);
        assert_eq!(chunk_count(chunk + 1), 2);
        assert_eq!(chunk_count(2 * chunk), 2);
        assert_eq!(chunk_count(2 * chunk + 10), 3);
    }
}
