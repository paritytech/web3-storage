// SPDX-License-Identifier: Apache-2.0

//! Shared primitives for Scalable Web3 Storage
//!
//! This crate contains types and structures shared between the on-chain pallet
//! and off-chain provider node.

#![cfg_attr(not(feature = "std"), no_std)]

extern crate alloc;

use alloc::vec::Vec;
use bitvec::prelude::{BitVec, Lsb0};
use codec::{Decode, DecodeWithMemTracking, Encode, MaxEncodedLen};
use core::fmt::Debug;
use scale_info::TypeInfo;
use sp_core::H256;

pub mod agreement_term;
pub mod merkle;
pub mod mmr;

pub use agreement_term::*;
pub use merkle::*;
pub use mmr::{bag_peaks, verify_mmr_proof, MmrLeaf, MmrProof, StorageMerge, LEAF_VERSION};

/// Bucket ID is a stable, unique identifier (not an index into a collection).
/// Using u64 ensures IDs never get reused even if buckets are deleted.
pub type BucketId = u64;

/// Chunk size of leaf version 1, 256 KiB (see `MmrLeaf::chunk_size`). Every
/// chunk of a version 1 data root has this size, except the last, which can be
/// shorter. The provider rejects a larger chunk on upload and replica sync.
pub const DEFAULT_CHUNK_SIZE: u32 = 256 * 1024;

/// Prime numbers used for historical root bucketing.
/// These provide logarithmic time coverage for replica sync validation.
pub const HISTORICAL_ROOT_PRIMES: [u32; 6] = [3, 7, 11, 23, 47, 113];

// ─────────────────────────────────────────────────────────────────────────────
// Roles and Membership
// ─────────────────────────────────────────────────────────────────────────────

/// Role within a bucket determining access permissions.
#[derive(
    Clone,
    Copy,
    PartialEq,
    Eq,
    Encode,
    Decode,
    DecodeWithMemTracking,
    TypeInfo,
    MaxEncodedLen,
    Debug,
)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub enum Role {
    /// Can modify members, manage settings, delete data (if not frozen)
    Admin,
    /// Can append data
    Writer,
    /// Can read data (for private buckets)
    Reader,
}

/// Whether primaries serve reads to anyone, or only to members.
///
/// The members-only restriction of `Private` is a cooperative request to
/// honest primaries, not on-chain-enforced; replicas serve everyone
/// regardless. On-chain, `Private` restricts primary challenges to bucket
/// members and primary-agreement owners.
#[derive(
    Clone,
    Copy,
    PartialEq,
    Eq,
    Default,
    Encode,
    Decode,
    DecodeWithMemTracking,
    TypeInfo,
    MaxEncodedLen,
    Debug,
)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub enum Visibility {
    /// Primaries serve reads to anyone.
    Public,
    /// Primaries serve reads only to members (Admin/Writer/Reader). The
    /// fail-safe default: creation surfaces that omit the choice must
    /// protect data, not expose it.
    #[default]
    Private,
}

// ─────────────────────────────────────────────────────────────────────────────
// Provider Types
// ─────────────────────────────────────────────────────────────────────────────

/// Provider role for a specific bucket agreement.
#[derive(
    Clone, PartialEq, Eq, Encode, Decode, DecodeWithMemTracking, TypeInfo, MaxEncodedLen, Debug,
)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub enum ProviderRole<Balance, BlockNumber> {
    /// Receives data directly from writers.
    /// - Admin-controlled (stored in bucket.primary_providers)
    /// - Count toward min_providers for checkpoints
    /// - Can be early-terminated by admin
    Primary,
    /// Syncs data from other providers autonomously.
    /// - Permissionless (anyone can add)
    /// - Does NOT count toward min_providers
    /// - Cannot be early-terminated (runs to expiry)
    /// - Receives per-sync payment from sync_balance
    Replica {
        /// Balance for per-sync payments (drawn down on each sync confirmation)
        sync_balance: Balance,
        /// Price per sync locked at creation/last extension
        sync_price: Balance,
        /// Minimum blocks between sync confirmations for this agreement.
        min_sync_interval: BlockNumber,
        /// Last confirmed sync. `None` if the replica hasn't confirmed sync yet.
        last_sync: Option<ReplicaSyncRecord<BlockNumber>>,
    },
}

/// A replica's last confirmed sync: the commitment and the anchor block.
/// `challenge_replica` targets leaves of this commitment.
#[derive(
    Clone,
    Copy,
    PartialEq,
    Eq,
    Encode,
    Decode,
    DecodeWithMemTracking,
    TypeInfo,
    MaxEncodedLen,
    Debug,
)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct ReplicaSyncRecord<BlockNumber> {
    /// MMR commitment the replica confirmed sync to (root + covered range).
    pub commitment: Commitment,
    /// Block at which `confirm_replica_sync` was executed.
    pub block: BlockNumber,
}

