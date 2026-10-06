// SPDX-License-Identifier: Apache-2.0

//! Balanced (padded) Merkle proof construction. Pure math over `H256` - no
//! storage access.

use sp_core::H256;
use storage_primitives::hash_children;

/// Root of the zero-padded balanced binary tree over `leaf_hashes`.
///
/// No leaves give `H256::zero()` and one leaf gives that leaf. Otherwise the
/// leaves are padded with `H256::zero()` to the next power of two, the same
/// shape `build_merkle_proof` proves against.
pub fn padded_merkle_root(leaf_hashes: &[H256]) -> H256 {
    match leaf_hashes {
        [] => return H256::zero(),
        [leaf] => return *leaf,
        _ => {}
    }

    let mut level = leaf_hashes.to_vec();
    level.resize(leaf_hashes.len().next_power_of_two(), H256::zero());
    while level.len() > 1 {
        level = level
            .chunks(2)
            .map(|pair| hash_children(pair[0], pair[1]))
            .collect();
    }
    level[0]
}

/// Build a Merkle proof for a leaf at the given index in a balanced (padded) tree.
///
/// Pads the leaf hashes to the next power of 2 with `H256::zero()` so that
/// the standard index-based verification in `verify_merkle_proof` works.
pub fn build_merkle_proof(leaf_hashes: &[H256], index: usize) -> storage_primitives::MerkleProof {
    if leaf_hashes.len() <= 1 {
        return storage_primitives::MerkleProof {
            siblings: vec![],
            path: vec![],
        };
    }

    // Pad to next power of 2 for a balanced tree
    let padded_len = leaf_hashes.len().next_power_of_two();
    let mut current_level = leaf_hashes.to_vec();
    current_level.resize(padded_len, H256::zero());

    let mut siblings = Vec::new();
    let mut path = Vec::new();
    let mut idx = index;

    while current_level.len() > 1 {
        let is_right = idx % 2 == 1;
        let sibling_idx = if is_right { idx - 1 } else { idx + 1 };
        siblings.push(current_level[sibling_idx]);
        path.push(is_right);

        // Build next level
        let mut next_level = Vec::new();
        for pair in current_level.chunks(2) {
            next_level.push(hash_children(pair[0], pair[1]));
        }

        idx /= 2;
        current_level = next_level;
    }

    storage_primitives::MerkleProof { siblings, path }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::backend::{build_padded_merkle_tree, DiskStorage};
    use storage_primitives::verify_merkle_proof;
    use tempfile::TempDir;

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
    fn root_matches_stored_tree() {
        for n in [0, 1, 2, 3, 5, 8] {
            let dir = TempDir::new().unwrap();
            let storage = DiskStorage::new(dir.path()).unwrap();
            let l = leaves(n);
            assert_eq!(
                padded_merkle_root(&l),
                build_padded_merkle_tree(&storage, 1, &l),
                "{n} leaves"
            );
        }
    }

    #[test]
    fn proofs_verify_against_root() {
        for n in [2, 3, 5, 8] {
            let l = leaves(n);
            let root = padded_merkle_root(&l);
            for (i, leaf) in l.iter().enumerate() {
                let proof = build_merkle_proof(&l, i);
                assert!(
                    verify_merkle_proof(*leaf, i as u64, &proof, &root),
                    "{n} leaves, index {i}"
                );
            }
        }
    }
}
