// SPDX-License-Identifier: GPL-3.0-only

use serde::{Deserialize, Serialize};
use storage_primitives::BucketId;

/// Request to upload a node (chunk or internal node).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UploadNodeRequest {
    /// Bucket this node belongs to
    pub bucket_id: BucketId,
    /// Expected hash of the node data
    pub hash: String,
    /// Base64-encoded node data
    pub data: String,
    /// Child hashes for internal nodes, null for leaf chunks
    pub children: Option<Vec<String>>,
}

/// Response from uploading a node.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UploadNodeResponse {
    pub stored: bool,
}

/// Response from downloading a node.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DownloadNodeResponse {
    pub hash: String,
    pub data: String,
    pub children: Option<Vec<String>>,
}