// ─────────────────────────────────────────────────────────────────────────────
// Agreement Types
// ─────────────────────────────────────────────────────────────────────────────

/// Action to take when ending an agreement.
#[derive(
    Clone,
    Copy,
    PartialEq,
    Eq,
    Encode,
    Decode,
    DecodeWithMemTracking,
    TypeInfo,
    MaxEncodedLen,
    Debug,
)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub enum EndAction {
    /// Pay provider in full
    Pay,
    /// Burn portion, pay rest (0-100%)
    Burn {
        /// Percentage to burn (0-100)
        burn_percent: u8,
    },
}

/// Reason for removing a primary provider from a bucket.
#[derive(
    Clone,
    Copy,
    PartialEq,
    Eq,
    Encode,
    Decode,
    DecodeWithMemTracking,
    TypeInfo,
    MaxEncodedLen,
    Debug,
)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub enum RemovalReason {
    /// Provider was slashed for failing a challenge
    Slashed,
    /// Admin terminated agreement early
    AdminTerminated,
    /// Agreement expired naturally
    Expired,
}

// ─────────────────────────────────────────────────────────────────────────────
// Challenge Types
// ─────────────────────────────────────────────────────────────────────────────

/// Challenge identifier combining deadline and index.
#[derive(
    Clone,
    Copy,
    PartialEq,
    Eq,
    Encode,
    Decode,
    DecodeWithMemTracking,
    TypeInfo,
    MaxEncodedLen,
    Debug,
)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct ChallengeId<BlockNumber> {
    /// Block by which provider must respond
    pub deadline: BlockNumber,
    /// Index within the deadline's challenge list
    pub index: u16,
}

// ─────────────────────────────────────────────────────────────────────────────
// Commitment Types
// ─────────────────────────────────────────────────────────────────────────────

/// The `(mmr_root, start_seq, leaf_count)` triplet that identifies an MMR
/// commitment over a contiguous range of leaves.
///
/// One reusable type instead of three loose fields: it is a field group inside
/// [`CommitmentPayload`], [`BucketSnapshot`], and [`ReplicaSyncRecord`], and it
/// is the single argument the checkpoint/challenge extrinsics take in place of
/// passing `mmr_root`, `start_seq`, and `leaf_count` separately.
#[derive(
    Clone,
    Copy,
    PartialEq,
    Eq,
    Encode,
    Decode,
    DecodeWithMemTracking,
    TypeInfo,
    MaxEncodedLen,
    Debug,
    Default,
)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct Commitment {
    /// Root of the MMR containing all data_roots.
    pub mmr_root: H256,
    /// Sequence number of the first leaf covered by this commitment.
    pub start_seq: u64,
    /// Number of leaves covered by this commitment.
    pub leaf_count: u64,
}

impl Commitment {
    /// Get the canonical range end (exclusive).
    pub fn range_end(&self) -> u64 {
        self.start_seq.saturating_add(self.leaf_count)
    }

    /// Check if a sequence number is within this commitment's range.
    pub fn contains_seq(&self, seq: u64) -> bool {
        seq >= self.start_seq && seq < self.range_end()
    }
}

/// The `(leaf_index, chunk_index)` pair identifying the exact chunk a challenge
/// targets: which leaf within the MMR, and which chunk within that leaf's data.
///
/// A position, distinct from [`Commitment`] (which is a *range*) — grouped so
/// the challenge extrinsics and the stored `Challenge` pass one value instead
/// of two loose `u64`s.
#[derive(
    Clone,
    Copy,
    PartialEq,
    Eq,
    Encode,
    Decode,
    DecodeWithMemTracking,
    TypeInfo,
    MaxEncodedLen,
    Debug,
    Default,
)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct ChunkLocation {
    /// Index of the challenged leaf within the MMR.
    pub leaf_index: u64,
    /// Index of the challenged chunk within the leaf's data.
    pub chunk_index: u64,
}

/// Payload that providers sign to commit to bucket state.
///
/// TODO(#316): this names a bucket but not the agreement it was signed under,
/// so a commitment outlives the obligation it attests to. Adding `agreement_id`
/// bounds validity to the agreement's lifetime, which is what #337 removed the
/// wall-clock nonce in favour of.
#[derive(
    Clone, PartialEq, Eq, Encode, Decode, DecodeWithMemTracking, TypeInfo, MaxEncodedLen, Debug,
)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct CommitmentPayload {
    /// Protocol version for future compatibility
    pub version: u8,
    /// Reference to on-chain bucket
    pub bucket_id: BucketId,
    /// MMR commitment being signed over (root + covered leaf range).
    pub commitment: Commitment,
}

