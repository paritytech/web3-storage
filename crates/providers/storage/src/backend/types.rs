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
//! [`MmrLeaf`](storage_primitives::MmrLeaf) - changes that format, and a
//! provider restarted on data written by the previous build will fail to
//! decode it. The golden-vector tests in [`tests::compatibility_tests`] pin
//! the encoding byte-for-byte so such a change breaks the build instead of a
//! live provider; when one is intended, bump [`FORMAT_VERSION`] alongside the
//! new vectors.
//!
//! [`StorageBackend`]: super::StorageBackend

use crate::error::Error;
use codec::{Decode, Encode};
use sp_core::H256;
use storage_primitives::{hash_children, DEFAULT_CHUNK_SIZE};

/// Version of the on-disk encoding of the records in this module.
pub const FORMAT_VERSION: u32 = 1;

/// One pruned leaf range. Erasure deletes the range's nodes but keeps its
/// leaves: a commitment signed before the prune also covers later leaves the
/// provider still stores, and a proof for those needs the pruned leaves to
/// rebuild the commitment's root. The leaves are in the leaf store, and the
/// range is stored under `first_seq` until every leaf's nodes are deleted.
#[derive(Debug, Clone, PartialEq, Eq, Encode, Decode)]
pub struct PrunedRange {
    /// Global sequence number of `leaves[0]`.
    pub first_seq: u64,
    /// The start_seq this prune advanced the bucket to, one past the last
    /// removed leaf.
    pub new_start_seq: u64,
    /// How many leaves from the start of the range have had their nodes
    /// deleted.
    pub erased_leaves: u64,
}

impl PrunedRange {
    /// Number of leaves in the range.
    pub fn leaf_count(&self) -> u64 {
        self.new_start_seq.saturating_sub(self.first_seq)
    }

    /// Whether every leaf's nodes were deleted.
    pub fn is_erased(&self) -> bool {
        self.erased_leaves == self.leaf_count()
    }
}

/// An admin-signed deletion authorization: the durable evidence for the
/// on-chain `Deleted` challenge defense. Kept after the bytes are erased —
/// it is what makes the erasure permanently defensible. A backend stores only
/// receipts whose signature [verifies](Self::verifies).
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

impl DeletionReceipt {
    /// Whether `signature` is `admin`'s signature over the deletion commitment
    /// of `bucket_id`, verified against the `admin` account bytes. The pallet's
    /// `Deleted` defense does the same unless `admin` is a registered provider,
    /// in which case it verifies against the provider's registered key.
    pub fn verifies(&self, bucket_id: storage_primitives::BucketId) -> bool {
        use sp_runtime::traits::Verify;

        let payload = storage_primitives::CommitmentPayload::new(
            bucket_id,
            storage_primitives::Commitment {
                mmr_root: self.mmr_root,
                start_seq: self.new_start_seq,
                leaf_count: 0,
            },
        );
        self.signature.verify(&payload.encode()[..], &self.admin)
    }
}

/// Per-bucket state a backend persists: the bucket's MMR summary and its
/// quota usage. Every field has a fixed size apart from `peaks`, which has at
/// most 64 entries, so the record does not grow with the bucket's history. The
/// leaves, pruned ranges and deletion receipts are separate records keyed by
/// bucket and sequence number.
#[derive(Debug, Clone, PartialEq, Encode, Decode)]
pub struct BucketState {
    /// Root of the MMR over the retained leaves.
    pub mmr_root: H256,
    /// Sequence number of the first retained leaf; advances as `delete_before`
    /// prunes.
    pub start_seq: u64,
    /// Number of retained leaves.
    pub leaf_count: u64,
    /// Peaks of the MMR over the retained leaves, left to right.
    pub peaks: Vec<H256>,
    /// Bytes of node data charged against the quota.
    pub used_bytes: u64,
    /// Number of nodes this bucket stored.
    pub node_count: u64,
    /// Quota agreed on-chain for this bucket.
    pub max_bytes: u64,
    /// Set when the bucket was deleted on-chain (or the agreement ended).
    /// The bucket's records are removed once no retained leaf and no pruned
    /// range is left to erase.
    pub condemned: bool,
}

impl BucketState {
    /// An empty bucket with the given quota.
    pub fn new(max_bytes: u64) -> Self {
        Self {
            mmr_root: H256::zero(),
            start_seq: 0,
            leaf_count: 0,
            peaks: Vec::new(),
            used_bytes: 0,
            node_count: 0,
            max_bytes,
            condemned: false,
        }
    }

    /// Sequence number one past the last retained leaf.
    pub fn end_seq(&self) -> u64 {
        self.start_seq.saturating_add(self.leaf_count)
    }
}

