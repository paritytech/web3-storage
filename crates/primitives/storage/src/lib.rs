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
use scale_info::TypeInfo;
use sp_core::H256;

pub mod agreement_term;
pub mod provider_replay_state;

pub use agreement_term::*;
pub use provider_replay_state::*;

/// Bucket ID is a stable, unique identifier (not an index into a collection).
/// Using u64 ensures IDs never get reused even if buckets are deleted.
pub type BucketId = u64;

/// Default chunk size: 256 KiB
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

/// Aggregated per-challenger statistics kept on-chain so the SDK can answer
/// "how many challenges have I issued / won / lost / earned" without scanning
/// historical events. Updated on `create_challenge`, on `ChallengeDefended`,
/// and on `ChallengeSlashed`.
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
    Default,
    Debug,
)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct ChallengerStatRecord {
    /// Total challenges the challenger has ever opened.
    pub total_challenges: u32,
    /// Challenges where the provider was slashed (either invalid response or
    /// timeout). The challenger is only made whole (deposit refunded) and earns
    /// no reward — the slashed stake goes entirely to the Treasury, per the
    /// design's challenge model.
    pub successful_challenges: u32,
    /// Challenges where the provider successfully defended.
    pub failed_challenges: u32,
}

/// Why a provider was slashed via the challenge mechanism.
///
/// Emitted in `ChallengeSlashed` so observers can distinguish a provider that
/// went silent (Timeout) from one that submitted a demonstrably-false response
/// (InvalidProof, InvalidDeletionClaim, InvalidSupersededClaim).
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
pub enum SlashReason {
    /// Provider failed to respond before the challenge deadline.
    Timeout,
    /// Provider submitted a `Proof` response whose chunk-Merkle or MMR proof
    /// did not verify.
    InvalidProof,
    /// Provider submitted a `Deleted` response with a signature or
    /// `new_start_seq` that does not stand up against on-chain state.
    InvalidDeletionClaim,
    /// Provider claimed `Superseded` but the bucket's canonical snapshot
    /// does not actually cover the challenged sequence.
    InvalidSupersededClaim,
}

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
    /// Peaks of the MMR
    pub peaks: Vec<H256>,
    /// The leaf being proven
    pub leaf: MmrLeaf,
    /// Proof from leaf to peak
    pub leaf_proof: MerkleProof,
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

/// Compute blake2b-256 hash of data
pub fn blake2_256(data: &[u8]) -> H256 {
    sp_crypto_hashing::blake2_256(data).into()
}

/// Compute hash of two children for internal Merkle node
pub fn hash_children(left: H256, right: H256) -> H256 {
    let mut data = [0u8; 64];
    data[..32].copy_from_slice(left.as_bytes());
    data[32..].copy_from_slice(right.as_bytes());
    blake2_256(&data)
}

// ─────────────────────────────────────────────────────────────────────────────
// Verification Utilities
// ─────────────────────────────────────────────────────────────────────────────

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
}