impl CommitmentPayload {
    /// Current protocol version `0x1`
    pub const CURRENT_VERSION: u8 = 1;

    /// Create a new commitment payload
    pub fn new(bucket_id: BucketId, commitment: Commitment) -> Self {
        Self {
            version: Self::CURRENT_VERSION,
            bucket_id,
            commitment,
        }
    }

    /// Get the canonical range end (exclusive)
    pub fn range_end(&self) -> u64 {
        self.commitment.range_end()
    }

    /// Check if a sequence number is within this commitment's range
    pub fn contains_seq(&self, seq: u64) -> bool {
        self.commitment.contains_seq(seq)
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Snapshot Types
// ─────────────────────────────────────────────────────────────────────────────

/// Bucket snapshot representing canonical state at a checkpoint.
#[derive(Clone, PartialEq, Eq, Encode, Decode, DecodeWithMemTracking, TypeInfo, Debug)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct BucketSnapshot<BlockNumber> {
    /// Canonical MMR commitment at this checkpoint (root + covered range).
    pub commitment: Commitment,
    /// Block at which checkpointed
    pub checkpoint_block: BlockNumber,
    /// Bit i is set if `primary_providers[i]` signed. The length always equals
    /// `primary_providers.len()`: `checkpoint` builds it, adding a primary
    /// appends an unset bit, removing a primary removes its bit, and
    /// `extend_checkpoint` sets the bit of a late signer.
    pub primary_signers: BitVec<u8, Lsb0>,
}

impl<BlockNumber> BucketSnapshot<BlockNumber> {
    /// Get the canonical range end (exclusive)
    pub fn range_end(&self) -> u64 {
        self.commitment.range_end()
    }