/// A node in a bucket's content-addressed chunk tree: a leaf chunk, or an
/// internal node over exactly two children.
///
/// A chunk carries its data and an internal node carries only its two child
/// hashes, so a node is exactly one of the two.
#[derive(Debug, Clone, PartialEq, Encode, Decode)]
pub enum ChunkTreeNode {
    /// Leaf: hash = hash_leaf(data) = blake2_256(0x00 ++ data)
    Chunk(Vec<u8>),
    /// Internal: hash = hash_children(left, right) = blake2_256(0x01 ++ left ++ right)
    Internal {
        /// Left child.
        left: H256,
        /// Right child.
        right: H256,
    },
}

impl ChunkTreeNode {
    /// Build a node from the `data` or the `children` a peer sent.
    ///
    /// `data` alone is a chunk of at most `DEFAULT_CHUNK_SIZE` bytes. `children`
    /// alone is an internal node over exactly two child hashes. Both or neither
    /// is `InvalidNode`.
    pub fn from_wire(data: Option<Vec<u8>>, children: Option<Vec<H256>>) -> Result<Self, Error> {
        match (data, children) {
            (Some(data), None) => {
                if data.len() > DEFAULT_CHUNK_SIZE as usize {
                    return Err(Error::ChunkTooLarge {
                        size: data.len() as u64,
                    });
                }
                Ok(Self::Chunk(data))
            }
            (None, Some(children)) => {
                let count = children.len();
                let [left, right]: [H256; 2] = children
                    .try_into()
                    .map_err(|_| Error::InvalidChildCount(count))?;
                Ok(Self::Internal { left, right })
            }
            _ => Err(Error::InvalidNode),
        }
    }

    /// Bytes this node counts against the bucket quota: the data length for a
    /// chunk, 64 (two child hashes) for an internal node.
    pub fn size(&self) -> u64 {
        match self {
            Self::Chunk(data) => data.len() as u64,
            Self::Internal { .. } => 2 * H256::len_bytes() as u64,
        }
    }

