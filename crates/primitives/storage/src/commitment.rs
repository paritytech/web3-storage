// SPDX-License-Identifier: Apache-2.0

//! MMR commitment types and the payload providers sign over bucket state.

use codec::{Decode, DecodeWithMemTracking, Encode, MaxEncodedLen};
use scale_info::TypeInfo;
use sp_core::H256;

use crate::BucketId;

/// The `(mmr_root, start_seq, leaf_count)` triplet that identifies an MMR
/// commitment over a contiguous range of leaves.
///
/// One reusable type instead of three loose fields: it is a field group inside
/// [`CommitmentPayload`], [`BucketSnapshot`](crate::BucketSnapshot), and
/// [`ReplicaSyncRecord`](crate::ReplicaSyncRecord), and it is the single
/// argument the checkpoint/challenge extrinsics take in place of passing
/// `mmr_root`, `start_seq`, and `leaf_count` separately.
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
    /// MMR commitment being signed over (root + covered leaf range).
    pub commitment: Commitment,
    /// Replay-protection nonce — block number at the time the signer signed.
    pub nonce: u64,
}

impl CommitmentPayload {
    /// Current protocol version. Bumped from `0x01` to `0x02` when the `nonce`
    /// field was added; older signatures (no nonce) cannot be replayed against
    /// this version because the encoded payload would mismatch on `version`.
    pub const CURRENT_VERSION: u8 = 2;

    /// Create a new commitment payload
    pub fn new(bucket_id: BucketId, commitment: Commitment, nonce: u64) -> Self {
        Self {
            version: Self::CURRENT_VERSION,
            bucket_id,
            commitment,
            nonce,
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
            42,
        );

        assert_eq!(payload.range_end(), 15);
        assert!(!payload.contains_seq(9));
        assert!(payload.contains_seq(10));
        assert!(payload.contains_seq(14));
        assert!(!payload.contains_seq(15));
    }

    /// Embedding `Commitment` in place of the loose
    /// `(mmr_root, start_seq, leaf_count)` triplet must be byte-identical to
    /// the previous flat layout, so existing signatures still verify and stored
    /// values need no migration. Pin the exact encoding to guard that.
    #[test]
    fn commitment_payload_encoding_is_byte_identical() {
        let payload = CommitmentPayload::new(
            1,
            Commitment {
                mmr_root: H256::zero(),
                start_seq: 10,
                leaf_count: 5,
            },
            42,
        );

        let mut expected = alloc::vec![CommitmentPayload::CURRENT_VERSION]; // version: u8
        expected.extend_from_slice(&1u64.to_le_bytes()); // bucket_id
        expected.extend_from_slice(&[0u8; 32]); // mmr_root (H256::zero)
        expected.extend_from_slice(&10u64.to_le_bytes()); // start_seq
        expected.extend_from_slice(&5u64.to_le_bytes()); // leaf_count
        expected.extend_from_slice(&42u64.to_le_bytes()); // nonce

        assert_eq!(payload.encode(), expected);
    }
}
