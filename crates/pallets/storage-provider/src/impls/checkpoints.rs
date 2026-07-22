// SPDX-License-Identifier: Apache-2.0

use crate::*;
use frame_support::pallet_prelude::*;
use sp_core::H256;
use sp_runtime::traits::{SaturatedConversion, Saturating};
use storage_primitives::{BucketId, HISTORICAL_ROOT_PRIMES};

impl<T: Config> Pallet<T> {
    pub(crate) fn update_historical_roots(
        bucket: &mut Bucket<T>,
        anchor_block: BlockNumberFor<T>,
        mmr_root: H256,
    ) {
        let block_num: u32 = anchor_block.try_into().unwrap_or(0u32);

        for (i, &prime) in HISTORICAL_ROOT_PRIMES.iter().enumerate() {
            let quotient = block_num / prime;
            if quotient != bucket.historical_roots[i].0 {
                bucket.historical_roots[i] = (quotient, mmr_root);
            }
        }
    }

    pub(crate) fn find_matching_root(
        bucket: &Bucket<T>,
        roots: &[Option<H256>; 7],
    ) -> Result<(u8, H256), DispatchError> {
        // Check current snapshot first
        if let (Some(snapshot), Some(root)) = (&bucket.snapshot, roots[0]) {
            if snapshot.commitment.mmr_root == root {
                return Ok((0, root));
            }
        }

        // Check historical roots
        for i in 0..6 {
            if let Some(root) = roots[i + 1] {
                if bucket.historical_roots[i].1 == root {
                    return Ok((i as u8 + 1, root));
                }
            }
        }

        Err(Error::<T>::InvalidSyncRoot.into())
    }

    /// Calculate the checkpoint window number for a given block.
    ///
    /// Window 0 starts at block 0, window 1 at block `interval`, etc.
    pub(crate) fn calculate_window(block: BlockNumberFor<T>, interval: BlockNumberFor<T>) -> u64 {
        if interval.is_zero() {
            return 0;
        }
        let block_num: u64 = block.saturated_into();
        let interval_num: u64 = interval.saturated_into();
        block_num / interval_num
    }

    /// Calculate the start block for a given checkpoint window.
    pub(crate) fn window_start_block(
        window: u64,
        interval: BlockNumberFor<T>,
    ) -> BlockNumberFor<T> {
        let interval_num: u64 = interval.saturated_into();
        let start: u64 = window.saturating_mul(interval_num);
        start.saturated_into()
    }

    /// Calculate the leader index for a given bucket and window.
    ///
    /// Uses deterministic selection: blake2_256(bucket_id || window) % num_providers.
    /// This ensures all providers can independently calculate who the leader is.
    pub(crate) fn calculate_leader_index(
        bucket_id: BucketId,
        window: u64,
        num_providers: u32,
    ) -> u32 {
        if num_providers == 0 {
            return 0;
        }
        // Create deterministic seed from bucket_id and window
        let mut data = [0u8; 16];
        data[..8].copy_from_slice(&bucket_id.to_le_bytes());
        data[8..].copy_from_slice(&window.to_le_bytes());
        let hash = sp_io::hashing::blake2_256(&data);
        // Take first 4 bytes as u32 and mod by num_providers
        let seed = u32::from_le_bytes([hash[0], hash[1], hash[2], hash[3]]);
        seed % num_providers
    }

    /// Get the checkpoint config for a bucket, falling back to defaults.
    pub(crate) fn get_checkpoint_config(
        bucket_id: BucketId,
    ) -> storage_primitives::CheckpointWindowConfig<BlockNumberFor<T>> {
        CheckpointConfigs::<T>::get(bucket_id).unwrap_or_else(|| {
            storage_primitives::CheckpointWindowConfig {
                interval: T::DefaultCheckpointInterval::get(),
                grace_period: T::DefaultCheckpointGrace::get(),
                enabled: true, // Enabled by default
            }
        })
    }

    /// Check if the anchor block is within the grace period for a window.
    pub(crate) fn is_within_grace_period(
        anchor_block: BlockNumberFor<T>,
        window: u64,
        config: &storage_primitives::CheckpointWindowConfig<BlockNumberFor<T>>,
    ) -> bool {
        let window_start = Self::window_start_block(window, config.interval);
        let grace_end = window_start.saturating_add(config.grace_period);
        anchor_block <= grace_end
    }
}