    /// This node's content hash.
    pub fn hash(&self) -> H256 {
        match self {
            Self::Chunk(data) => storage_primitives::hash_leaf(data),
            Self::Internal { left, right } => hash_children(*left, *right),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use codec::DecodeAll;
    use storage_primitives::{MmrLeaf, LEAF_VERSION};

    /// Pins the exact bytes these records encode to. A failure here means
    /// existing provider databases can no longer be decoded - bump
    /// `FORMAT_VERSION`.
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
            let encoded = value.encode();
            assert_eq!(
                array_bytes::bytes2hex("", &encoded),
                golden,
                "encoding changed"
            );
            let bytes = array_bytes::hex2bytes(golden).unwrap();
            assert_eq!(T::decode_all(&mut &bytes[..]).unwrap(), value);
        }

        #[test]
        fn bucket_state() {
            assert_golden(
                BucketState {
                    mmr_root: H256::repeat_byte(0xab),
                    start_seq: 7,
                    leaf_count: 3,
                    peaks: vec![H256::repeat_byte(0x11), H256::repeat_byte(0x22)],
                    used_bytes: 999,
                    node_count: 5,
                    max_bytes: 1_000_000,
                    condemned: false,
                },
                concat!(
                    // mmr_root: H256 (0xab * 32)
                    "abababababababababababababababababababababababababababababababab",
                    // start_seq: u64 = 7 (little-endian)
                    "0700000000000000",
                    // leaf_count: u64 = 3
                    "0300000000000000",
                    // peaks: Vec<H256>, compact length 2, then the peaks
                    "08",
                    "1111111111111111111111111111111111111111111111111111111111111111",
                    "2222222222222222222222222222222222222222222222222222222222222222",
                    // used_bytes: u64 = 999
                    "e703000000000000",
                    // node_count: u64 = 5
                    "0500000000000000",
                    // max_bytes: u64 = 1_000_000
                    "40420f0000000000",
                    // condemned: bool = false
                    "00",
                ),
            );
        }

        #[test]
        fn pruned_range() {
            assert_golden(
                PrunedRange {
                    first_seq: 3,
                    new_start_seq: 4,
                    erased_leaves: 1,
                },
                concat!(
                    // first_seq: u64 = 3
                    "0300000000000000",
                    // new_start_seq: u64 = 4
                    "0400000000000000",
                    // erased_leaves: u64 = 1
                    "0100000000000000",
                ),
            );
        }

        #[test]
        fn chunk_tree_node() {
            assert_golden(
                ChunkTreeNode::Chunk(vec![1, 2, 3, 4, 5]),
                concat!(
                    // variant 0 (Chunk)
                    "00",
                    // data: Vec<u8>, compact length 5, then the bytes
                    "14",
                    "0102030405",
                ),
            );
            assert_golden(ChunkTreeNode::Chunk(vec![]), "0000");
            assert_golden(
                ChunkTreeNode::Internal {
                    left: H256::repeat_byte(0x11),
                    right: H256::repeat_byte(0x22),
                },
                concat!(
                    // variant 1 (Internal)
                    "01",
                    // left, right: H256 each, fixed-size, no length prefix
                    "1111111111111111111111111111111111111111111111111111111111111111",
                    "2222222222222222222222222222222222222222222222222222222222222222",
                ),
            );
        }

        #[test]
        fn mmr_leaf() {
            // Also hashed (`hash_leaf(leaf.encode())`) to build the MMR, so a
            // layout change breaks on-chain MMR root reproducibility too.
            assert_golden(
                MmrLeaf {
                    leaf_version: LEAF_VERSION,
                    data_root: H256::repeat_byte(0xcd),
                    data_size: 111,
                    total_size: 222,
                },
                concat!(
                    // leaf_version: u8 = 1
                    "01",
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

            let mut encoded = ChunkTreeNode::Chunk(vec![1, 2, 3]).encode();
            encoded.push(0x00);
            assert!(ChunkTreeNode::decode_all(&mut &encoded[..]).is_err());
        }
    }

    #[test]
    fn bucket_state_new_is_empty() {
        let bucket = BucketState::new(1_000);
        assert_eq!(bucket.leaf_count, 0);
        assert_eq!(bucket.used_bytes, 0);
        assert_eq!(bucket.max_bytes, 1_000);
    }

    #[test]
    fn bucket_state_end_seq_follows_the_retained_leaves() {
        let mut bucket = BucketState::new(1_000);
        bucket.start_seq = 4;
        bucket.leaf_count = 5;
        assert_eq!(bucket.end_seq(), 9);
    }

    #[test]
    fn pruned_range_is_erased_when_every_leaf_is() {
        let mut range = PrunedRange {
            first_seq: 3,
            new_start_seq: 5,
            erased_leaves: 1,
        };
        assert_eq!(range.leaf_count(), 2);
        assert!(!range.is_erased());
        range.erased_leaves = 2;
        assert!(range.is_erased());
    }

    #[test]
    fn from_wire_with_data_only_is_a_chunk() {
        assert_eq!(
            ChunkTreeNode::from_wire(Some(vec![1, 2, 3]), None).unwrap(),
            ChunkTreeNode::Chunk(vec![1, 2, 3])
        );
    }

    #[test]
    fn from_wire_rejects_a_chunk_above_default_chunk_size() {
        let max = DEFAULT_CHUNK_SIZE as usize;
        assert!(ChunkTreeNode::from_wire(Some(vec![7; max]), None).is_ok());
        assert!(matches!(
            ChunkTreeNode::from_wire(Some(vec![7; max + 1]), None),
            Err(Error::ChunkTooLarge { size }) if size == max as u64 + 1
        ));
    }

    #[test]
    fn from_wire_with_children_only_is_an_internal_node() {
        let (left, right) = (H256::repeat_byte(1), H256::repeat_byte(2));
        assert_eq!(
            ChunkTreeNode::from_wire(None, Some(vec![left, right])).unwrap(),
            ChunkTreeNode::Internal { left, right }
        );
    }

    #[test]
    fn from_wire_rejects_wrong_child_count() {
        let child = H256::repeat_byte(1);
        for children in [vec![], vec![child], vec![child, child, child]] {
            let count = children.len();
            let err = ChunkTreeNode::from_wire(None, Some(children)).unwrap_err();
            assert!(matches!(err, Error::InvalidChildCount(n) if n == count));
        }
    }

    #[test]
    fn from_wire_rejects_both_or_neither_of_data_and_children() {
        let (left, right) = (H256::repeat_byte(1), H256::repeat_byte(2));
        assert!(matches!(
            ChunkTreeNode::from_wire(Some(vec![1]), Some(vec![left, right])),
            Err(Error::InvalidNode)
        ));
        assert!(matches!(
            ChunkTreeNode::from_wire(None, None),
            Err(Error::InvalidNode)
        ));
    }

    #[test]
    fn chunk_size_and_hash_come_from_its_data() {
        let node = ChunkTreeNode::Chunk(vec![1, 2, 3]);
        assert_eq!(node.size(), 3);
        assert_eq!(node.hash(), storage_primitives::hash_leaf(&[1, 2, 3]));
    }

    #[test]
    fn internal_size_and_hash_come_from_its_children() {
        let left = H256::repeat_byte(0x11);
        let right = H256::repeat_byte(0x22);
        let node = ChunkTreeNode::Internal { left, right };

        assert_eq!(node.size(), 64);
        assert_eq!(node.hash(), hash_children(left, right));
    }
}
