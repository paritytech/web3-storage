// SPDX-License-Identifier: Apache-2.0

//! The bucket MMR, built in memory from a bucket's leaves with
//! `polkadot-ckb-merkle-mountain-range`. Nothing is persisted: every call
//! rebuilds the MMR from the leaves it is given.

use crate::error::Error;
use codec::Encode;
use polkadot_ckb_merkle_mountain_range::{self as mmr_lib, util::MemStore, MMR};
use sp_core::H256;
use storage_primitives::{hash_leaf, MmrLeaf, MmrProof, StorageMerge};

type LeafMmr<'a> = MMR<H256, StorageMerge, &'a MemStore<H256>>;

/// The MMR over `leaves`, with its nodes in `store`.
fn build<'a>(store: &'a MemStore<H256>, leaves: &[MmrLeaf]) -> Result<LeafMmr<'a>, Error> {
    let mut mmr = LeafMmr::new(0, store);
    for leaf in leaves {
        mmr.push(hash_leaf(&leaf.encode()))?;
    }
    Ok(mmr)
}

/// Root of the MMR over `leaves`. No leaves give the zero root.
pub(super) fn mmr_root(leaves: &[MmrLeaf]) -> Result<H256, Error> {
    Ok(mmr_peaks(leaves)?.0)
}

/// Root and peaks (left to right) of the MMR over `leaves`. No leaves give
/// the zero root and no peaks.
pub(super) fn mmr_peaks(leaves: &[MmrLeaf]) -> Result<(H256, Vec<H256>), Error> {
    if leaves.is_empty() {
        return Ok((H256::zero(), Vec::new()));
    }
    let store = MemStore::default();
    let mmr = build(&store, leaves)?;
    let (peaks, root) = mmr.get_ancestor_peaks_and_root(mmr.mmr_size())?;
    Ok((root, peaks))
}

/// Root of the MMR over `leaves` and the proof for the leaf at `leaf_index`.
pub(super) fn mmr_proof(leaves: &[MmrLeaf], leaf_index: u64) -> Result<(H256, MmrProof), Error> {
    let leaf = leaves
        .get(leaf_index as usize)
        .ok_or(Error::NodeNotFound(format!("leaf_{leaf_index}")))?
        .clone();

    let store = MemStore::default();
    let mmr = build(&store, leaves)?;
    let root = mmr.get_root()?;
    let items = mmr
        .gen_proof(vec![mmr_lib::leaf_index_to_pos(leaf_index)])?
        .proof_items()
        .to_vec();

    Ok((root, MmrProof { leaf, items }))
}

#[cfg(test)]
mod tests {
    use super::*;
    use storage_primitives::{bag_peaks, Commitment};

    fn leaf(i: u64) -> MmrLeaf {
        MmrLeaf {
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

    /// Roots of the same leaves computed independently of both the library
    /// and `bag_peaks`: blake2b-256 over `0x00 ++ SCALE(leaf)` for leaves,
    /// `0x01 ++ left ++ right` for nodes and `0x02 ++ peak ++ rest` when
    /// bagging peaks from right to left.
    #[test]
    fn roots_match_independently_computed_values() {
        let expected = [
            (
                1,
                "0x492a3842089489aa28f0fe1588d7650b853cdb731be096d851211f084c8194db",
            ),
            (
                2,
                "0x9e3fe480f4f2facf999ab0642e8fb29eecfb559fac575bd3ade8ada6d78a5e9d",
            ),
            (
                3,
                "0x8b526e54af4123d6109cc800d29933849860c451cc3864afae832cd5b656bb51",
            ),
            (
                5,
                "0x43e36aab7e51c967b222311383bab2932a2342afa1888d839584f79ccfc5ebab",
            ),
            (
                8,
                "0x11be9655168f9f2ed092b07a70d75446b2443eea897a4fc8092294d9af4212e3",
            ),
            (
                33,
                "0xf2a0ab0144b76bfa739a49df9f07c5f6d31e06d1faa5f8b3e2ac160e374a6a78",
            ),
        ];
        for (count, root) in expected {
            assert_eq!(mmr_root(&leaves(count)).unwrap(), h(root), "{count} leaves");
        }
    }

    #[test]
    fn empty_mmr_has_zero_root_and_no_peaks() {
        assert_eq!(mmr_root(&[]).unwrap(), H256::zero());
        assert_eq!(mmr_peaks(&[]).unwrap(), (H256::zero(), Vec::new()));
    }

    #[test]
    fn peaks_bag_to_the_root() {
        for count in 1..=33 {
            let (root, peaks) = mmr_peaks(&leaves(count)).unwrap();
            assert_eq!(root, mmr_root(&leaves(count)).unwrap());
            assert_eq!(peaks.len(), count.count_ones() as usize);
            assert_eq!(bag_peaks(&peaks), root, "{count} leaves");
        }
    }

    #[test]
    fn proofs_verify_for_every_leaf() {
        for leaf_count in 1..=33u64 {
            let all = leaves(leaf_count);
            let commitment = Commitment {
                mmr_root: mmr_root(&all).unwrap(),
                start_seq: 0,
                leaf_count,
            };
            for leaf_index in 0..leaf_count {
                let (root, proof) = mmr_proof(&all, leaf_index).unwrap();
                assert_eq!(root, commitment.mmr_root);
                assert_eq!(proof.leaf, all[leaf_index as usize]);
                assert!(
                    storage_primitives::verify_mmr_proof(&proof, leaf_index, &commitment),
                    "leaf {leaf_index} of {leaf_count}"
                );
            }
        }
    }

    #[test]
    fn proof_for_a_missing_leaf_is_an_error() {
        let all = leaves(4);
        assert!(matches!(mmr_proof(&all, 4), Err(Error::NodeNotFound(_))));
        assert!(matches!(mmr_proof(&[], 0), Err(Error::NodeNotFound(_))));
    }
}
