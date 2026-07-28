// SPDX-License-Identifier: GPL-3.0-only

//! Bucket-level summary types produced by the storage backends.

use serde::{Deserialize, Serialize};
use storage_primitives::BucketId;

/// Bucket summary info.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BucketSummary {
    pub bucket_id: BucketId,
    pub mmr_root: String,
    pub start_seq: u64,
    pub leaf_count: u64,
}

/// Per-bucket statistics.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BucketStats {
    pub bucket_id: BucketId,
    pub leaf_count: u64,
    pub node_count: u64,
    pub bytes_stored: u64,
}
