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
use storage_primitives::MmrLeaf;

/// Per-bucket state a backend persists: the bucket's MMR and its quota usage.
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
