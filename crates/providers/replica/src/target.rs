// SPDX-License-Identifier: Apache-2.0

//! Choosing which of the chain's commitments for a bucket a replica syncs to.

use crate::coordinator::BucketCommitments;
use sp_core::H256;
use storage_primitives::{Commitment, HISTORICAL_ROOT_PRIMES};

/// A commitment to sync to, with the position `confirm_replica_sync` finds it
/// at.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SyncTarget {
    /// 0 for the snapshot, `p` in 1..=6 for `historical_roots[p - 1]`.
    pub position: u8,
    pub commitment: Commitment,
}

/// Pick the commitment to sync to, or `None` when the chain has none the
/// replica may confirm.
///
/// A checkpoint overwrites historical slot `i` at the first checkpoint at or
/// after anchor block `(quotient_i + 1) * prime_i`. A transfer takes time, so
/// the replica picks the slot that stays valid longest: the one with the latest
/// overwrite block. When several slots tie, the lowest position wins. The
/// snapshot is the fallback when no slot qualifies.
///
/// A commitment qualifies when its root is not zero (no checkpoint wrote it)
/// and differs from `last_sync_root`, because the pallet rejects a confirmation
/// of the root last confirmed.
pub fn select_target(
    commitments: &BucketCommitments,
    last_sync_root: Option<H256>,
) -> Option<SyncTarget> {
    let qualifies = |commitment: &Commitment| {
        commitment.mmr_root != H256::zero() && Some(commitment.mmr_root) != last_sync_root
    };

    let slot = commitments
        .historical_roots
        .iter()
        .zip(HISTORICAL_ROOT_PRIMES)
        .enumerate()
        .filter(|(_, ((_, commitment), _))| qualifies(commitment))
        .map(|(index, ((quotient, commitment), prime))| {
            let overwrite_block = (u64::from(*quotient) + 1) * u64::from(prime);
            (overwrite_block, index, commitment)
        })
        .reduce(|best, next| if next.0 > best.0 { next } else { best });

    if let Some((_, index, commitment)) = slot {
        return Some(SyncTarget {
            position: index as u8 + 1,
            commitment: *commitment,
        });
    }

    qualifies(&commitments.snapshot).then_some(SyncTarget {
        position: 0,
        commitment: commitments.snapshot,
    })
}

/// The position where `confirm_replica_sync` finds `root`: 0 for the
/// snapshot, `p` for `historical_roots[p - 1]`, in the order the pallet tries
/// them. `None` when the chain no longer holds `root`.
pub fn position_of(commitments: &BucketCommitments, root: H256) -> Option<u8> {
    if root == H256::zero() {
        return None;
    }
    if commitments.snapshot.mmr_root == root {
        return Some(0);
    }
    commitments
        .historical_roots
        .iter()
        .position(|(_, commitment)| commitment.mmr_root == root)
        .map(|index| index as u8 + 1)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn commitment(byte: u8) -> Commitment {
        Commitment {
            mmr_root: H256::repeat_byte(byte),
            start_seq: 0,
            leaf_count: 1,
        }
    }

    /// Commitments with the given snapshot and `(slot index, quotient, root
    /// byte)` historical entries; the other slots are unwritten.
    fn commitments(snapshot: u8, slots: &[(usize, u32, u8)]) -> BucketCommitments {
        let mut commitments = BucketCommitments::default();
        if snapshot != 0 {
            commitments.snapshot = commitment(snapshot);
        }
        for &(index, quotient, byte) in slots {
            commitments.historical_roots[index] = (quotient, commitment(byte));
        }
        commitments
    }

    #[test]
    fn nothing_is_selected_when_every_root_is_zero() {
        assert_eq!(select_target(&commitments(0, &[]), None), None);
    }

    #[test]
    fn the_snapshot_is_the_fallback_when_no_slot_qualifies() {
        let target = select_target(&commitments(1, &[]), None).unwrap();
        assert_eq!(target.position, 0);
        assert_eq!(target.commitment, commitment(1));
    }

    #[test]
    fn a_slot_is_preferred_over_the_snapshot() {
        // The snapshot is newer, but the slot stays valid for longer.
        let target = select_target(&commitments(1, &[(2, 5, 2)]), None).unwrap();
        assert_eq!(target.position, 3);
        assert_eq!(target.commitment, commitment(2));
    }

    #[test]
    fn the_slot_with_the_latest_overwrite_block_is_selected() {
        // Overwrite blocks: slot 0 (prime 3): 11 * 3 = 33, slot 3 (prime 23):
        // 2 * 23 = 46, slot 5 (prime 113): 1 * 113 = 113.
        let slots = [(0, 10, 10), (3, 1, 30), (5, 0, 50)];
        let target = select_target(&commitments(1, &slots), None).unwrap();
        assert_eq!(target.position, 6);
        assert_eq!(target.commitment, commitment(50));

        // Without slot 5, slot 3 wins.
        let target = select_target(&commitments(1, &slots[..2]), None).unwrap();
        assert_eq!(target.position, 4);
    }

    #[test]
    fn the_lowest_position_wins_a_tie() {
        // Slot 0 (prime 3) with quotient 6 and slot 1 (prime 7) with quotient 2
        // are both overwritten at block 21.
        let target = select_target(&commitments(0, &[(0, 6, 1), (1, 2, 2)]), None).unwrap();
        assert_eq!(target.position, 1);
    }

    #[test]
    fn the_last_synced_root_is_skipped() {
        let both_slots = commitments(1, &[(1, 5, 2), (4, 1, 3)]);

        // Slot 4 (overwrite block 94) beats slot 1 (42) until it is the last
        // sync.
        assert_eq!(select_target(&both_slots, None).unwrap().position, 5);
        let last_sync = Some(H256::repeat_byte(3));
        assert_eq!(select_target(&both_slots, last_sync).unwrap().position, 2);

        // The snapshot is skipped too, so nothing is left.
        let snapshot_only = commitments(1, &[]);
        assert_eq!(
            select_target(&snapshot_only, Some(H256::repeat_byte(1))),
            None
        );
    }

    #[test]
    fn a_root_is_found_at_the_first_position_that_holds_it() {
        let both = commitments(1, &[(3, 5, 2), (4, 1, 1)]);
        assert_eq!(position_of(&both, H256::repeat_byte(1)), Some(0));
        assert_eq!(position_of(&both, H256::repeat_byte(2)), Some(4));
        assert_eq!(position_of(&both, H256::repeat_byte(9)), None);
        assert_eq!(position_of(&both, H256::zero()), None);
    }

    #[test]
    fn an_unwritten_slot_with_a_stale_quotient_never_wins() {
        // A zero root is unwritten whatever its quotient is.
        let mut commitments = commitments(1, &[]);
        commitments.historical_roots[5] = (1_000_000, Commitment::default());

        let target = select_target(&commitments, None).unwrap();
        assert_eq!(target.position, 0);
    }
}
