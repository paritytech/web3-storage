// SPDX-License-Identifier: Apache-2.0

//! Shared primitives for Scalable Web3 Storage
//!
//! This crate contains types and structures shared between the on-chain pallet
//! and off-chain provider node.

#![cfg_attr(not(feature = "std"), no_std)]

extern crate alloc;

use alloc::vec::Vec;
use codec::{Decode, DecodeWithMemTracking, Encode, MaxEncodedLen};
use core::fmt::Debug;
use polkadot_ckb_merkle_mountain_range::{self as mmr_lib, Merge};
use scale_info::TypeInfo;
use sp_core::H256;

pub mod agreement_term;
pub mod merkle;

pub use agreement_term::*;
pub use merkle::*;

/// Bucket ID is a stable, unique identifier (not an index into a collection).
/// Using u64 ensures IDs never get reused even if buckets are deleted.
pub type BucketId = u64;

/// Chunk size, 256 KiB. Every chunk of a committed data root has this size,
/// except the last, which can be shorter. The runtime uses it as
/// `MaxChunkSize`. The provider rejects a larger chunk on upload and replica
/// sync.
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
        /// Last confirmed sync. None if replica hasn't confirmed sync yet.
        last_sync: Option<ReplicaSyncRecord<BlockNumber>>,
    },
}

