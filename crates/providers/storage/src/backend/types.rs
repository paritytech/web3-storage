// SPDX-License-Identifier: Apache-2.0

//! On-disk record types shared by every [`StorageBackend`] implementation.
//!
//! These are the SCALE-encoded shapes a backend writes to durable storage, kept
//! separate from the engines that store them so a second engine (SQLite, or
//! whatever comes next) persists the same records rather than inventing its own.
//!
//! # Encoding stability
//!
//! Their SCALE encoding *is* the on-disk format. Adding, removing, reordering,
//! or retyping a field - here or in any type they embed, such as
//! [`MmrLeaf`] - changes that format, and a provider restarted on data written
//! by the previous build will fail to decode it. The golden-vector tests in
//! [`tests::compatibility_tests`] pin the encoding byte-for-byte so such a change breaks the build
//! instead of a live provider; when one is intended, it needs a versioning and
//! migration story (see issue #375) alongside the new vectors.
//!
//! [`StorageBackend`]: super::StorageBackend

use codec::{Decode, Encode};
use sp_core::H256;
use std::collections::BTreeMap;
use storage_primitives::MmrLeaf;

/// One pruned-but-not-yet-erased leaf range: the retention stash keeping
/// challenges over the range provable until erasure is permitted.
#[derive(Debug, Clone, PartialEq, Encode, Decode)]
pub struct PrunedRange {
    /// Global sequence number of `leaves[0]`.
    pub first_seq: u64,
    /// The removed leaves, contiguous from `first_seq`.
    pub leaves: Vec<MmrLeaf>,
    /// The start_seq this prune advanced the bucket to.
    pub new_start_seq: u64,
}

/// An admin-signed deletion authorization: the durable evidence for the
/// on-chain `Deleted` challenge defense. Kept after the bytes are erased —
/// it is what makes the erasure permanently defensible.
#[derive(Debug, Clone, PartialEq, Eq, Encode, Decode)]
pub struct DeletionReceipt {
    /// The post-prune MMR root the admin signed.
    pub mmr_root: H256,
    /// The start_seq the deletion advanced the bucket to.
    pub new_start_seq: u64,
    /// The admin account that signed the deletion authorization.
    pub admin: sp_core::crypto::AccountId32,
    /// The admin's signature over the deletion `CommitmentPayload`.
    pub signature: sp_runtime::MultiSignature,
}

/// Per-bucket state a backend persists: the bucket's MMR, its quota usage,
/// and its deletion lifecycle (stash, receipts, condemnation).
#[derive(Debug, Clone, PartialEq, Encode, Decode)]
pub struct BucketState {
    /// Root of the MMR over `leaves`.
    pub mmr_root: H256,
    /// Sequence number of `leaves[0]`; advances as `delete_before` prunes.
    pub start_seq: u64,
    /// MMR leaves still retained, oldest first.
    pub leaves: Vec<MmrLeaf>,
    /// Bytes of node data charged against the quota.
    pub used_bytes: u64,
    /// Quota agreed on-chain for this bucket.
    pub max_bytes: u64,
    /// Pruned-but-not-yet-erased leaf ranges (the pending-erasure queue).
    pub pruned: Vec<PrunedRange>,
    /// Admin-signed deletion receipts keyed by `new_start_seq` (one per
    /// prune point), kept even after their ranges are erased — permanent
    /// evidence for the on-chain `Deleted` defense.
    pub deletion_receipts: BTreeMap<u64, DeletionReceipt>,
    /// Set when the bucket was deleted on-chain (or the agreement ended).
    /// The bucket row is removed once `leaves` and `pruned` are both empty.
    pub condemned: bool,
}

impl BucketState {
    /// An empty bucket with the given quota.
    pub fn new(max_bytes: u64) -> Self {
        Self {
            mmr_root: H256::zero(),
            start_seq: 0,
            leaves: Vec::new(),
            used_bytes: 0,
            max_bytes,
            pruned: Vec::new(),
            deletion_receipts: BTreeMap::new(),
            condemned: false,
        }
    }

    /// Number of MMR leaves currently retained.
    pub fn leaf_count(&self) -> u64 {
        self.leaves.len() as u64
    }
}

/// A stored node: a chunk (no children) or an internal Merkle node.
#[derive(Debug, Clone, PartialEq, Encode, Decode)]
pub struct StoredNode {
    /// The raw data
    pub data: Vec<u8>,
    /// Child hashes for internal nodes
    pub children: Option<Vec<H256>>,
}

#[cfg(test)]
mod tests {
    use super::*;
    use codec::DecodeAll;

