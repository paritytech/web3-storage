// SPDX-License-Identifier: GPL-3.0-only

use serde::{Deserialize, Serialize};

/// Query parameters for reading chunks.
#[derive(Debug, Clone, Deserialize)]
pub struct ReadQuery {
    pub data_root: String,
    pub offset: u64,
    pub length: u64,
}

/// A chunk with its proof.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChunkWithProof {
    pub hash: String,
    pub data: String,
    pub proof: Vec<String>,
}

/// Response with chunks.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ReadResponse {
    pub chunks: Vec<ChunkWithProof>,
}
