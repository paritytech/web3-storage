// SPDX-License-Identifier: Apache-2.0

//! Merkle tree over the chunks of one data root.
//!
//! Every internal node is `hash_children(left, right)`. An odd last node of a
//! level moves up to the next level unchanged.

use crate::hash_children;
use alloc::vec::Vec;
use core::ops::Range;
use sp_core::H256;

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
            .filter_map(|pair| pair.iter().copied().reduce(hash_children))
            .collect();
        levels.push(core::mem::replace(&mut level, next));
    }
    levels.push(level);
    levels
}

fn root(levels: &[Vec<H256>]) -> H256 {
    levels.last().map_or_else(H256::zero, |root| root[0])
}

/// Root of the tree over `leaf_hashes`.
///
/// No leaves give `H256::zero()` and one leaf gives that leaf.
pub fn merkle_root(leaf_hashes: &[H256]) -> H256 {
    root(&levels(leaf_hashes))
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
    (root(&levels), nodes)
}

/// Merkle proofs for the leaves at `indices` in the tree over `leaf_hashes`,
/// in index order. Leaves out indices past the last leaf.
///
/// A proof is the sibling hashes from the leaf up to the root. A verifier
/// takes the side of each sibling from the leaf index and the leaf count.
pub fn build_merkle_proofs(leaf_hashes: &[H256], indices: Range<usize>) -> Vec<Vec<H256>> {
    let end = indices.end.min(leaf_hashes.len());
    let start = indices.start.min(end);
    if start == end {
        return Vec::new();
    }

    let levels = levels(leaf_hashes);
    (start..end)
        .map(|index| proof_from_levels(&levels, index))
        .collect()
}

/// Proof for the leaf at `index`, from the `levels` of a tree with that leaf.
fn proof_from_levels(levels: &[Vec<H256>], index: usize) -> Vec<H256> {
    let mut siblings = Vec::new();
    let mut idx = index;

    // The last level is the root, which has no sibling.
    for level in &levels[..levels.len() - 1] {
        // An odd last node moves up without a sibling.
        if idx != level.len() - 1 || level.len().is_multiple_of(2) {
            siblings.push(level[idx ^ 1]);
        }
        idx /= 2;
    }

    siblings
}