    /// Pins the exact bytes these records encode to. A failure here means
    /// existing provider databases can no longer be decoded - a storage
    /// version/migration is required
    /// (see <https://github.com/paritytech/web3-storage/issues/375>).
    ///
    /// The raw keys and values a backend writes around them are pinned by that
    /// backend, e.g. `rocksdb::tests::on_disk_bytes`.
    mod compatibility_tests {
        use super::*;

        /// `value` must encode exactly to the `golden` hex, and the golden
        /// bytes must decode back to `value`.
        fn assert_golden<T: Encode + DecodeAll + PartialEq + std::fmt::Debug>(
            value: T,
            golden: &str,
        ) {
            assert_eq!(hex::encode(value.encode()), golden, "encoding changed");
            let bytes = hex::decode(golden).unwrap();
            assert_eq!(T::decode_all(&mut &bytes[..]).unwrap(), value);
        }

        #[test]
        fn bucket_state() {
            assert_golden(
                BucketState {
                    mmr_root: H256::repeat_byte(0xab),
                    start_seq: 7,
                    leaves: vec![MmrLeaf {
                        data_root: H256::repeat_byte(0xcd),
                        data_size: 111,
                        total_size: 222,
                    }],
                    used_bytes: 999,
                    max_bytes: 1_000_000,
                    pruned: Vec::new(),
                    deletion_receipts: BTreeMap::new(),
                    condemned: false,
                },
                concat!(
                    // mmr_root: H256 (0xab * 32)
                    "abababababababababababababababababababababababababababababababab",
                    // start_seq: u64 = 7 (little-endian)
                    "0700000000000000",
                    // leaves: Vec<MmrLeaf>, compact length 1
                    "04",
                    // leaves[0]: data_root (0xcd * 32), data_size = 111, total_size = 222
                    "cdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcd",
                    "6f00000000000000",
                    "de00000000000000",
                    // used_bytes: u64 = 999
                    "e703000000000000",
                    // max_bytes: u64 = 1_000_000
                    "40420f0000000000",
                    // pruned: Vec<PrunedRange>, compact length 0
                    "00",
                    // deletion_receipts: BTreeMap, compact length 0
                    "00",
                    // condemned: bool = false
                    "00",
                ),
            );
        }

        #[test]
        fn stored_node() {
            assert_golden(
                StoredNode {
                    data: vec![1, 2, 3, 4, 5],
                    children: Some(vec![H256::repeat_byte(0x11)]),
                },
                concat!(
                    // data: Vec<u8>, compact length 5, then the bytes
                    "14",
                    "0102030405",
                    // children: Option<Vec<H256>> = Some, compact length 1
                    "01",
                    "04",
                    "1111111111111111111111111111111111111111111111111111111111111111",
                ),
            );
            assert_golden(
                StoredNode {
                    data: vec![],
                    children: None,
                },
                "0000",
            );
        }

        #[test]
        fn mmr_leaf() {
            // Also hashed (`blake2_256(leaf.encode())`) to build the MMR, so a
            // layout change breaks on-chain MMR root reproducibility too.
            assert_golden(
                MmrLeaf {
                    data_root: H256::repeat_byte(0xcd),
                    data_size: 111,
                    total_size: 222,
                },
                concat!(
                    // data_root: H256 (0xcd * 32)
                    "cdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcd",
                    // data_size: u64 = 111 (little-endian)
                    "6f00000000000000",
                    // total_size: u64 = 222
                    "de00000000000000",
                ),
            );
        }

        #[test]
        fn decode_all_rejects_trailing_bytes() {
            let mut encoded = BucketState::new(1_000).encode();
            encoded.extend_from_slice(&[0xff, 0xff]);
            assert!(BucketState::decode_all(&mut &encoded[..]).is_err());

            let mut encoded = StoredNode {
                data: vec![1, 2, 3],
                children: None,
            }
            .encode();
            encoded.push(0x00);
            assert!(StoredNode::decode_all(&mut &encoded[..]).is_err());
        }
    }

    #[test]
    fn bucket_state_new_is_empty() {
        let bucket = BucketState::new(1_000);
        assert_eq!(bucket.leaf_count(), 0);
        assert_eq!(bucket.used_bytes, 0);
        assert_eq!(bucket.max_bytes, 1_000);
    }

    #[test]
    fn bucket_state_leaf_count_tracks_leaves() {
        let mut bucket = BucketState::new(1_000);
        bucket.leaves.push(MmrLeaf {
            data_root: H256::repeat_byte(0xcd),
            data_size: 111,
            total_size: 222,
        });
        assert_eq!(bucket.leaf_count(), 1);
    }
}
