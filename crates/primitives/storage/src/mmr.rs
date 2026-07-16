// SPDX-License-Identifier: Apache-2.0

//! MMR and Merkle proof types plus hashing and verification utilities.

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

/// Merkle proof for verifying inclusion.
#[derive(Clone, PartialEq, Eq, Encode, Decode, DecodeWithMemTracking, TypeInfo, Debug)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct MerkleProof {
    /// Sibling hashes from leaf to root
    pub siblings: Vec<H256>,
    /// Path bits (false = left, true = right)
    pub path: Vec<bool>,
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

/// Compute blake2b-256 hash of data
pub fn blake2_256(data: &[u8]) -> H256 {
    // Use sp_core's blake2_256 which is optimized and available in both std and no_std
    sp_core::hashing::blake2_256(data).into()
}

/// Compute hash of two children for internal Merkle node
pub fn hash_children(left: H256, right: H256) -> H256 {
    let mut data = [0u8; 64];
    data[..32].copy_from_slice(left.as_bytes());
    data[32..].copy_from_slice(right.as_bytes());
    blake2_256(&data)
}

/// Verify a Merkle proof
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

/// Verify an MMR proof
///
/// This verifies that a leaf at the given index with the given hash
/// is part of an MMR with the given root.
pub fn verify_mmr_proof(proof: &MmrProof, root: &H256) -> bool {
    // First verify the Merkle proof gets us to the data root
    let leaf_hash = blake2_256(&proof.leaf.encode());

    // Hash up from leaf through the Merkle proof to reach a peak
    let mut current = leaf_hash;
    for (i, sibling) in proof.leaf_proof.siblings.iter().enumerate() {
        let is_right = proof.leaf_proof.path.get(i).copied().unwrap_or(false);
        current = if is_right {
            hash_children(*sibling, current)
        } else {
            hash_children(current, *sibling)
        };
    }

    // Current should be one of the peaks
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

    bagged_root == *root
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_blake2_256() {
        let data = b"hello world";
        let hash = blake2_256(data);
        assert_ne!(hash, H256::zero());

        // Same input should produce same output
        let hash2 = blake2_256(data);
        assert_eq!(hash, hash2);

        // Different input should produce different output
        let hash3 = blake2_256(b"hello world!");
        assert_ne!(hash, hash3);
    }
}