/// `true` if the sibling hashes in `proof` link `leaf_hash` at leaf `index`
/// of a tree with `leaf_count` leaves to `root`. `index` and `leaf_count`
/// decide whether each sibling is the left or the right child.
///
/// `leaf_count` must come from authenticated data, such as a proven
/// `data_size`. With a count chosen by the prover, a proof for another index
/// can verify.
pub fn verify_merkle_proof(
    leaf_hash: H256,
    index: u64,
    leaf_count: u64,
    proof: &[H256],
    root: &H256,
) -> bool {
    if index >= leaf_count {
        return false;
    }

    let mut current = leaf_hash;
    let mut current_index = index;
    let mut width = leaf_count;
    let mut steps = proof.iter();

    while width > 1 {
        // An odd last node moves up without a sibling.
        if current_index != width - 1 || width.is_multiple_of(2) {
            let Some(sibling) = steps.next() else {
                return false;
            };
            current = if current_index % 2 == 1 {
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
    use crate::blake2_256;

    fn leaves(n: usize) -> Vec<H256> {
        (1..=n).map(|i| H256::from_low_u64_be(i as u64)).collect()
    }

    fn proof_for(l: &[H256], index: usize) -> Vec<H256> {
        let mut proofs = build_merkle_proofs(l, index..index + 1);
        assert_eq!(proofs.len(), 1, "index {index}");
        proofs.remove(0)
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
            "5632fced358a72ed6fd53ada497c9980a010c7e82c8386a10c12f2f27f9195f3",
            "56d7f37e08c1fac178a24ff1a5cc3413e35e4816ad2ce2a79120f3072bc9c618",
            "226e43e73fc2eb5cb50dc8f74106275685bf54f1aabd6e07e67c285f6f1032f9",
            "96c211dab54f3b6abd7a0f240df556dc810fc28d216c3b60dde7f491bbb5805e",
            "ea39f530b256e22bbff141464469eca0da922437cbab3e08fc706ac147ddc002",
            "470b3383b4658c30ba2466cc75dba6f0316fbe66e0e711708b47a85650f803b1",
            "734ed4bd23af21a41d23be13e369e592f49499fd2ef89e1b33b96a4540daa0f7",
            "c487902011078c0f151b8986afe18f487d7c69f46d58fd4ce293ade5b2a40ce5",
        ];
        for (i, expected) in golden.iter().enumerate() {
            let n = i + 1;
            let l: Vec<H256> = (1..=n as u8).map(|b| blake2_256(&[b])).collect();
            assert_eq!(
                merkle_root(&l),
                expected.parse::<H256>().unwrap(),
                "{n} leaves"
            );
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
        // n leaves give n - 1 internal nodes.
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
        assert!(proof.is_empty());
    }

    #[test]
    fn proof_of_the_odd_last_leaf_skips_the_levels_it_moves_through() {
        // Leaf 2 of 3 moves up one level, so its only sibling is H(l0, l1).
        let l = leaves(3);
        let proof = proof_for(&l, 2);
        assert_eq!(proof, [hash_children(l[0], l[1])]);
    }

    #[test]
    fn proofs_of_a_range_equal_single_proofs() {
        let l = leaves(7);
        let proofs = build_merkle_proofs(&l, 2..6);
        assert_eq!(proofs.len(), 4);
        for (proof, index) in proofs.iter().zip(2..6) {
            assert_eq!(*proof, proof_for(&l, index), "index {index}");
        }
    }

    #[test]
    fn proofs_leave_out_indices_past_the_last_leaf() {
        assert!(build_merkle_proofs(&[], 0..1).is_empty());
        assert!(build_merkle_proofs(&leaves(1), 1..2).is_empty());
        assert!(build_merkle_proofs(&leaves(3), 3..5).is_empty());
        assert_eq!(build_merkle_proofs(&leaves(7), 6..8).len(), 1);
        assert!(build_merkle_proofs(&leaves(7), 7..7).is_empty());
        assert!(build_merkle_proofs(&leaves(7), Range { start: 4, end: 2 }).is_empty());
    }

    #[test]
    fn verify_accepts_an_empty_proof_only_for_leaf_equal_root() {
        let leaf = leaves(1)[0];
        assert!(verify_merkle_proof(leaf, 0, 1, &[], &leaf));
        assert!(!verify_merkle_proof(
            leaf,
            0,
            1,
            &[],
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
        // Index 3 puts the first sibling on the other side.
        assert!(!verify_merkle_proof(l[2], 3, 5, &proof, &root));
    }

    #[test]
    fn verify_rejects_an_index_outside_the_tree() {
        let l = leaves(3);
        let root = merkle_root(&l);
        let proof = proof_for(&l, 2);
        assert!(!verify_merkle_proof(l[2], 3, 3, &proof, &root));
        assert!(!verify_merkle_proof(l[2], 0, 0, &proof, &root));
        assert!(!verify_merkle_proof(H256::zero(), 0, 0, &[], &H256::zero()));
    }

    #[test]
    fn proof_of_the_odd_last_leaf_binds_the_leaf_count() {
        let l = leaves(3);
        let root = merkle_root(&l);
        let proof = proof_for(&l, 2);
        assert!(verify_merkle_proof(l[2], 2, 3, &proof, &root));
        // As index 1 the proof is one sibling short.
        assert!(!verify_merkle_proof(l[2], 1, 3, &proof, &root));
        // With a count of 2, index 2 is outside the tree.
        assert!(!verify_merkle_proof(l[2], 2, 2, &proof, &root));
        // The root over 3 leaves is also the root over `[H(l0, l1), l2]`.
        assert!(verify_merkle_proof(l[2], 1, 2, &proof, &root));
    }

    #[test]
    fn verify_rejects_missing_and_extra_siblings() {
        let l = leaves(5);
        let root = merkle_root(&l);

        let mut missing = proof_for(&l, 2);
        missing.pop();
        assert!(!verify_merkle_proof(l[2], 2, 5, &missing, &root));

        let mut extra = proof_for(&l, 2);
        extra.push(H256::from_low_u64_be(9));
        assert!(!verify_merkle_proof(l[2], 2, 5, &extra, &root));
    }
}
