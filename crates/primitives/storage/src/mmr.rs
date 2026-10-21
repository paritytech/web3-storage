// SPDX-License-Identifier: Apache-2.0

//! The bucket MMR: its leaf and proof types, proof verification and, with the
//! `std` feature, a builder that rebuilds the MMR in memory from a bucket's
//! leaves on every call.

use crate::{hash_children, hash_leaf, hash_pair, Commitment, PEAK_PREFIX};
use alloc::vec::Vec;
use codec::{Decode, DecodeWithMemTracking, Encode, MaxEncodedLen};
#[cfg(feature = "std")]
use mmr_lib::util::{MemMMR, MemStore};
use polkadot_ckb_merkle_mountain_range::{self as mmr_lib, Merge};
use scale_info::TypeInfo;
use sp_core::H256;

/// Error of the MMR library.
pub use mmr_lib::Error;

/// The `MmrLeaf` format this build writes into every leaf it commits.
///
/// A verifier reads `leaf_version` first and checks the rest of the leaf under
/// that version's rules: its fields, its chunk size and the chunk tree shape.
/// A format change bumps this constant; leaves of different versions share one
/// MMR and each verifies under its own version.
pub const LEAF_VERSION: u8 = 1;

/// MMR leaf containing data root and size information.
#[derive(
    Clone, PartialEq, Eq, Encode, Decode, DecodeWithMemTracking, TypeInfo, MaxEncodedLen, Debug,
)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct MmrLeaf {
    /// Leaf format version, see [`LEAF_VERSION`]. First field, so the leaf
    /// hash preimage is `0x00 ++ leaf_version ++ ...`.
    pub leaf_version: u8,
    /// Merkle root of chunk tree
    pub data_root: H256,
    /// Size of content under this data_root
    pub data_size: u64,
    /// Cumulative unique bytes in MMR at this point
    pub total_size: u64,
}

impl MmrLeaf {
    /// Chunk size of this leaf's version. `None` for an unknown version.
    pub fn chunk_size(&self) -> Option<u32> {
        match self.leaf_version {
            1 => Some(crate::DEFAULT_CHUNK_SIZE),
            _ => None,
        }
    }

    /// Number of chunks under `data_root`: every chunk has this leaf's chunk
    /// size except the last, which has 1 to chunk size bytes. Empty data has
    /// no chunks. `None` for an unknown version.
    pub fn chunk_count(&self) -> Option<u64> {
        let chunk_size = u64::from(self.chunk_size()?);
        Some(self.data_size.div_ceil(chunk_size))
    }
}

/// MMR proof for verifying leaf inclusion.
///
/// The proof does not contain the leaf index or the MMR size: the verifier
/// takes both from the challenge and its commitment (see [`verify_mmr_proof`]).
#[derive(Clone, PartialEq, Eq, Encode, Decode, DecodeWithMemTracking, TypeInfo, Debug)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct MmrProof {
    /// The leaf being proven
    pub leaf: MmrLeaf,
    /// Proof items: the peaks left of the leaf's peak, the siblings from the
    /// leaf up to its peak, then one hash of the bagged peaks to its right (see
    /// the `MmrProof` section of the implementation design doc).
    pub items: Vec<H256>,
}

/// Combine MMR peaks into the MMR root.
///
/// Peaks fold from right to left with `blake2_256(0x02 ++ peak ++ rest)`. One
/// peak is the root itself and no peaks give the zero root.
pub fn bag_peaks(peaks: &[H256]) -> H256 {
    mmr_lib::ancestry_proof::bagging_peaks_hashes::<H256, StorageMerge>(peaks.to_vec())
        .unwrap_or_default()
}

/// Combine a peak with the bagged peaks to its right:
/// `blake2_256(0x02 ++ left ++ right)`.
fn hash_peaks(left: H256, right: H256) -> H256 {
    hash_pair(PEAK_PREFIX, left, right)
}

/// [`Merge`] for `polkadot-ckb-merkle-mountain-range` that builds the same
/// MMR roots as [`hash_children`] and [`bag_peaks`].
pub struct StorageMerge;

impl Merge for StorageMerge {
    type Item = H256;

    fn merge(left: &H256, right: &H256) -> mmr_lib::Result<H256> {
        Ok(hash_children(*left, *right))
    }

    /// The library bags peaks from right to left and passes the right-hand
    /// value first.
    fn merge_peaks(right: &H256, left: &H256) -> mmr_lib::Result<H256> {
        Ok(hash_peaks(*left, *right))
    }
}

