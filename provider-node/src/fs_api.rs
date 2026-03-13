//! File system HTTP API handlers.
//!
//! These endpoints handle the full upload flow (chunking, tree-building, commit, index update)
//! so clients just send raw bytes + path — mirroring the S3 API pattern.
//!
//! File path is passed as a `?path=` query parameter on file/directory endpoints.
//! Example: `PUT /fs/1/file?path=/docs/report.pdf`

use crate::error::Error;
use crate::fs_index::FsEntryMeta;
use crate::storage::{build_padded_merkle_tree, hex_encode};
use crate::ProviderState;
use axum::{
    body::Bytes,
    extract::{Path, Query, State},
    http::StatusCode,
    response::{IntoResponse, Response},
    Json,
};
use serde::{Deserialize, Serialize};
use sp_core::H256;
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};
use storage_primitives::{blake2_256, BucketId};

/// Query parameter for file path.
#[derive(Debug, Deserialize)]
pub struct FilePathParam {
    pub path: String,
}

/// Query parameter for directory listing.
#[derive(Debug, Deserialize)]
pub struct ListDirParam {
    #[serde(default = "default_root")]
    pub path: String,
    #[serde(default)]
    pub recursive: bool,
}

fn default_root() -> String {
    "/".to_string()
}

/// PUT /fs/:bucket_id/file?path=...
///
/// Accepts raw bytes, chunks internally, builds Merkle tree, commits to MMR, updates FS index.
pub async fn fs_put_file(
    State(state): State<Arc<ProviderState>>,
    Path(bucket_id): Path<BucketId>,
    Query(params): Query<FilePathParam>,
    headers: axum::http::HeaderMap,
    body: Bytes,
) -> Result<Json<PutFileResponse>, Error> {
    let path = params.path;
    if path.is_empty() || !path.starts_with('/') {
        return Err(Error::InvalidPath("path must start with '/'".to_string()));
    }

    let data = body.to_vec();
    let size = data.len() as u64;

    // Initialize bucket if needed
    state.storage.init_bucket(bucket_id, u64::MAX);

    // 1. Split into chunks (256 KiB)
    let chunk_size = storage_primitives::DEFAULT_CHUNK_SIZE as usize;
    let chunks: Vec<&[u8]> = if data.is_empty() {
        vec![&[]]
    } else {
        data.chunks(chunk_size).collect()
    };

    // 2. Hash and store each chunk
    let chunk_hashes: Vec<H256> = chunks
        .iter()
        .map(|chunk| {
            let hash = blake2_256(chunk);
            state
                .storage
                .store_node(bucket_id, hash, chunk.to_vec(), None)?;
            Ok(hash)
        })
        .collect::<Result<Vec<_>, Error>>()?;

    // 3. Build balanced Merkle tree
    let data_root = build_padded_merkle_tree(&*state.storage, bucket_id, &chunk_hashes);

    // 4. Commit data_root to MMR
    let (_mmr_root, _start_seq, leaf_indices) = state.storage.commit(bucket_id, vec![data_root])?;
    let leaf_index = leaf_indices[0];

    // 5. Extract content type from headers
    let content_type = headers
        .get("content-type")
        .and_then(|v| v.to_str().ok())
        .unwrap_or("application/octet-stream")
        .to_string();

    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs();

    // 6. Create FsEntryMeta and insert into FS index
    let meta = FsEntryMeta {
        entry_type: "file".to_string(),
        data_root,
        size,
        content_type,
        mtime: now,
        leaf_index,
    };

    state.fs_index.put_file(bucket_id, path, meta);

    Ok(Json(PutFileResponse {
        data_root: format!("0x{}", hex_encode(data_root.as_bytes())),
        size,
        leaf_index,
    }))
}

/// GET /fs/:bucket_id/file?path=...
///
/// Reassembles data from stored chunks and returns with Content-Type.
pub async fn fs_get_file(
    State(state): State<Arc<ProviderState>>,
    Path(bucket_id): Path<BucketId>,
    Query(params): Query<FilePathParam>,
) -> Result<Response, Error> {
    let path = params.path;
    let meta = state
        .fs_index
        .get_entry(bucket_id, &path)
        .ok_or_else(|| Error::FileNotFound {
            bucket_id,
            path: path.clone(),
        })?;

    if meta.entry_type != "file" {
        return Err(Error::NotAFile { bucket_id, path });
    }

    // Collect chunks and reassemble
    let chunks = state.storage.collect_chunks(meta.data_root);
    let mut data = Vec::with_capacity(meta.size as usize);
    for chunk in chunks {
        data.extend_from_slice(&chunk);
    }
    data.truncate(meta.size as usize);

    let mut response = (StatusCode::OK, data).into_response();
    let headers = response.headers_mut();
    headers.insert(
        "content-type",
        meta.content_type
            .parse()
            .unwrap_or_else(|_| "application/octet-stream".parse().unwrap()),
    );
    headers.insert(
        "content-length",
        meta.size
            .to_string()
            .parse()
            .unwrap_or_else(|_| "0".parse().unwrap()),
    );

    Ok(response)
}

