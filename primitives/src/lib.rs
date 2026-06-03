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
        /// Last confirmed sync: (mmr_root, block_number).
        /// None if replica hasn't confirmed sync yet.
        last_sync: Option<(H256, BlockNumber)>,
    },
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

/// Parameters specific to replica agreement requests.
#[derive(
    Clone, PartialEq, Eq, Encode, Decode, DecodeWithMemTracking, TypeInfo, MaxEncodedLen, Debug,
)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct ReplicaRequestParams<Balance, BlockNumber> {
    /// Initial sync balance to fund per-sync payments
    pub sync_balance: Balance,
    /// Minimum blocks between sync confirmations.
    pub min_sync_interval: BlockNumber,
}

// ─────────────────────────────────────────────────────────────────────────────
// Challenge Types
// ─────────────────────────────────────────────────────────────────────────────

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

/// Payload that providers sign to commit to bucket state.
///
/// The `nonce` field binds each signature to a specific moment in time —
/// callers populate it with the block number at sign-time. The pallet rejects
/// signatures whose nonce is too far in the past, preventing an attacker who
/// captures a single signature from replaying it forever to challenge or
/// defend against the signer.
#[derive(
    Clone, PartialEq, Eq, Encode, Decode, DecodeWithMemTracking, TypeInfo, MaxEncodedLen, Debug,
)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct CommitmentPayload {
    /// Protocol version for future compatibility
    pub version: u8,
    /// Reference to on-chain bucket
    pub bucket_id: BucketId,
    /// Root of MMR containing all data_roots
    pub mmr_root: H256,
    /// Sequence number of first leaf in this MMR
    pub start_seq: u64,
    /// Number of leaves in this MMR
    pub leaf_count: u64,
    /// Replay-protection nonce — block number at the time the signer signed.
    pub nonce: u64,
}

impl CommitmentPayload {
    /// Current protocol version. Bumped from `0x01` to `0x02` when the `nonce`
    /// field was added; older signatures (no nonce) cannot be replayed against
    /// this version because the encoded payload would mismatch on `version`.
    pub const CURRENT_VERSION: u8 = 2;

    /// Create a new commitment payload
    pub fn new(
        bucket_id: BucketId,
        mmr_root: H256,
        start_seq: u64,
        leaf_count: u64,
        nonce: u64,
    ) -> Self {
        Self {
            version: Self::CURRENT_VERSION,
            bucket_id,
            mmr_root,
            start_seq,
            leaf_count,
            nonce,
        }
    }

    /// Get the canonical range end (exclusive)
    pub fn range_end(&self) -> u64 {
        self.start_seq.saturating_add(self.leaf_count)
    }

    /// Check if a sequence number is within this commitment's range
    pub fn contains_seq(&self, seq: u64) -> bool {
        seq >= self.start_seq && seq < self.range_end()
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Snapshot Types
// ─────────────────────────────────────────────────────────────────────────────

/// Bucket snapshot representing canonical state at a checkpoint.
#[derive(Clone, PartialEq, Eq, Encode, Decode, DecodeWithMemTracking, TypeInfo, Debug)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct BucketSnapshot<BlockNumber> {
    /// Canonical MMR root
    pub mmr_root: H256,
    /// Start sequence number
    pub start_seq: u64,
    /// Number of leaves in the MMR
    pub leaf_count: u64,
    /// Block at which checkpointed
    pub checkpoint_block: BlockNumber,
    /// Bitfield indicating which primary providers signed this snapshot.
    /// Bit i is set if primary_providers[i] signed.
    /// Uses Vec<u8> with LSB0 ordering for efficient bit manipulation.
    pub primary_signers: Vec<u8>,
    /// The `nonce` value from the `CommitmentPayload` that the original
    /// signers signed. Required by `extend_checkpoint` so a late-arriving
    /// signature can be verified against the same payload the initial
    /// signers committed to.
    pub commitment_nonce: u64,
}

impl<BlockNumber> BucketSnapshot<BlockNumber> {
    /// Get the canonical range end (exclusive)
    pub fn range_end(&self) -> u64 {
        self.start_seq.saturating_add(self.leaf_count)
    }

    /// Check if a sequence number is within this snapshot's range
    pub fn contains_seq(&self, seq: u64) -> bool {
        seq >= self.start_seq && seq < self.range_end()
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

    /// Count the number of providers who signed this snapshot
    pub fn count_signers(&self) -> usize {
        self.primary_signers
            .iter()
            .map(|byte| byte.count_ones() as usize)
            .sum()
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Provider-Initiated Checkpoint Types
// ─────────────────────────────────────────────────────────────────────────────

/// Configuration for provider-initiated checkpoints.
///
/// Providers can autonomously coordinate checkpoints without requiring
/// the client to be online. Uses deterministic leader election and
/// checkpoint windows with grace periods.
#[derive(
    Clone, PartialEq, Eq, Encode, Decode, DecodeWithMemTracking, TypeInfo, MaxEncodedLen, Debug,
)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct CheckpointWindowConfig<BlockNumber> {
    /// Blocks between checkpoints (e.g., 100 blocks = ~10 minutes)
    pub interval: BlockNumber,
    /// Grace period for leader before fallback (e.g., 20 blocks = ~2 minutes)
    pub grace_period: BlockNumber,
    /// Whether provider-initiated checkpoints are enabled for this bucket
    pub enabled: bool,
}

/// Proposal for provider-initiated checkpoint (signed by providers).
///
/// This is the payload that providers sign to agree on a checkpoint.
/// The window number prevents cross-window replay attacks.
#[derive(
    Clone, PartialEq, Eq, Encode, Decode, DecodeWithMemTracking, TypeInfo, MaxEncodedLen, Debug,
)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct CheckpointProposal {
    /// Protocol version for future compatibility
    pub version: u8,
    /// Reference to on-chain bucket
    pub bucket_id: BucketId,
    /// Root of MMR containing all data_roots
    pub mmr_root: H256,
    /// Sequence number of first leaf in this MMR
    pub start_seq: u64,
    /// Number of leaves in this MMR
    pub leaf_count: u64,
    /// Window number this proposal is for (prevents replay)
    pub window: u64,
}

impl CheckpointProposal {
    /// Current protocol version
    pub const CURRENT_VERSION: u8 = 1;

    /// Create a new checkpoint proposal
    pub fn new(
        bucket_id: BucketId,
        mmr_root: H256,
        start_seq: u64,
        leaf_count: u64,
        window: u64,
    ) -> Self {
        Self {
            version: Self::CURRENT_VERSION,
            bucket_id,
            mmr_root,
            start_seq,
            leaf_count,
            window,
        }
    }

    /// Get the canonical range end (exclusive)
    pub fn range_end(&self) -> u64 {
        self.start_seq.saturating_add(self.leaf_count)
    }

    /// Check if a sequence number is within this proposal's range
    pub fn contains_seq(&self, seq: u64) -> bool {
        seq >= self.start_seq && seq < self.range_end()
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Hashing Utilities
// ─────────────────────────────────────────────────────────────────────────────

/// Compute blake2b-256 hash of data
pub fn blake2_256(data: &[u8]) -> H256 {
    // Use sp_core's blake2_256 which is optimized and available in both std and no_std
    sp_core::hashing::blake2_256(data).into()
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
        let payload = CommitmentPayload::new(1, H256::zero(), 10, 5, 42);

        assert_eq!(payload.range_end(), 15);
        assert!(!payload.contains_seq(9));
        assert!(payload.contains_seq(10));
        assert!(payload.contains_seq(14));
        assert!(!payload.contains_seq(15));
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
