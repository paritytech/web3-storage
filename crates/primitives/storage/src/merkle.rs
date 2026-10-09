// SPDX-License-Identifier: Apache-2.0

//! Padded Merkle tree over chunk hashes: the tree shape of every `data_root`.

use crate::{hash_children, MerkleProof};
use alloc::vec::Vec;
use sp_core::H256;

/// Internal node of a Merkle tree: `hash = hash_children(left, right)`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct MerkleNode {
    /// Hash of this node.
    pub hash: H256,
    /// Left child hash.
    pub left: H256,
    /// Right child hash.
    pub right: H256,
}

impl MerkleNode {
    /// The bytes a provider stores for this node: `left ++ right`. Its stored
    /// children are `[left, right]`.
    pub fn data(&self) -> [u8; 64] {
        let mut data = [0u8; 64];
        data[..32].copy_from_slice(self.left.as_bytes());
        data[32..].copy_from_slice(self.right.as_bytes());
        data
    }
}

/// Levels of the padded Merkle tree over `leaves`, bottom-up: the leaves padded
/// to the next power of two with `H256::zero()`, then each parent level, ending
/// with the root alone. Needs at least two leaves.
fn padded_merkle_levels(leaves: &[H256]) -> Vec<Vec<H256>> {
    let mut level = leaves.to_vec();
    level.resize(leaves.len().next_power_of_two(), H256::zero());
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

/// Balanced Merkle tree over `leaves`, padded to the next power of two with
/// `H256::zero()` leaves. This is the tree shape of every `data_root`: the
/// provider stores it, [`padded_merkle_proof`] proves against it and
/// [`verify_merkle_proof`](crate::verify_merkle_proof) checks those proofs.
///
/// Returns the root and the internal nodes, bottom-up. No leaves gives a zero
/// root; one leaf is its own root. Equal subtrees appear once per position,
/// so a node hash can repeat.
pub fn padded_merkle_tree(leaves: &[H256]) -> (H256, Vec<MerkleNode>) {
    match leaves.len() {
        0 => return (H256::zero(), Vec::new()),
        1 => return (leaves[0], Vec::new()),
        _ => {}
    }
    let levels = padded_merkle_levels(leaves);
    let nodes = levels
        .windows(2)
        .flat_map(|pair| {
            pair[0]
                .chunks(2)
                .zip(pair[1].iter())
                .map(|(children, hash)| MerkleNode {
                    hash: *hash,
                    left: children[0],
                    right: children[1],
                })
        })
        .collect();
    (levels[levels.len() - 1][0], nodes)
}

/// Proof for the leaf at `index` in [`padded_merkle_tree`] over `leaves`.
/// Empty for a single leaf. `None` when `index` is not a leaf.
pub fn padded_merkle_proof(leaves: &[H256], index: usize) -> Option<MerkleProof> {
    if index >= leaves.len() {
        return None;
    }
    let mut proof = MerkleProof {
        siblings: Vec::new(),
        path: Vec::new(),
    };
    if leaves.len() == 1 {
        return Some(proof);
    }
    let levels = padded_merkle_levels(leaves);
    let mut idx = index;
    for level in &levels[..levels.len() - 1] {
        proof.siblings.push(level[idx ^ 1]);
        proof.path.push(idx % 2 == 1);
        idx /= 2;
    }
    Some(proof)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{blake2_256, verify_merkle_proof};

    fn leaf(n: u8) -> H256 {
        blake2_256(&[n])
    }

    const PADDED_ROOT_1_2_3: &str =
        "0xb00ba4725e9a6c357dfffcf32d0ea9f2b73418705bdab4ea5a602cac23fbc650";

    #[test]
    fn padded_merkle_tree_small_inputs() {
        assert_eq!(padded_merkle_tree(&[]), (H256::zero(), Vec::new()));
        assert_eq!(padded_merkle_tree(&[leaf(1)]), (leaf(1), Vec::new()));
    }

    /// Three leaves pad to four: the third leaf pairs with a zero hash
    /// instead of moving up a level unchanged.
    #[test]
    fn padded_merkle_tree_pads_odd_leaf_counts() {
        let (root, nodes) = padded_merkle_tree(&[leaf(1), leaf(2), leaf(3)]);
        let left = hash_children(leaf(1), leaf(2));
        let right = hash_children(leaf(3), H256::zero());
        assert_eq!(root, hash_children(left, right));
        assert_eq!(
            nodes,
            vec![
                MerkleNode {
                    hash: left,
                    left: leaf(1),
                    right: leaf(2)
                },
                MerkleNode {
                    hash: right,
                    left: leaf(3),
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

    /// Every leaf of a padded tree proves against its root by index.
    #[test]
    fn padded_merkle_proof_verifies_every_leaf() {
        let leaves: Vec<H256> = (1..=5).map(leaf).collect();
        let (root, nodes) = padded_merkle_tree(&leaves);
        for (index, leaf_hash) in leaves.iter().enumerate() {
            let proof = padded_merkle_proof(&leaves, index).unwrap();
            assert_eq!(proof.siblings.len(), 3);
            assert!(verify_merkle_proof(*leaf_hash, index as u64, &proof, &root));
        }
        assert_eq!(nodes.len(), 7);
        assert_eq!(padded_merkle_proof(&leaves, 5), None);
    }

    /// Fixed vector shared with `packages/core/src/merkle.test.ts`, so the
    /// Rust and TS trees stay identical: leaves are `blake2_256([1])`,
    /// `blake2_256([2])`, `blake2_256([3])`.
    #[test]
    fn padded_merkle_tree_matches_ts_vector() {
        let (root, _) = padded_merkle_tree(&[leaf(1), leaf(2), leaf(3)]);
        assert_eq!(format!("{root:?}"), PADDED_ROOT_1_2_3);
    }
}