/// DELETE /fs/:bucket_id/file?path=...
///
/// Removes from FS index. Data stays in MMR for challenge proofs.
pub async fn fs_delete_file(
    State(state): State<Arc<ProviderState>>,
    Path(bucket_id): Path<BucketId>,
    Query(params): Query<FilePathParam>,
) -> Result<Json<DeleteFileResponse>, Error> {
    let path = params.path;
    let deleted = state.fs_index.delete_entry(bucket_id, &path).is_some();

    Ok(Json(DeleteFileResponse { deleted }))
}

/// POST /fs/:bucket_id/mkdir?path=...
///
/// Creates a directory entry in the FS index.
pub async fn fs_mkdir(
    State(state): State<Arc<ProviderState>>,
    Path(bucket_id): Path<BucketId>,
    Query(params): Query<FilePathParam>,
) -> Result<Json<MkdirResponse>, Error> {
    let path = params.path;
    if path.is_empty() || !path.starts_with('/') {
        return Err(Error::InvalidPath("path must start with '/'".to_string()));
    }

    state.fs_index.mkdir(bucket_id, path.clone());

    Ok(Json(MkdirResponse {
        path,
        created: true,
    }))
}

/// GET /fs/:bucket_id/ls?path=...&recursive=false
///
/// List directory contents from the FS index.
pub async fn fs_list_dir(
    State(state): State<Arc<ProviderState>>,
    Path(bucket_id): Path<BucketId>,
    Query(params): Query<ListDirParam>,
) -> Json<ListDirResponse> {
    let entries = state
        .fs_index
        .list_dir(bucket_id, &params.path, params.recursive);

    let (file_count, dir_count, total_size) = state.fs_index.drive_stats(bucket_id);

    Json(ListDirResponse {
        path: params.path,
        entries: entries
            .into_iter()
            .map(|e| ListDirEntry {
                name: e.name,
                path: e.path,
                entry_type: e.entry_type,
                size: e.size,
                mtime: e.mtime,
            })
            .collect(),
        file_count,
        dir_count,
        total_size,
    })
}

/// GET /fs/:bucket_id/index_root
///
/// Returns the metadata Merkle root and stats for checkpoint integration.
pub async fn fs_index_root(
    State(state): State<Arc<ProviderState>>,
    Path(bucket_id): Path<BucketId>,
) -> Json<FsIndexRootResponse> {
    let root = state.fs_index.metadata_merkle_root(bucket_id);
    let (file_count, dir_count, total_size) = state.fs_index.drive_stats(bucket_id);

    Json(FsIndexRootResponse {
        bucket_id,
        metadata_merkle_root: format!("0x{}", hex_encode(root.as_bytes())),
        file_count,
        dir_count,
        total_size,
    })
}

// ─────────────────────────────────────────────────────────────────────────────
// Request/Response types
// ─────────────────────────────────────────────────────────────────────────────

#[derive(Debug, Serialize)]
pub struct PutFileResponse {
    pub data_root: String,
    pub size: u64,
    pub leaf_index: u64,
}

#[derive(Debug, Serialize)]
pub struct DeleteFileResponse {
    pub deleted: bool,
}

#[derive(Debug, Serialize)]
pub struct MkdirResponse {
    pub path: String,
    pub created: bool,
}

#[derive(Debug, Serialize)]
pub struct ListDirEntry {
    pub name: String,
    pub path: String,
    pub entry_type: String,
    pub size: u64,
    pub mtime: u64,
}

#[derive(Debug, Serialize)]
pub struct ListDirResponse {
    pub path: String,
    pub entries: Vec<ListDirEntry>,
    pub file_count: u64,
    pub dir_count: u64,
    pub total_size: u64,
}

#[derive(Debug, Serialize)]
pub struct FsIndexRootResponse {
    pub bucket_id: BucketId,
    pub metadata_merkle_root: String,
    pub file_count: u64,
    pub dir_count: u64,
    pub total_size: u64,
}