/// Verify that `proof` proves leaf `leaf_index` of `commitment`.
///
/// `commitment.leaf_count` fixes the MMR shape and with it the number of
/// proof items. If `commitment.mmr_root` is the root of an MMR with
/// `leaf_count` leaves, a proof for another leaf, or for an MMR of another
/// size, fails. A commitment with more than `u64::MAX / 2` leaves fails
/// every proof.
pub fn verify_mmr_proof(proof: MmrProof, leaf_index: u64, commitment: &Commitment) -> bool {
    // Reject `leaf_count > u64::MAX / 2`: `leaf_index_to_mmr_size` computes
    // `2 * leaf_count`, which overflows above that.
    if leaf_index >= commitment.leaf_count || commitment.leaf_count > u64::MAX / 2 {
        return false;
    }
    let mmr_size = mmr_lib::leaf_index_to_mmr_size(commitment.leaf_count - 1);
    let leaf_pos = mmr_lib::leaf_index_to_pos(leaf_index);
    // The library also accepts proofs with fewer or more items for the peaks
    // right of the leaf's peak, and ignores the items of a 1-leaf MMR.
    if Some(proof.items.len()) != mmr_proof_len(leaf_pos, mmr_size) {
        return false;
    }

    let leaf = (leaf_pos, hash_leaf(&proof.leaf.encode()));
    mmr_lib::MerkleProof::<H256, StorageMerge>::new(mmr_size, proof.items)
        .verify(commitment.mmr_root, alloc::vec![leaf])
        .unwrap_or(false)
}

/// Number of items in the proof for the leaf at `leaf_pos` of an MMR of
/// `mmr_size` nodes: one per peak to the left, one per level of the leaf's
/// peak, and one for the peaks to the right. `None` if no peak covers
/// `leaf_pos`.
fn mmr_proof_len(leaf_pos: u64, mmr_size: u64) -> Option<usize> {
    use mmr_lib::helper::{get_peaks, pos_height_in_tree};

    let peaks = get_peaks(mmr_size);
    let peak = peaks.iter().position(|pos| *pos >= leaf_pos)?;
    let height = usize::from(pos_height_in_tree(peaks[peak]));
    Some(peak + height + usize::from(peak + 1 < peaks.len()))
}

#[cfg(feature = "std")]
type LeafMmr<'a> = MemMMR<'a, H256, StorageMerge>;

/// The MMR over `leaves`, with its nodes in `store`.
#[cfg(feature = "std")]
fn build<'a>(store: &'a MemStore<H256>, leaves: &[MmrLeaf]) -> Result<LeafMmr<'a>, Error> {
    let mut mmr = LeafMmr::new(0, store);
    for leaf in leaves {
        mmr.push(hash_leaf(&leaf.encode()))?;
    }
    Ok(mmr)
}

/// Root of the MMR over `leaves`. No leaves give the zero root.
#[cfg(feature = "std")]
pub fn root(leaves: &[MmrLeaf]) -> Result<H256, Error> {
    Ok(peaks(leaves)?.0)
}

/// Root and peaks (left to right) of the MMR over `leaves`. No leaves give
/// the zero root and no peaks.
#[cfg(feature = "std")]
pub fn peaks(leaves: &[MmrLeaf]) -> Result<(H256, Vec<H256>), Error> {
    if leaves.is_empty() {
        return Ok((H256::zero(), Vec::new()));
    }
    let store = MemStore::default();
    let mmr = build(&store, leaves)?;
    let (peaks, root) = mmr.get_ancestor_peaks_and_root(mmr.mmr_size())?;
    Ok((root, peaks))
}

