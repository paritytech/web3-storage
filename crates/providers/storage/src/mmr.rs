// SPDX-License-Identifier: Apache-2.0

//! Merkle Mountain Range implementation.
//!
//! Positions are assigned sequentially as nodes are added. Leaf positions
//! follow the formula `leaf_pos(k) = 2*k - popcount(k)` (0-indexed).
//! After inserting n leaves, the number of parent merges equals
//! `n.trailing_zeros()`.

use sp_core::H256;
use storage_primitives::hash_children;

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

    /// Generate a proof for a leaf at the given index.
    pub fn proof(&self, leaf_index: u64) -> Option<MmrProof> {
        let (siblings, path, peaks) = self.proof_with_path(leaf_index)?;
        Some(MmrProof {
            leaf_index,
            siblings,
            path,
            peaks,
        })
    }

    /// Generate a proof with path bits for a leaf at the given index.
    ///
    /// Returns `(siblings, path_bits, peaks)` suitable for constructing
    /// a `storage_primitives::MmrProof`. At each level, `is_right = true`
    /// means the current node is the right child (sibling is to its left).
    pub fn proof_with_path(&self, leaf_index: u64) -> Option<(Vec<H256>, Vec<bool>, Vec<H256>)> {
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

        Some((siblings, path, self.peaks()))
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

impl Default for Mmr {
    fn default() -> Self {
        Self::new()
    }
}

/// Proof of inclusion in an MMR.
#[derive(Debug, Clone)]
pub struct MmrProof {
    /// Index of the leaf in the MMR
    pub leaf_index: u64,
    /// Sibling hashes on the path to the peak
    pub siblings: Vec<H256>,
    /// Path bits (true = current node is right child)
    pub path: Vec<bool>,
    /// Peaks of the MMR
    pub peaks: Vec<H256>,
}

#[cfg(test)]
mod tests {
    use super::*;
    use storage_primitives::blake2_256;

    /// Bagged root of `m`'s current peaks.
    fn root_of(m: &Mmr) -> H256 {
        storage_primitives::bag_peaks(&m.peaks())
    }

    /// Walk a proof to its peak and bag; test scaffolding for the local
    /// prover's proof type (the production verifier lives in primitives and
    /// takes the SCALE proof type).
    fn verify_proof(root: H256, leaf_hash: H256, proof: &MmrProof) -> bool {
        if proof.siblings.len() != proof.path.len() {
            return false;
        }
        let mut current = leaf_hash;
        for (sibling, is_right) in proof.siblings.iter().zip(proof.path.iter()) {
            current = if *is_right {
                hash_children(*sibling, current)
            } else {
                hash_children(current, *sibling)
            };
        }
        if !proof.peaks.contains(&current) {
            return false;
        }
        storage_primitives::bag_peaks(&proof.peaks) == root
    }

    #[test]
    fn test_mmr_basic() {
        let mut mmr = Mmr::new();

        let leaf1 = blake2_256(b"leaf1");
        let leaf2 = blake2_256(b"leaf2");
        let leaf3 = blake2_256(b"leaf3");

        mmr.push(leaf1);
        assert_eq!(mmr.leaf_count(), 1);

        mmr.push(leaf2);
        assert_eq!(mmr.leaf_count(), 2);

        mmr.push(leaf3);
        assert_eq!(mmr.leaf_count(), 3);

        let root = root_of(&mmr);
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

    #[test]
    fn test_mmr_proof() {
        let mut mmr = Mmr::new();

        let leaves: Vec<H256> = (0..5)
            .map(|i| blake2_256(format!("leaf{i}").as_bytes()))
            .collect();

        for leaf in &leaves {
            mmr.push(*leaf);
        }

        let root = root_of(&mmr);

        for (i, leaf) in leaves.iter().enumerate() {
            let proof = mmr.proof(i as u64).expect("proof should exist");
            assert!(
                verify_proof(root, *leaf, &proof),
                "proof should verify for leaf {i}"
            );
        }
    }

    #[test]
    fn test_proof_with_path() {
        let mut mmr = Mmr::new();

        let leaves: Vec<H256> = (0..5)
            .map(|i| blake2_256(format!("leaf{i}").as_bytes()))
            .collect();

        for leaf in &leaves {
            mmr.push(*leaf);
        }

        let root = root_of(&mmr);

        for (i, leaf) in leaves.iter().enumerate() {
            let (siblings, path, _peaks) =
                mmr.proof_with_path(i as u64).expect("proof should exist");

            // Verify via the local proof walker
            let proof = mmr.proof(i as u64).expect("proof should exist");
            assert!(
                verify_proof(root, *leaf, &proof),
                "basic proof should verify for leaf {i}"
            );

            assert_eq!(
                siblings.len(),
                path.len(),
                "siblings and path length mismatch for leaf {i}"
            );
        }
    }

    // Pins the wire format: blake2-256 of a raw chunk, the SCALE encoding of
    // `MmrLeaf`, and the bagged root for 1- and 2-leaf MMRs. A change to leaf
    // encoding or hashing would silently invalidate every commitment already
    // signed on-chain; these constants make it loud.
    #[test]
    fn mmr_wire_format_pinned() {
        use codec::Encode;
        use storage_primitives::{hash_children, MmrLeaf};

        // "hello world" (11 bytes) is a single chunk, so data_root == chunk hash.
        let h0 = blake2_256(b"hello world");
        let leaf0 = MmrLeaf {
            data_root: h0,
            data_size: 11,
            total_size: 11,
        };
        let l0 = blake2_256(&leaf0.encode());

        let mut mmr = Mmr::new();
        mmr.push(l0);

        assert_eq!(
            format!("{h0:x}"),
            "256c83b297114d201b30179f3f0ef0cace9783622da5974326b436178aeef610"
        );
        assert_eq!(
            format!("{l0:x}"),
            "ec89c4bb9c2abf33c7d090e64b3d53f3886518930c61cf9b9a0b866eff2406c9"
        );
        assert_eq!(root_of(&mmr), l0);

        // Append a second file "goodbye moon".
        let h1 = blake2_256(b"goodbye moon");
        let leaf1 = MmrLeaf {
            data_root: h1,
            data_size: 12,
            total_size: 23,
        };
        let l1 = blake2_256(&leaf1.encode());
        mmr.push(l1);

        assert_eq!(root_of(&mmr), hash_children(l0, l1));
        assert_eq!(
            format!("{:x}", root_of(&mmr)),
            "051cfeb922130ffefe7a9f68875b61fe1fa057dd90ad33d6c8c21592d9cc9c2b"
        );
    }

    // A client that keeps only the MMR peaks (~log2(n) hashes) can follow
    // appends and modify-as-append, but not a front-remove (`delete_before`
    // rebuilds the tree, so the new root is unrelated to the old peaks);
    // following removes requires the full leaf-hash list. This is the state
    // model the SDK's local root tracking is built on.
    #[test]
    fn peaks_only_client_tracks_appends_not_removes() {
        use codec::Encode;
        use storage_primitives::{blake2_256, hash_children, MmrLeaf};

        fn leaf_hash(data: &[u8], total_after: u64) -> H256 {
            let leaf = MmrLeaf {
                data_root: blake2_256(data),
                data_size: data.len() as u64,
                total_size: total_after,
            };
            blake2_256(&leaf.encode())
        }
        fn root_over(window: &[H256]) -> H256 {
            let mut m = Mmr::new();
            for &lh in window {
                m.push(lh);
            }
            root_of(&m)
        }

        // Ground truth == the minimal "leaf-hash list" client: every leaf hash
        // (32B each, NOT the data) + the window start. Enough to follow append,
        // modify-as-append, AND remove (by rebuilding over the survivors).
        struct LeafList {
            leaves: Vec<H256>,
            start: usize,
            total: u64,
        }
        impl LeafList {
            fn append(&mut self, data: &[u8]) {
                self.total += data.len() as u64;
                self.leaves.push(leaf_hash(data, self.total));
            }
            fn remove_front(&mut self, k: usize) {
                self.start += k;
            }
            fn root(&self) -> H256 {
                root_over(&self.leaves[self.start..])
            }
        }

        // Peaks-only accumulator: follows appends, but has NO way to follow a
        // front-remove (the prune rebuilds the tree; the new root is unrelated
        // to the old peaks).
        struct Peaks {
            peaks: Vec<H256>,
            leaf_count: u64,
            total: u64,
        }
        impl Peaks {
            fn append(&mut self, data: &[u8]) {
                self.total += data.len() as u64;
                let mut node = leaf_hash(data, self.total);
                self.leaf_count += 1;
                for _ in 0..self.leaf_count.trailing_zeros() {
                    node = hash_children(self.peaks.pop().unwrap(), node);
                }
                self.peaks.push(node);
            }
            fn root(&self) -> H256 {
                storage_primitives::bag_peaks(&self.peaks)
            }
        }

        let mut truth = LeafList {
            leaves: vec![],
            start: 0,
            total: 0,
        };
        let mut peaks = Peaks {
            peaks: vec![],
            leaf_count: 0,
            total: 0,
        };

        // (1) APPEND A, B, C, D
        let files: [&[u8]; 4] = [b"A", b"B", b"C", b"D"];
        for f in files {
            truth.append(f);
            peaks.append(f);
        }
        assert_eq!(peaks.root(), truth.root(), "append: both clients track");

        // (2) MODIFY B == append a new version B-v2 (old B's leaf stays; the
        //     directory layer, not modeled here, repoints /B -> B-v2).
        truth.append(b"B-v2");
        peaks.append(b"B-v2");
        assert_eq!(peaks.root(), truth.root(), "modify-as-append: both track");
        let before_delete = truth.root();

        // (3) REMOVE the two oldest leaves (A and original B) via delete_before.
        truth.remove_front(2);
        assert_eq!(
            peaks.root(),
            before_delete,
            "peaks-only is frozen at the pre-delete root"
        );
        assert_ne!(
            peaks.root(),
            truth.root(),
            "=> peaks-only is WRONG after a remove; only the leaf-hash list can rebuild"
        );
    }

    // A client holding only peaks + two counters (no old file bytes) computes
    // the exact post-append root the provider will report, and can verify
    // provider-supplied peaks against a previously trusted root via
    // `bag_peaks` before appending.
    #[test]
    fn client_computes_next_root_from_peaks() {
        use codec::Encode;
        use storage_primitives::{blake2_256, hash_children, MmrLeaf};

        // single-chunk file => data_root == blake2_256(bytes)
        fn leaf_hash(data: &[u8], total_after: u64) -> H256 {
            let leaf = MmrLeaf {
                data_root: blake2_256(data),
                data_size: data.len() as u64,
                total_size: total_after,
            };
            blake2_256(&leaf.encode())
        }

        // A minimal CLIENT-side accumulator: keeps ONLY peaks + two counters,
        // never any file bytes. `append` mirrors mmr.rs push (carry merges).
        struct Acc {
            peaks: Vec<H256>,
            leaf_count: u64,
            total: u64,
        }
        impl Acc {
            fn append(&mut self, data: &[u8]) {
                self.total += data.len() as u64;
                let mut node = leaf_hash(data, self.total); // needs ONLY new data + running total
                self.leaf_count += 1;
                for _ in 0..self.leaf_count.trailing_zeros() {
                    let left = self.peaks.pop().unwrap(); // smallest existing peak
                    node = hash_children(left, node);
                }
                self.peaks.push(node);
            }
            fn root(&self) -> H256 {
                storage_primitives::bag_peaks(&self.peaks)
            }
        }

        let a: &[u8] = b"file A";
        let b: &[u8] = b"file B";
        let c: &[u8] = b"file C";
        let d: &[u8] = b"file D (the brand new upload)";

        // Phase 1: client uploaded A, B, C earlier and folded them into peaks.
        let mut acc = Acc {
            peaks: vec![],
            leaf_count: 0,
            total: 0,
        };
        acc.append(a);
        acc.append(b);
        acc.append(c);
        let kept_peaks = acc.peaks.clone();
        let root_after_3 = acc.root();
        let kept_total = acc.total;

        // Client now DISCARDS A/B/C bytes; retains only this tiny state:
        let mut client = Acc {
            peaks: kept_peaks.clone(), // ~log2(n) hashes
            leaf_count: 3,             // a counter
            total: kept_total,         // a counter
        };

        // Phase 2: client uploads D (has only D's bytes) and computes expected root.
        client.append(d);
        let expected_root = client.root();

        // Ground truth: the real provider Mmr built over A,B,C,D from scratch.
        let mut mmr = Mmr::new();
        let mut total = 0u64;
        for f in [a, b, c, d] {
            total += f.len() as u64;
            mmr.push(leaf_hash(f, total));
        }
        let actual_root = root_of(&mmr);

        assert_eq!(
            expected_root, actual_root,
            "accumulator-only client must reproduce the full MMR root"
        );

        // If the client had kept only the OLD ROOT, it can re-fetch peaks from the
        // provider and verify them against that trusted root before appending:
        let bagged = storage_primitives::bag_peaks(&kept_peaks);
        assert_eq!(bagged, root_after_3, "bag(peaks) == trusted old root");
    }

    // Regression test for the challenge leaf-index binding fix.
    //
    // Before the fix, `verify_mmr_proof` ignored the challenged leaf_index, so a
    // provider could answer a challenge for leaf N with any other leaf it still
    // held. This test asserts that the bound `verify_mmr_proof(proof, leaf_index,
    // leaf_count, root)` accepts a leaf ONLY when it is presented at its own index.
    #[test]
    fn challenge_binds_leaf_index() {
        use codec::Encode;
        use storage_primitives::{
            verify_merkle_proof, verify_mmr_proof, MerkleProof, MmrLeaf, MmrProof,
        };

        // A bucket MMR of 6 single-chunk "files". For a single chunk,
        // data_root == blake2_256(chunk), so the chunk proof is empty and
        // verify_merkle_proof(chunk_hash, 0, [], &data_root) reduces to
        // chunk_hash == data_root.
        let files: Vec<Vec<u8>> = (0..6)
            .map(|i| format!("file {i} contents").into_bytes())
            .collect();
        let leaf_count = files.len() as u64;

        let mut mmr = Mmr::new();
        let mut leaves: Vec<MmrLeaf> = Vec::new();
        let mut total = 0u64;
        for f in &files {
            let data_root = blake2_256(f);
            let data_size = f.len() as u64;
            total += data_size;
            let leaf = MmrLeaf {
                data_root,
                data_size,
                total_size: total,
            };
            mmr.push(blake2_256(&leaf.encode()));
            leaves.push(leaf);
        }
        let root = root_of(&mmr);

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
            let chunk_hash = blake2_256(chunk_data);
            verify_merkle_proof(chunk_hash, 0, &empty, &proof.leaf.data_root)
                && verify_mmr_proof(proof, challenged_leaf, leaf_count, &root)
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
            !verify_mmr_proof(&make_proof(5), 5, leaf_count + 1, &root),
            "a mismatched leaf_count must fail"
        );
    }

    #[test]
    fn test_proof_with_path_primitives_verify() {
        use codec::Encode;

        // This test mirrors how the pallet verifies: push blake2_256(&leaf.encode())
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
            mmr.push(blake2_256(&leaf.encode()));
        }

        let root = root_of(&mmr);

        for (i, leaf) in mmr_leaves.iter().enumerate() {
            let (siblings, path, peaks) =
                mmr.proof_with_path(i as u64).expect("proof should exist");

            let mmr_proof = storage_primitives::MmrProof {
                peaks,
                leaf: leaf.clone(),
                leaf_proof: storage_primitives::MerkleProof { siblings, path },
            };

            assert!(
                storage_primitives::verify_mmr_proof(&mmr_proof, i as u64, 5, &root),
                "verify_mmr_proof failed for leaf {i}"
            );
        }
    }
}
