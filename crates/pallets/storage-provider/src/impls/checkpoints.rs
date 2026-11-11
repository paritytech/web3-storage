// SPDX-License-Identifier: Apache-2.0

use crate::*;
use frame_support::pallet_prelude::*;
use sp_core::H256;
use storage_primitives::{Commitment, HISTORICAL_ROOT_PRIMES};

/// The root a replica sync matched: `position` 0 is the current snapshot,
/// `position` p in 1..=6 is `Bucket::historical_roots[p - 1]`.
pub(crate) struct MatchedRoot {
    pub position: u8,
    pub commitment: Commitment,
}

impl<T: Config> Pallet<T> {
    pub(crate) fn update_historical_roots(
        bucket: &mut Bucket<T>,
        anchor_block: BlockNumberFor<T>,
        commitment: Commitment,
    ) {
        let block_num: u32 = anchor_block.try_into().unwrap_or(0u32);

        for (i, &prime) in HISTORICAL_ROOT_PRIMES.iter().enumerate() {
            let quotient = block_num / prime;
            if quotient != bucket.historical_roots[i].0 {
                bucket.historical_roots[i] = (quotient, commitment);
            }
        }
    }

    pub(crate) fn find_matching_root(
        bucket: &Bucket<T>,
        roots: &[Option<H256>; 7],
    ) -> Result<MatchedRoot, DispatchError> {
        if let (Some(snapshot), Some(root)) = (&bucket.snapshot, roots[0]) {
            if snapshot.commitment.mmr_root == root {
                return Ok(MatchedRoot {
                    position: 0,
                    commitment: snapshot.commitment,
                });
            }
        }

        // A zero root is an entry no checkpoint has written yet, not a
        // commitment a replica can sync to.
        for i in 0..6 {
            let entry = bucket.historical_roots[i].1;
            if entry.mmr_root != H256::zero() && roots[i + 1] == Some(entry.mmr_root) {
                return Ok(MatchedRoot {
                    position: i as u8 + 1,
                    commitment: entry,
                });
            }
        }

        Err(Error::<T>::InvalidSyncRoot.into())
    }
}
