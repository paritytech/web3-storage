// SPDX-License-Identifier: Apache-2.0

//! Balanced (padded) Merkle proof construction. Pure math over `H256` - no
//! storage access.

use sp_core::H256;
use storage_primitives::hash_children;

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
    use storage_primitives::{blake2_256, verify_merkle_proof};

    /// Root of the padded balanced tree, folded independently of
    /// `build_merkle_proof` so the test checks the proof against a root the
    /// function under test never touches.
    fn padded_root(leaf_hashes: &[H256]) -> H256 {
        let padded_len = leaf_hashes.len().next_power_of_two();
        let mut level = leaf_hashes.to_vec();
        level.resize(padded_len, H256::zero());
        while level.len() > 1 {
            level = level
                .chunks(2)
                .map(|pair| hash_children(pair[0], pair[1]))
                .collect();
        }
        level[0]
    }

    #[test]
    fn proof_verifies_for_non_power_of_two_leaf_counts() {
        // 2 and 8 pad to themselves, at depth 1 and 3: the no-padding base
        // case and its deep counterpart. 3 pairs a leaf directly with the
        // zero pad; 5 folds a whole `H(0, 0)` subtree of padding into a
        // sibling. A failure at 3 or 5 that passes at 2 and 8 is a padding
        // bug rather than a sibling or path bug.
        for leaf_count in [2usize, 3, 5, 8] {
            let leaves: Vec<H256> = (0..leaf_count)
                .map(|i| blake2_256(format!("leaf{i}").as_bytes()))
                .collect();
            let root = padded_root(&leaves);

            for index in 0..leaf_count {
                let proof = build_merkle_proof(&leaves, index);
                assert!(
                    verify_merkle_proof(leaves[index], index as u64, &proof, &root),
                    "proof for leaf {index} of {leaf_count} leaves did not verify"
                );
            }
        }
    }
}
