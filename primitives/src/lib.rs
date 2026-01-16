//! Shared primitives for Scalable Web3 Storage
//!
//! This crate contains types and structures shared between the on-chain pallet
//! and off-chain provider node.

#![cfg_attr(not(feature = "std"), no_std)]

extern crate alloc;

use alloc::vec::Vec;
use bitvec::{order::Lsb0, vec::BitVec};
use parity_scale_codec::{Decode, Encode, MaxEncodedLen};
use scale_info::TypeInfo;
use sp_core::H256;
use sp_runtime::RuntimeDebug;

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
#[derive(Clone, Copy, PartialEq, Eq, Encode, Decode, TypeInfo, MaxEncodedLen, RuntimeDebug)]
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
#[derive(Clone, PartialEq, Eq, Encode, Decode, TypeInfo, MaxEncodedLen, RuntimeDebug)]
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
#[derive(Clone, Copy, PartialEq, Eq, Encode, Decode, TypeInfo, MaxEncodedLen, RuntimeDebug)]
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
#[derive(Clone, Copy, PartialEq, Eq, Encode, Decode, TypeInfo, MaxEncodedLen, RuntimeDebug)]
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
#[derive(Clone, PartialEq, Eq, Encode, Decode, TypeInfo, MaxEncodedLen, RuntimeDebug)]
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

/// Challenge identifier combining deadline and index.
#[derive(Clone, Copy, PartialEq, Eq, Encode, Decode, TypeInfo, MaxEncodedLen, RuntimeDebug)]
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
#[derive(Clone, PartialEq, Eq, Encode, Decode, TypeInfo, MaxEncodedLen, RuntimeDebug)]
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
#[derive(Clone, PartialEq, Eq, Encode, Decode, TypeInfo, RuntimeDebug)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct MerkleProof {
    /// Sibling hashes from leaf to root
    pub siblings: Vec<H256>,
    /// Path bits (false = left, true = right)
    pub path: Vec<bool>,
}

/// MMR proof for verifying leaf inclusion.
#[derive(Clone, PartialEq, Eq, Encode, Decode, TypeInfo, RuntimeDebug)]
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
#[derive(Clone, PartialEq, Eq, Encode, Decode, TypeInfo, MaxEncodedLen, RuntimeDebug)]
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
}

impl CommitmentPayload {
    /// Current protocol version
    pub const CURRENT_VERSION: u8 = 1;

    /// Create a new commitment payload
    pub fn new(bucket_id: BucketId, mmr_root: H256, start_seq: u64, leaf_count: u64) -> Self {
        Self {
            version: Self::CURRENT_VERSION,
            bucket_id,
            mmr_root,
            start_seq,
            leaf_count,
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
#[derive(Clone, PartialEq, Eq, Encode, Decode, TypeInfo, RuntimeDebug)]
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
    pub primary_signers: BitVec<u8, Lsb0>,
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
}

// ─────────────────────────────────────────────────────────────────────────────
// Hashing Utilities
// ─────────────────────────────────────────────────────────────────────────────

/// Compute blake2b-256 hash of data
pub fn blake2_256(data: &[u8]) -> H256 {
    use blake2::{Blake2b, Digest};
    use sp_core::crypto::ByteArray;

    let mut hasher = Blake2b::<sp_core::U32>::new();
    hasher.update(data);
    let result = hasher.finalize();
    H256::from_slice(result.as_slice())
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
pub fn verify_merkle_proof(
    leaf_hash: H256,
    index: u64,
    proof: &MerkleProof,
    root: &H256,
) -> bool {
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_commitment_payload_range() {
        let payload = CommitmentPayload::new(1, H256::zero(), 10, 5);

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
