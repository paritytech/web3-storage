// SPDX-License-Identifier: GPL-3.0-only

use serde::{Deserialize, Serialize};
use storage_primitives::BucketId;

/// Request to check existence of nodes.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ExistsRequest {
    pub bucket_id: BucketId,
    pub hashes: Vec<String>,
}

/// Response with existing and missing nodes.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ExistsResponse {
    pub exists: Vec<String>,
    pub missing: Vec<String>,
}