/// Snapshot metadata captured at a replica's `confirm_replica_sync` so the
/// pallet can later challenge a specific leaf without going back to the
/// historical_roots table (which doesn't store sequence metadata).
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
// MMR and Merkle Types
// ─────────────────────────────────────────────────────────────────────────────

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
    /// The leaf being proven
    pub leaf: MmrLeaf,
    /// Proof hashes in the order `polkadot-ckb-merkle-mountain-range`
    /// produces: one per peak to the left of the leaf's peak, then the
    /// siblings from the leaf up to its peak, then one item for all peaks to
    /// the right (the bagged peaks if there are several). The leaf's own peak
    /// is not included, because the verifier computes it.
    pub items: Vec<H256>,
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
    /// Bitfield indicating which primary providers signed this snapshot.
    /// Bit i is set if primary_providers[i] signed.
    /// Uses Vec<u8> with LSB0 ordering for efficient bit manipulation.
    pub primary_signers: Vec<u8>,
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

    /// Check if a provider at the given index has signed this snapshot
    pub fn has_provider_signed(&self, provider_index: usize) -> bool {
        let byte_index = provider_index / 8;
        let bit_index = provider_index % 8;
        self.primary_signers
            .get(byte_index)
            .map(|byte| (byte & (1 << bit_index)) != 0)
            .unwrap_or(false)
    }

    /// Re-index the signer bitfield after the provider at `idx` is removed
    /// from `primary_providers`.
    ///
    /// The bitfield is positional: bit `i` corresponds to
    /// `primary_providers[i]`. Removing element `idx` shifts every later
    /// provider down one index, so the bits must shift to match: new bit `j`
    /// is old bit `j` for `j < idx`, and old bit `j + 1` for `j >= idx` (i.e.
    /// bit `idx` is dropped and all higher bits shift down by one).
    pub fn remove_provider_bit(&mut self, idx: usize) {
        let total_bits = self.primary_signers.len().saturating_mul(8);
        // Unpack into individual bits (LSB-first within each byte, matching
        // how `has_provider_signed` reads them).
        let mut bits: Vec<bool> = (0..total_bits)
            .map(|i| self.has_provider_signed(i))
            .collect();
        if idx < bits.len() {
            bits.remove(idx);
        }
        // Repack into the minimal Vec<u8>, dropping trailing all-zero bytes.
        let byte_len = bits.len().div_ceil(8);
        let mut bytes = alloc::vec![0u8; byte_len];
        for (i, set) in bits.iter().enumerate() {
            if *set {
                bytes[i / 8] |= 1 << (i % 8);
            }
        }
        while bytes.last() == Some(&0) {
            bytes.pop();
        }
        self.primary_signers = bytes;
    }

    /// Count the number of providers who signed this snapshot
    pub fn count_signers(&self) -> usize {
        self.primary_signers
            .iter()
            .map(|byte| byte.count_ones() as usize)
            .sum()
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
const PEAK_PREFIX: u8 = 0x02;

/// Hash a Merkle leaf: `blake2_256(0x00 ++ data)`.
///
/// Leaves and internal nodes use different prefixes, so the bytes of an
/// internal node never hash to the same value as a leaf.
pub fn hash_leaf(data: &[u8]) -> H256 {
    hash_prefixed(LEAF_PREFIX, data)
}

/// `blake2_256(prefix ++ data)`.
fn hash_prefixed(prefix: u8, data: &[u8]) -> H256 {
    let mut preimage = Vec::with_capacity(1 + data.len());
    preimage.push(prefix);
    preimage.extend_from_slice(data);
    blake2_256(&preimage)
}

/// Hash two children into an internal Merkle node:
/// `blake2_256(0x01 ++ left ++ right)`.
pub fn hash_children(left: H256, right: H256) -> H256 {
    let mut data = [0u8; 65];
    data[0] = NODE_PREFIX;
    data[1..33].copy_from_slice(left.as_bytes());
    data[33..].copy_from_slice(right.as_bytes());
    blake2_256(&data)
}

/// Combine MMR peaks into the MMR root.
///
/// Peaks fold from right to left with `blake2_256(0x02 ++ peak ++ rest)`.
/// One peak is the root itself and no peaks give the zero root.
pub fn bag_peaks(peaks: &[H256]) -> H256 {
    peaks
        .iter()
        .rev()
        .copied()
        .reduce(|rest, peak| hash_peaks(peak, rest))
        .unwrap_or_default()
}

/// Combine a peak with the bagged peaks to its right:
/// `blake2_256(0x02 ++ left ++ right)`.
fn hash_peaks(left: H256, right: H256) -> H256 {
    let mut data = [0u8; 65];
    data[0] = PEAK_PREFIX;
    data[1..33].copy_from_slice(left.as_bytes());
    data[33..].copy_from_slice(right.as_bytes());
    blake2_256(&data)
}

/// [`sp_core::Hasher`] for `binary-merkle-tree` that hashes internal nodes the
/// same way as [`hash_children`].
///
/// Pass leaf hashes to the library's `*_raw` functions. The library then
/// calls `hash` only on `left ++ right`, so the result is
/// `blake2_256(0x01 ++ left ++ right)`.
pub struct StorageHasher;

impl sp_core::Hasher for StorageHasher {
    type Out = H256;
    type StdHasher = hash256_std_hasher::Hash256StdHasher;
    const LENGTH: usize = 32;

    fn hash(x: &[u8]) -> H256 {
        hash_prefixed(NODE_PREFIX, x)
    }
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

// ─────────────────────────────────────────────────────────────────────────────
// Verification Utilities
// ─────────────────────────────────────────────────────────────────────────────

/// Verify that `proof.leaf` is the leaf at `leaf_index` of the MMR that has
/// `leaf_count` leaves and root `root`.
///
/// The caller supplies `leaf_index` and `leaf_count`. The proof does not
/// carry them, so it cannot prove the leaf at another position. The library
/// treats missing or extra trailing proof items as the bagged peaks to the
/// right, so the proof does not authenticate `leaf_count` itself.
pub fn verify_mmr_proof(proof: &MmrProof, leaf_index: u64, leaf_count: u64, root: &H256) -> bool {
    // `leaf_count / 2` bound: the library computes `2 * leaf_count`.
    if leaf_index >= leaf_count || leaf_count > u64::MAX / 2 {
        return false;
    }

    let mmr_size = mmr_lib::leaf_index_to_mmr_size(leaf_count - 1);
    let leaf = (
        mmr_lib::leaf_index_to_pos(leaf_index),
        hash_leaf(&proof.leaf.encode()),
    );
    mmr_lib::MerkleProof::<H256, StorageMerge>::new(mmr_size, proof.items.clone())
        .verify(*root, alloc::vec![leaf])
        .unwrap_or(false)
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

    fn snapshot_with_signers(primary_signers: Vec<u8>) -> BucketSnapshot<u32> {
        BucketSnapshot {
            commitment: Commitment::default(),
            checkpoint_block: 0,
            primary_signers,
        }
    }

    #[test]
    fn test_remove_provider_bit_shifts_higher_bits_down() {
        // Positions {0, 1, 3} set -> 0b0000_1011.
        let mut snapshot = snapshot_with_signers(alloc::vec![0b0000_1011]);
        assert!(snapshot.has_provider_signed(0));
        assert!(snapshot.has_provider_signed(1));
        assert!(!snapshot.has_provider_signed(2));
        assert!(snapshot.has_provider_signed(3));

        // Remove index 1: the old bit 1 is dropped, higher bits shift down.
        // New layout: old bit 0 -> new bit 0 (set), old bit 2 -> new bit 1
        // (clear), old bit 3 -> new bit 2 (set) => positions {0, 2} =>
        // 0b0000_0101.
        snapshot.remove_provider_bit(1);
        assert_eq!(snapshot.primary_signers, alloc::vec![0b0000_0101]);
        assert!(snapshot.has_provider_signed(0));
        assert!(!snapshot.has_provider_signed(1));
        assert!(snapshot.has_provider_signed(2));
        assert!(!snapshot.has_provider_signed(3));
    }

    #[test]
    fn test_remove_provider_bit_drops_highest_set_bit() {
        // Positions {0, 2} set -> 0b0000_0101.
        let mut snapshot = snapshot_with_signers(alloc::vec![0b0000_0101]);
        // Remove the highest set bit (index 2). Remaining position {0}.
        snapshot.remove_provider_bit(2);
        assert_eq!(snapshot.primary_signers, alloc::vec![0b0000_0001]);
        assert!(snapshot.has_provider_signed(0));
        assert!(!snapshot.has_provider_signed(1));
        assert!(!snapshot.has_provider_signed(2));
    }

    #[test]
    fn test_remove_provider_bit_trims_trailing_zero_bytes() {
        // Only bit 0 set in a two-byte field; removing index 0 leaves all
        // bits clear, so the repacked field is empty (trailing zeros dropped).
        let mut snapshot = snapshot_with_signers(alloc::vec![0b0000_0001, 0b0000_0000]);
        snapshot.remove_provider_bit(0);
        assert!(snapshot.primary_signers.is_empty());
        assert!(!snapshot.has_provider_signed(0));
    }

    #[test]
    fn test_remove_provider_bit_across_byte_boundary() {
        // Bit 8 set (first bit of second byte) -> [0, 1]. Removing index 0
        // shifts it down to bit 7 of the first byte -> [0b1000_0000].
        let mut snapshot = snapshot_with_signers(alloc::vec![0b0000_0000, 0b0000_0001]);
        assert!(snapshot.has_provider_signed(8));
        snapshot.remove_provider_bit(0);
        assert_eq!(snapshot.primary_signers, alloc::vec![0b1000_0000]);
        assert!(snapshot.has_provider_signed(7));
        assert!(!snapshot.has_provider_signed(8));
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
        const MMR_LEAF: &str = "3fbff3e341e04b94439e2f793f6983fda1cb58b7e8cc09b73670245f7e816f8d";
        const BAG_2_PEAKS: &str =
            "39ffd1718a7a2d0f61de8f7902cb47d9661392d004d5b91e835ccd3d475cd035";
        const BAG_3_PEAKS: &str =
            "de6b5def5927714c7e957d45b39ba6b3cb1b24c08d3d72e038387f46268eb23f";

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
                data_root: H256::repeat_byte(0x11),
                data_size: 1,
                total_size: 2,
            };
            assert_eq!(hash_leaf(&leaf.encode()), h(MMR_LEAF));
        }

        #[test]
        fn peaks_bag_right_to_left_with_the_peak_prefix() {
            let peaks: Vec<H256> = (1..=3).map(H256::repeat_byte).collect();
            assert_eq!(bag_peaks(&peaks[..2]), h(BAG_2_PEAKS));
            assert_eq!(bag_peaks(&peaks), h(BAG_3_PEAKS));
        }

        #[test]
        fn one_peak_is_the_root_and_no_peaks_give_zero() {
            let peak = H256::repeat_byte(7);
            assert_eq!(bag_peaks(&[peak]), peak);
            assert_eq!(bag_peaks(&[]), H256::zero());
        }

        #[test]
        fn library_root_over_three_chunks_matches_golden_root() {
            let leaves = [h(LEAF_A), h(LEAF_B), h(LEAF_C)];
            let root = binary_merkle_tree::merkle_root_raw::<StorageHasher, _>(leaves);
            assert_eq!(root, h(ROOT_3_CHUNKS));
        }
    }

    #[test]
    fn storage_hasher_hashes_like_hash_children() {
        use sp_core::Hasher;

        let left = H256::repeat_byte(0x0a);
        let right = H256::repeat_byte(0x0b);
        let mut input = left.as_bytes().to_vec();
        input.extend_from_slice(right.as_bytes());
        assert_eq!(StorageHasher::hash(&input), hash_children(left, right));
    }

    /// For every MMR size up to 33 leaves, the library root over the peaks
    /// it stores equals `bag_peaks` over the same peaks.
    #[test]
    fn library_mmr_root_matches_bag_peaks() {
        use mmr_lib::{helper::get_peaks, util::MemStore, MMRStoreReadOps, MMR};

        for leaf_count in 1u64..=33 {
            let store = MemStore::default();
            let mut mmr = MMR::<H256, StorageMerge, _>::new(0, &store);
            for i in 0..leaf_count {
                mmr.push(hash_leaf(&i.to_le_bytes())).unwrap();
            }
            let mmr_size = mmr.mmr_size();
            let root = mmr.get_root().unwrap();
            mmr.commit().unwrap();

            let peaks: Vec<H256> = get_peaks(mmr_size)
                .into_iter()
                .map(|pos| (&store).get_elem(pos).unwrap().unwrap())
                .collect();
            assert_eq!(root, bag_peaks(&peaks), "leaf_count {leaf_count}");
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
        let honest = MerkleProof {
            siblings: vec![leaves[1], right],
            path: vec![false, false],
        };
        assert!(verify_merkle_proof(leaves[0], 0, 4, &honest, &root));

        // The bytes of `left` presented as chunk data with one sibling. A
        // count of 2 makes one sibling the right length, so only the prefix
        // rejects the proof.
        let mut node_bytes = leaves[0].as_bytes().to_vec();
        node_bytes.extend_from_slice(leaves[1].as_bytes());
        let forged = MerkleProof {
            siblings: vec![right],
            path: vec![false],
        };
        assert!(!verify_merkle_proof(
            hash_leaf(&node_bytes),
            0,
            2,
            &forged,
            &root
        ));
    }

    fn test_leaf(i: u64) -> MmrLeaf {
        MmrLeaf {
            data_root: H256::repeat_byte(i as u8 + 1),
            data_size: i,
            total_size: 2 * i,
        }
    }

    /// Build the MMR over `leaf_count` test leaves with the library. Returns
    /// its root and the proof for the leaf at `index`.
    fn mmr_root_and_proof(leaf_count: u64, index: u64) -> (H256, MmrProof) {
        use mmr_lib::{util::MemStore, MMR};

        let store = MemStore::default();
        let mut mmr = MMR::<H256, StorageMerge, _>::new(0, &store);
        let positions: Vec<u64> = (0..leaf_count)
            .map(|i| mmr.push(hash_leaf(&test_leaf(i).encode())).unwrap())
            .collect();
        let root = mmr.get_root().unwrap();
        let items = mmr
            .gen_proof(vec![positions[index as usize]])
            .unwrap()
            .proof_items()
            .to_vec();
        (
            root,
            MmrProof {
                leaf: test_leaf(index),
                items,
            },
        )
    }

    #[test]
    fn mmr_proof_verifies_for_every_leaf_and_size() {
        for leaf_count in 1u64..=33 {
            for index in 0..leaf_count {
                let (root, proof) = mmr_root_and_proof(leaf_count, index);
                assert!(
                    verify_mmr_proof(&proof, index, leaf_count, &root),
                    "leaf {index} of {leaf_count}"
                );
            }
        }
    }

    #[test]
    fn mmr_proof_does_not_verify_for_another_leaf_index() {
        for leaf_count in 2u64..=17 {
            for index in 0..leaf_count {
                let (root, proof) = mmr_root_and_proof(leaf_count, index);
                for other in (0..leaf_count).filter(|other| *other != index) {
                    assert!(
                        !verify_mmr_proof(&proof, other, leaf_count, &root),
                        "proof for leaf {index} verified as leaf {other} of {leaf_count}"
                    );
                }
            }
        }
    }

    #[test]
    fn tampered_mmr_proof_does_not_verify() {
        let (root, proof) = mmr_root_and_proof(7, 2);
        assert!(verify_mmr_proof(&proof, 2, 7, &root));

        let mut wrong_leaf = proof.clone();
        wrong_leaf.leaf.data_size += 1;
        assert!(!verify_mmr_proof(&wrong_leaf, 2, 7, &root));

        let mut wrong_item = proof.clone();
        wrong_item.items[0] = H256::repeat_byte(0x44);
        assert!(!verify_mmr_proof(&wrong_item, 2, 7, &root));

        let mut missing_item = proof.clone();
        missing_item.items.pop();
        assert!(!verify_mmr_proof(&missing_item, 2, 7, &root));

        let mut extra_item = proof.clone();
        extra_item.items.push(H256::repeat_byte(0x55));
        assert!(!verify_mmr_proof(&extra_item, 2, 7, &root));

        assert!(!verify_mmr_proof(&proof, 2, 7, &H256::repeat_byte(0x66)));
    }

    #[test]
    fn mmr_proof_rejects_invalid_leaf_position_without_panicking() {
        let (root, proof) = mmr_root_and_proof(4, 1);
        assert!(!verify_mmr_proof(&proof, 0, 0, &root));
        assert!(!verify_mmr_proof(&proof, 4, 4, &root));
        assert!(!verify_mmr_proof(&proof, 1, u64::MAX / 2 + 1, &root));
        assert!(!verify_mmr_proof(&proof, u64::MAX, u64::MAX, &root));
    }
}