/// Root of the MMR over `leaves` and the proof for the leaf at `leaf_index`.
/// `None` if there is no leaf at `leaf_index`.
#[cfg(feature = "std")]
pub fn proof(leaves: &[MmrLeaf], leaf_index: u64) -> Result<Option<(H256, MmrProof)>, Error> {
    let Some(leaf) = usize::try_from(leaf_index)
        .ok()
        .and_then(|index| leaves.get(index))
    else {
        return Ok(None);
    };

    let store = MemStore::default();
    let mmr = build(&store, leaves)?;
    let root = mmr.get_root()?;
    let items = mmr
        .gen_proof(alloc::vec![mmr_lib::leaf_index_to_pos(leaf_index)])?
        .proof_items()
        .to_vec();

    Ok(Some((
        root,
        MmrProof {
            leaf: leaf.clone(),
            items,
        },
    )))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn leaf(i: u64) -> MmrLeaf {
        MmrLeaf {
            leaf_version: LEAF_VERSION,
            data_root: H256::repeat_byte(i as u8 + 1),
            data_size: i,
            total_size: 2 * i,
        }
    }

    fn leaves(count: u64) -> Vec<MmrLeaf> {
        (0..count).map(leaf).collect()
    }

    fn h(hex: &str) -> H256 {
        hex.parse().unwrap()
    }

    #[test]
    fn chunk_count_uses_the_chunk_size_of_the_leaf_version() {
        let chunk = crate::DEFAULT_CHUNK_SIZE as u64;
        let with_size = |data_size| MmrLeaf {
            data_size,
            ..leaf(0)
        };
        assert_eq!(with_size(0).chunk_size(), Some(crate::DEFAULT_CHUNK_SIZE));
        assert_eq!(with_size(0).chunk_count(), Some(0));
        assert_eq!(with_size(1).chunk_count(), Some(1));
        assert_eq!(with_size(chunk).chunk_count(), Some(1));
        assert_eq!(with_size(chunk + 1).chunk_count(), Some(2));
        assert_eq!(with_size(2 * chunk).chunk_count(), Some(2));
        assert_eq!(with_size(2 * chunk + 10).chunk_count(), Some(3));

        let unknown = MmrLeaf {
            leaf_version: LEAF_VERSION + 1,
            ..with_size(chunk)
        };
        assert_eq!(unknown.chunk_size(), None);
        assert_eq!(unknown.chunk_count(), None);
    }

    /// Roots of the same leaves computed independently of both the library
    /// and `bag_peaks`: blake2b-256 over `0x00 ++ SCALE(leaf)` for leaves,
    /// `0x01 ++ left ++ right` for nodes and `0x02 ++ peak ++ rest` when
    /// bagging peaks from right to left.
    #[test]
    fn roots_match_independently_computed_values() {
        let expected = [
            (
                1,
                "0xb8313af17d8c37fcd9be77773ae701b0804a13ef03f07275fc17177a515e814c",
            ),
            (
                2,
                "0xd96aa7a3e6a79256d673d1edb6ad575027fe10a5198315890f4c080f071bf061",
            ),
            (
                3,
                "0xb03e69026d561142a025323fc22412b887c55bd33aa2a00df1f45158573c20bc",
            ),
            (
                5,
                "0x392587284baaec20f8b5341c1313e5ee1f48e802dc65c7e480dca35d063abf86",
            ),
            (
                8,
                "0xbb5a199cf97a8c7baea198b4ec4436e1d2fb9e346603ae9dcc367639932f388e",
            ),
            (
                33,
                "0x38e22e5fd9000bbf2bc63d7d3e6b687d662e59d95190377088426f85833ce0f8",
            ),
        ];
        for (count, expected_root) in expected {
            assert_eq!(
                root(&leaves(count)).unwrap(),
                h(expected_root),
                "{count} leaves"
            );
        }
    }

    #[test]
    fn empty_mmr_has_zero_root_and_no_peaks() {
        assert_eq!(root(&[]).unwrap(), H256::zero());
        assert_eq!(peaks(&[]).unwrap(), (H256::zero(), Vec::new()));
    }

    #[test]
    fn peaks_bag_to_the_root() {
        for count in 1..=33 {
            let (mmr_root, mmr_peaks) = peaks(&leaves(count)).unwrap();
            assert_eq!(mmr_root, root(&leaves(count)).unwrap());
            assert_eq!(mmr_peaks.len(), count.count_ones() as usize);
            assert_eq!(bag_peaks(&mmr_peaks), mmr_root, "{count} leaves");
        }
    }

    /// Expected values come from an independent blake2b-256 computation over
    /// `0x02 ++ peak ++ rest`, folding from the right.
    /// `packages/core/src/merkle.test.ts` asserts the same values.
    #[test]
    fn peaks_bag_right_to_left_with_the_peak_prefix() {
        let peaks: Vec<H256> = (1..=3).map(H256::repeat_byte).collect();
        assert_eq!(
            bag_peaks(&peaks[..2]),
            h("39ffd1718a7a2d0f61de8f7902cb47d9661392d004d5b91e835ccd3d475cd035")
        );
        assert_eq!(
            bag_peaks(&peaks),
            h("de6b5def5927714c7e957d45b39ba6b3cb1b24c08d3d72e038387f46268eb23f")
        );
    }

    #[test]
    fn one_peak_is_the_root_and_no_peaks_give_zero() {
        let peak = H256::repeat_byte(7);
        assert_eq!(bag_peaks(&[peak]), peak);
        assert_eq!(bag_peaks(&[]), H256::zero());
    }

    #[test]
    fn proof_for_a_missing_leaf_is_none() {
        assert_eq!(proof(&leaves(4), 4).unwrap(), None);
        assert_eq!(proof(&[], 0).unwrap(), None);
        assert_eq!(proof(&leaves(4), u64::MAX).unwrap(), None);
    }

    /// Build the MMR over `leaf_count` test leaves. Returns its commitment
    /// and the proof for the leaf at `index`.
    fn mmr_commitment_and_proof(leaf_count: u64, index: u64) -> (Commitment, MmrProof) {
        let (mmr_root, mmr_proof) = proof(&leaves(leaf_count), index).unwrap().unwrap();
        (
            Commitment {
                mmr_root,
                start_seq: 0,
                leaf_count,
            },
            mmr_proof,
        )
    }

    /// A proof verifies for its own leaf index and for no other.
    #[test]
    fn mmr_proof_verifies_only_for_its_leaf_index() {
        for leaf_count in 1u64..=33 {
            let all = leaves(leaf_count);
            for index in 0..leaf_count {
                let (commitment, proof) = mmr_commitment_and_proof(leaf_count, index);
                assert_eq!(
                    commitment.mmr_root,
                    root(&all).unwrap(),
                    "leaf {index} of {leaf_count}"
                );
                assert_eq!(
                    proof.leaf, all[index as usize],
                    "leaf {index} of {leaf_count}"
                );
                for other in 0..leaf_count {
                    assert_eq!(
                        verify_mmr_proof(proof.clone(), other, &commitment),
                        other == index,
                        "proof for leaf {index} checked as leaf {other} of {leaf_count}"
                    );
                }
            }
        }
    }

    /// The commitment of another MMR size over the same leaves rejects the
    /// proof.
    #[test]
    fn mmr_proof_does_not_verify_with_another_leaf_count() {
        // `commitments[n - 1]` is the commitment of the MMR with `n` leaves.
        let commitments: Vec<Commitment> =
            (1..=17).map(|n| mmr_commitment_and_proof(n, 0).0).collect();
        for leaf_count in 1u64..=17 {
            for index in 0..leaf_count {
                let (_, proof) = mmr_commitment_and_proof(leaf_count, index);
                for other in (index + 1..=17).filter(|other| *other != leaf_count) {
                    assert!(
                        !verify_mmr_proof(proof.clone(), index, &commitments[other as usize - 1]),
                        "proof for leaf {index} of {leaf_count} verified in {other} leaves"
                    );
                }
            }
        }
    }

    #[test]
    fn mmr_proof_len_matches_the_library() {
        for leaf_count in 1u64..=65 {
            for index in 0..leaf_count {
                let (_, proof) = mmr_commitment_and_proof(leaf_count, index);
                assert_eq!(
                    mmr_proof_len(
                        mmr_lib::leaf_index_to_pos(index),
                        mmr_lib::leaf_index_to_mmr_size(leaf_count - 1)
                    ),
                    Some(proof.items.len()),
                    "leaf {index} of {leaf_count}"
                );
            }
        }
        // The largest MMR `verify_mmr_proof` accepts: 63 peaks.
        let leaf_count = u64::MAX / 2;
        let mmr_size = mmr_lib::leaf_index_to_mmr_size(leaf_count - 1);
        let len = |index| mmr_proof_len(mmr_lib::leaf_index_to_pos(index), mmr_size);
        assert_eq!(len(0), Some(63));
        assert_eq!(len(leaf_count - 1), Some(62));
    }

    #[test]
    fn tampered_mmr_proof_does_not_verify() {
        let (commitment, proof) = mmr_commitment_and_proof(7, 2);
        assert!(verify_mmr_proof(proof.clone(), 2, &commitment));

        let mut wrong_leaf = proof.clone();
        wrong_leaf.leaf.data_size += 1;
        assert!(!verify_mmr_proof(wrong_leaf, 2, &commitment));

        let mut wrong_item = proof.clone();
        wrong_item.items[0] = H256::repeat_byte(0x44);
        assert!(!verify_mmr_proof(wrong_item, 2, &commitment));

        let mut missing_item = proof.clone();
        missing_item.items.pop();
        assert!(!verify_mmr_proof(missing_item, 2, &commitment));

        let mut extra_item = proof.clone();
        extra_item.items.push(H256::repeat_byte(0x55));
        assert!(!verify_mmr_proof(extra_item, 2, &commitment));

        // A 1-leaf MMR takes no items.
        let (single, mut single_proof) = mmr_commitment_and_proof(1, 0);
        assert!(verify_mmr_proof(single_proof.clone(), 0, &single));
        single_proof.items.push(H256::repeat_byte(0x55));
        assert!(!verify_mmr_proof(single_proof, 0, &single));

        let wrong_root = Commitment {
            mmr_root: H256::repeat_byte(0x66),
            ..commitment
        };
        assert!(!verify_mmr_proof(proof.clone(), 2, &wrong_root));
    }

    /// The library accepts the peaks right of the leaf's peak as separate
    /// items instead of one bagged item. The item count check rejects that
    /// form, so every accepted proof costs at most the benchmarked hashing.
    #[test]
    fn mmr_proof_with_unbagged_right_peaks_does_not_verify() {
        // 7 leaves: peaks over leaves 0..4, 4..6 and 6.
        let (commitment, proof) = mmr_commitment_and_proof(7, 2);
        let (_, last) = mmr_commitment_and_proof(7, 6);
        let middle_peak = last.items[1];
        let last_peak = hash_leaf(&last.leaf.encode());

        let mut unbagged = proof.clone();
        unbagged.items.truncate(2);
        unbagged.items.extend([middle_peak, last_peak]);
        assert_eq!(unbagged.items.len(), proof.items.len() + 1);
        assert!(!verify_mmr_proof(unbagged, 2, &commitment));
    }

    /// The largest MMR `verify_mmr_proof` accepts (`u64::MAX / 2` leaves, 63
    /// peaks of heights 62 down to 0): the first and the last leaf of a peak
    /// of every height verify with a synthetic proof, and a changed first or
    /// last item fails.
    #[test]
    fn mmr_proof_verifies_at_the_largest_leaf_count() {
        let leaf_count = u64::MAX / 2;
        let leaf = leaf(1);
        for height in [0u32, 1, 31, 61, 62] {
            // Peaks taller than `height` are to the left of the leaf's peak.
            let left_peaks: Vec<H256> = (0..62 - height)
                .map(|i| H256::from_low_u64_be(u64::from(i) + 1))
                .collect();
            let first_leaf = leaf_count & !((2u64 << height) - 1);
            let siblings: Vec<H256> = (0..height)
                .map(|i| H256::repeat_byte(i as u8 + 1))
                .collect();
            let right_peaks = (height > 0).then(|| H256::repeat_byte(0xFF));

            // The first leaf of a peak is a left child at every level, the
            // last leaf a right child.
            for (leaf_index, is_right) in
                [(first_leaf, false), (first_leaf + (1 << height) - 1, true)]
            {
                let peak = siblings
                    .iter()
                    .fold(hash_leaf(&leaf.encode()), |node, sibling| {
                        if is_right {
                            hash_children(*sibling, node)
                        } else {
                            hash_children(node, *sibling)
                        }
                    });
                let mut peaks = left_peaks.clone();
                peaks.push(peak);
                peaks.extend(right_peaks);
                let commitment = Commitment {
                    mmr_root: bag_peaks(&peaks),
                    start_seq: 0,
                    leaf_count,
                };
                let mut items = left_peaks.clone();
                items.extend(siblings.iter().copied());
                items.extend(right_peaks);
                let proof = MmrProof {
                    leaf: leaf.clone(),
                    items,
                };

                assert!(
                    verify_mmr_proof(proof.clone(), leaf_index, &commitment),
                    "leaf {leaf_index} in the height-{height} peak"
                );
                if proof.items.is_empty() {
                    continue;
                }
                for changed in [0, proof.items.len() - 1] {
                    let mut tampered = proof.clone();
                    tampered.items[changed] = H256::repeat_byte(0xEE);
                    assert!(
                        !verify_mmr_proof(tampered, leaf_index, &commitment),
                        "leaf {leaf_index}, item {changed} changed"
                    );
                }
            }
        }
    }

    #[test]
    fn mmr_proof_rejects_invalid_leaf_position_without_panicking() {
        let (commitment, proof) = mmr_commitment_and_proof(4, 1);

        assert!(!verify_mmr_proof(proof.clone(), 4, &commitment));
        assert!(!verify_mmr_proof(proof.clone(), u64::MAX, &commitment));

        for leaf_count in [0, u64::MAX / 2 + 1, u64::MAX] {
            let resized = Commitment {
                leaf_count,
                ..commitment
            };
            assert!(
                !verify_mmr_proof(proof.clone(), 0, &resized),
                "{leaf_count} leaves"
            );
        }
    }
}
