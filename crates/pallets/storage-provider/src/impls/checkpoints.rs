// SPDX-License-Identifier: Apache-2.0

use crate::*;
use frame_support::pallet_prelude::*;
use sp_core::H256;
use storage_primitives::{Commitment, HISTORICAL_ROOT_PRIMES};

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
    ) -> Result<(u8, Commitment), DispatchError> {
        // Check current snapshot first
        if let (Some(snapshot), Some(root)) = (&bucket.snapshot, roots[0]) {
            if snapshot.commitment.mmr_root == root {
                return Ok((0, snapshot.commitment));
            }
        }

        // Check historical roots
        for i in 0..6 {
            if let Some(root) = roots[i + 1] {
                if bucket.historical_roots[i].1.mmr_root == root {
                    return Ok((i as u8 + 1, bucket.historical_roots[i].1));
                }
            }
        }

        Err(Error::<T>::InvalidSyncRoot.into())
    }
}
