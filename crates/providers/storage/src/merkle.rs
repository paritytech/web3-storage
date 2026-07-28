// SPDX-License-Identifier: Apache-2.0

//! Balanced (padded) Merkle proof construction, plus hex encode/decode
//! utilities. Pure math over `H256` - no storage access.

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

/// Encode bytes as lowercase hex (no `0x` prefix).
pub fn hex_encode(bytes: &[u8]) -> String {
    hex::encode(bytes)
}

/// Decode a hex string, accepting an optional `0x` prefix.
pub fn hex_decode(s: &str) -> Result<Vec<u8>, &'static str> {
    let s = s.strip_prefix("0x").unwrap_or(s);
    hex::decode(s).map_err(|e| match e {
        hex::FromHexError::OddLength => "invalid hex length",
        _ => "invalid hex",
    })
}
