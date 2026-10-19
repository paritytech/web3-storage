// SPDX-License-Identifier: Apache-2.0

//! Merkle Mountain Range implementation.
//!
//! Positions are assigned sequentially as nodes are added. Leaf positions
//! follow the formula `leaf_pos(k) = 2*k - popcount(k)` (0-indexed).
//! After inserting n leaves, the number of parent merges equals
//! `n.trailing_zeros()`.

use sp_core::H256;
use storage_primitives::{bag_peaks, hash_children};

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

    /// Generate a proof with path bits for a leaf at the given index.
    ///
    /// Returns `(siblings, path_bits, peaks)` suitable for constructing
    /// a `storage_primitives::MmrProof`. At each level, `is_right = true`
    /// means the current node is the right child (sibling is to its left).
    pub fn proof_with_path(&self, leaf_index: u64) -> Option<(Vec<H256>, Vec<bool>, Vec<H256>)> {
        let position = storage_primitives::mmr_leaf_position(leaf_index, self.leaf_count)?;
        let leaf_pos = Self::leaf_index_to_pos(leaf_index);

        let mut siblings = Vec::new();
        let mut path = Vec::new();
        let mut pos = leaf_pos;

        for h in 0..position.height {
            let is_right = (position.local_index >> h) & 1 == 1;
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

#[cfg(test)]
mod tests {
    use super::*;
    use storage_primitives::{blake2_256, hash_leaf};

    #[test]
    fn test_mmr_basic() {
        let mut mmr = Mmr::new();
        assert_eq!(mmr.root(), H256::zero());

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
            mmr.push(blake2_256(format!("leaf{i}").as_bytes()));
            let expected_nodes = 2 * i - i.count_ones() as u64;
            assert_eq!(
                mmr.nodes.len() as u64,
                expected_nodes,
                "node count wrong after {i} leaves"
            );
        }
    }

    // `verify_mmr_proof(proof, leaf_index, commitment)` accepts a leaf
    // only when it is presented at its own index.
    #[test]
    fn challenge_binds_leaf_index() {
        use codec::Encode;
        use storage_primitives::{
            verify_merkle_proof, verify_mmr_proof, Commitment, MerkleProof, MmrLeaf, MmrProof,
        };

        // A bucket MMR of 6 single-chunk "files". For a single chunk,
        // data_root == hash_leaf(chunk), so the chunk proof is empty and
        // verify_merkle_proof(chunk_hash, 0, 1, [], &data_root) reduces to
        // chunk_hash == data_root.
        let files: Vec<Vec<u8>> = (0..6)
            .map(|i| format!("file {i} contents").into_bytes())
            .collect();
        let leaf_count = files.len() as u64;

        let mut mmr = Mmr::new();
        let mut leaves: Vec<MmrLeaf> = Vec::new();
        let mut total = 0u64;
        for f in &files {
            let data_root = hash_leaf(f);
            let data_size = f.len() as u64;
            total += data_size;
            let leaf = MmrLeaf {
                data_root,
                data_size,
                total_size: total,
            };
            mmr.push(hash_leaf(&leaf.encode()));
            leaves.push(leaf);
        }
        let root = mmr.root();
        let commitment = |leaf_count: u64| Commitment {
            mmr_root: root,
            start_seq: 0,
            leaf_count,
        };

        // Build an on-chain `storage_primitives::MmrProof` for a chosen leaf index.
        let make_proof = |idx: u64| -> MmrProof {
            let (siblings, path, peaks) = mmr.proof_with_path(idx).unwrap();
            MmrProof {
                peaks,
                leaf: leaves[idx as usize].clone(),
                leaf_proof: MerkleProof { siblings, path },
            }
        };

        // The two checks the pallet runs for a `Proof` response (chunk_index 0,
        // empty chunk proof), now BOUND to the challenged (leaf_index, leaf_count).
        let empty = MerkleProof {
            siblings: vec![],
            path: vec![],
        };
        let pallet_accepts = |chunk_data: &[u8], challenged_leaf: u64, proof: &MmrProof| -> bool {
            let chunk_hash = hash_leaf(chunk_data);
            let chunk_count = storage_primitives::chunk_count(proof.leaf.data_size);
            verify_merkle_proof(chunk_hash, 0, chunk_count, &empty, &proof.leaf.data_root)
                && verify_mmr_proof(proof, challenged_leaf, &commitment(leaf_count))
        };

        // HONEST: challenge for leaf 5, answer with leaf 5 -> accepted.
        assert!(
            pallet_accepts(&files[5], 5, &make_proof(5)),
            "honest proof for the challenged leaf must pass"
        );

        // SUBSTITUTION (the fix): challenge for leaf 5, answer with leaf 2's proof
        // + data -> REJECTED, because the proof's path/peak don't match the path
        // derived from leaf_index 5.
        assert!(
            !pallet_accepts(&files[2], 5, &make_proof(2)),
            "a substituted leaf must be rejected for the challenged index"
        );

        // Exhaustive: a proof for leaf `answer` verifies against a leaf-`challenged`
        // challenge IFF answer == challenged. No off-diagonal substitution survives.
        for answer in 0..leaf_count {
            for challenged in 0..leaf_count {
                let accepted =
                    pallet_accepts(&files[answer as usize], challenged, &make_proof(answer));
                assert_eq!(
                    accepted,
                    answer == challenged,
                    "leaf {answer} answering a leaf-{challenged} challenge"
                );
            }
        }

        // Tampered chunk bytes fail (chunk content is bound).
        assert!(
            !pallet_accepts(b"forged bytes", 5, &make_proof(5)),
            "forged chunk bytes must fail"
        );

        // Out-of-range challenged leaf_index fails (the leaf does not exist).
        assert!(
            !pallet_accepts(&files[3], leaf_count, &make_proof(3)),
            "an out-of-range leaf_index must fail"
        );

        // A wrong leaf_count (peak-structure mismatch) fails.
        assert!(
            !verify_mmr_proof(&make_proof(5), 5, &commitment(leaf_count + 1)),
            "a mismatched leaf_count must fail"
        );
    }

    #[test]
    fn test_proof_with_path_primitives_verify() {
        use codec::Encode;

        // This test mirrors how the pallet verifies: push hash_leaf(&leaf.encode())
        // into MMR, then verify with storage_primitives::verify_mmr_proof
        let mut mmr = Mmr::new();

        let mmr_leaves: Vec<storage_primitives::MmrLeaf> = (0..5)
            .map(|i| storage_primitives::MmrLeaf {
                data_root: blake2_256(format!("root{i}").as_bytes()),
                data_size: 100 * (i as u64 + 1),
                total_size: 100 * (i as u64 + 1),
            })
            .collect();

        for leaf in &mmr_leaves {
            mmr.push(hash_leaf(&leaf.encode()));
        }

        let commitment = storage_primitives::Commitment {
            mmr_root: mmr.root(),
            start_seq: 0,
            leaf_count: 5,
        };

        for (i, leaf) in mmr_leaves.iter().enumerate() {
            let (siblings, path, peaks) =
                mmr.proof_with_path(i as u64).expect("proof should exist");

            let mmr_proof = storage_primitives::MmrProof {
                peaks,
                leaf: leaf.clone(),
                leaf_proof: storage_primitives::MerkleProof { siblings, path },
            };

            assert!(
                storage_primitives::verify_mmr_proof(&mmr_proof, i as u64, &commitment),
                "verify_mmr_proof failed for leaf {i}"
            );
        }
    }
}
