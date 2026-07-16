// SPDX-License-Identifier: Apache-2.0

//! Checkpoint snapshot types and provider-initiated checkpoint coordination.

use alloc::vec::Vec;
use codec::{Decode, DecodeWithMemTracking, Encode, MaxEncodedLen};
use scale_info::TypeInfo;
use sp_core::H256;

use crate::{BucketId, Commitment};

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
    /// The `nonce` value from the `CommitmentPayload` that the original
    /// signers signed. Required by `extend_checkpoint` so a late-arriving
    /// signature can be verified against the same payload the initial
    /// signers committed to.
    pub commitment_nonce: u64,
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
    /// MMR commitment being proposed (root + covered leaf range).
    pub commitment: Commitment,
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
            commitment: Commitment {
                mmr_root,
                start_seq,
                leaf_count,
            },
            window,
        }
    }

    /// Get the canonical range end (exclusive)
    pub fn range_end(&self) -> u64 {
        self.commitment.range_end()
    }

    /// Check if a sequence number is within this proposal's range
    pub fn contains_seq(&self, seq: u64) -> bool {
        self.commitment.contains_seq(seq)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn snapshot_with_signers(primary_signers: Vec<u8>) -> BucketSnapshot<u32> {
        BucketSnapshot {
            commitment: Commitment::default(),
            checkpoint_block: 0,
            primary_signers,
            commitment_nonce: 0,
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
}