    /// Check if a sequence number is within this snapshot's range
    pub fn contains_seq(&self, seq: u64) -> bool {
        self.commitment.contains_seq(seq)
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Hashing Utilities
// ─────────────────────────────────────────────────────────────────────────────

/// Compute blake2b-256 hash of data.
///
/// In the runtime this calls the host function, which is faster than hashing
/// in wasm. Native code hashes directly and needs no externalities.
pub fn blake2_256(data: &[u8]) -> H256 {
    sp_io::hashing::blake2_256(data).into()
}

/// Prefix of a Merkle leaf preimage: a chunk, an MMR leaf or a metadata entry.
const LEAF_PREFIX: u8 = 0x00;
/// Prefix of an internal Merkle node preimage.
const NODE_PREFIX: u8 = 0x01;
/// Prefix of the preimage that combines two MMR peaks.
pub(crate) const PEAK_PREFIX: u8 = 0x02;

/// Hash a Merkle leaf: `blake2_256(0x00 ++ data)`.
///
/// Leaves and internal nodes use different prefixes, so the bytes of an
/// internal node never hash to the same value as a leaf.
pub fn hash_leaf(data: &[u8]) -> H256 {
    let mut preimage = Vec::with_capacity(1 + data.len());
    preimage.push(LEAF_PREFIX);
    preimage.extend_from_slice(data);
    blake2_256(&preimage)
}

/// Hash two children into an internal Merkle node:
/// `blake2_256(0x01 ++ left ++ right)`.
pub fn hash_children(left: H256, right: H256) -> H256 {
    hash_pair(NODE_PREFIX, left, right)
}

/// `blake2_256(prefix ++ left ++ right)`.
pub(crate) fn hash_pair(prefix: u8, left: H256, right: H256) -> H256 {
    let mut data = [0u8; 65];
    data[0] = prefix;
    data[1..33].copy_from_slice(left.as_bytes());
    data[33..].copy_from_slice(right.as_bytes());
    blake2_256(&data)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// `BucketTarget` replaced `Option<BucketId>` in `AgreementTerms` without
    /// changing the SCALE encoding, so quotes signed before the change still
    /// verify and `transaction_version` did not change.
    #[test]
    fn bucket_target_encodes_like_option_bucket_id() {
        assert_eq!(BucketTarget::New.encode(), None::<BucketId>.encode());
        assert_eq!(
            BucketTarget::Existing(5).encode(),
            Some(5 as BucketId).encode()
        );
    }

    #[test]
    fn test_commitment_payload_range() {
        let payload = CommitmentPayload::new(
            1,
            Commitment {
                mmr_root: H256::zero(),
                start_seq: 10,
                leaf_count: 5,
            },
        );

        assert_eq!(payload.range_end(), 15);
        assert!(!payload.contains_seq(9));
        assert!(payload.contains_seq(10));
        assert!(payload.contains_seq(14));
        assert!(!payload.contains_seq(15));
    }

    /// Off-chain signers and the pallet must encode this payload identically
    /// or no signature verifies, so pin the exact bytes. The leading `0x01` and
    /// the length of 57 together record that this is the pre-`0x02` encoding
    /// restored: dropping the nonce reverted both the layout and the version.
    #[test]
    fn commitment_payload_encoding_is_byte_identical() {
        let payload = CommitmentPayload::new(
            1,
            Commitment {
                mmr_root: H256::zero(),
                start_seq: 10,
                leaf_count: 5,
            },
        );

        let mut expected = alloc::vec![1u8]; // version
        expected.extend_from_slice(&1u64.to_le_bytes()); // bucket_id
        expected.extend_from_slice(&[0u8; 32]); // mmr_root (H256::zero)
        expected.extend_from_slice(&10u64.to_le_bytes()); // start_seq
        expected.extend_from_slice(&5u64.to_le_bytes()); // leaf_count

        assert_eq!(payload.encode(), expected);
        assert_eq!(expected.len(), 57);
        assert_eq!(CommitmentPayload::CURRENT_VERSION, 1);
    }
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

    fn h(hex: &str) -> H256 {
        hex.parse().unwrap()
    }

    /// Expected values come from an independent blake2b-256 computation over
    /// the documented preimages. The TypeScript package asserts the same
    /// leaf, node and root values.
    mod golden_vectors {
        use super::*;

        const LEAF_EMPTY: &str = "03170a2e7597b7b7e3d84c05391d139a62b157e78786d8c082f29dcf4c111314";
        const LEAF_ABC: &str = "4b44b5a5f9e6fafead231e4d609a8e88053a6053c087b68e24e31faf0fb8dfe7";
        const LEAF_A: &str = "7234082e1dd0b5ec0acd71875d61c9f374af30c100bc4de7aa4eb3f15bbed686";
        const LEAF_B: &str = "b3d5dedf654e9fc853bdc5daf79330c5a1eaf2b910f2a36c72ef8ea999ccf953";
        const LEAF_C: &str = "960259f5c0885e7b7967cc25158bc9069db1ca8222e7beffdfffe5dea0297966";
        const NODE_AB: &str = "ee616625a590167bc4b3dc703ab4f3f2ddecbee6b9d05fee9281f02046e6082e";
        const ROOT_3_CHUNKS: &str =
            "17321db51c1ef3ec1f77e271aa300b4e5c6091708bcba37e46025774a26142ee";
        const MMR_LEAF: &str = "5941e24c88492304e9bf5ba943790c6fb89bbd6afc74e08603d931e5b9ac3b5b";

        #[test]
        fn leaf_hash_uses_the_leaf_prefix() {
            assert_eq!(hash_leaf(b""), h(LEAF_EMPTY));
            assert_eq!(hash_leaf(b"abc"), h(LEAF_ABC));
            assert_ne!(hash_leaf(b"abc"), blake2_256(b"abc"));
        }

        #[test]
        fn node_hash_uses_the_node_prefix() {
            assert_eq!(hash_children(h(LEAF_A), h(LEAF_B)), h(NODE_AB));
        }

        #[test]
        fn three_chunk_root_moves_the_odd_leaf_up() {
            let left = hash_children(h(LEAF_A), h(LEAF_B));
            assert_eq!(hash_children(left, h(LEAF_C)), h(ROOT_3_CHUNKS));
            assert_eq!(
                merkle_root(&[h(LEAF_A), h(LEAF_B), h(LEAF_C)]),
                h(ROOT_3_CHUNKS)
            );
        }

        #[test]
        fn mmr_leaf_hash_covers_the_encoded_leaf() {
            let leaf = MmrLeaf {
                leaf_version: LEAF_VERSION,
                data_root: H256::repeat_byte(0x11),
                data_size: 1,
                total_size: 2,
            };
            assert_eq!(hash_leaf(&leaf.encode()), h(MMR_LEAF));
        }
    }

    #[test]
    fn internal_node_bytes_do_not_verify_as_a_chunk() {
        // Four chunks: root = H(H(l0, l1), H(l2, l3)).
        let leaves: Vec<H256> = (0u8..4).map(|i| hash_leaf(&[i])).collect();
        let left = hash_children(leaves[0], leaves[1]);
        let right = hash_children(leaves[2], leaves[3]);
        let root = hash_children(left, right);

        // The honest proof for chunk 0 verifies.
        assert!(verify_merkle_proof(
            leaves[0],
            0,
            4,
            &[leaves[1], right],
            &root
        ));

        // The bytes of `left` presented as chunk data with one sibling. A
        // count of 2 makes one sibling the right length, so only the prefix
        // rejects the proof.
        let mut node_bytes = leaves[0].as_bytes().to_vec();
        node_bytes.extend_from_slice(leaves[1].as_bytes());
        assert!(!verify_merkle_proof(
            hash_leaf(&node_bytes),
            0,
            2,
            &[right],
            &root
        ));
    }
}
