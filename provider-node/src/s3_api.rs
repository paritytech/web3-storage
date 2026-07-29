// SPDX-License-Identifier: GPL-3.0-only

//! S3-compatible HTTP API handlers.
//!
//! These endpoints handle the full upload flow (chunking, tree-building, commit, index update)
//! so clients just send raw bytes + key — no multi-request upload dance.
//!
//! Object key is passed as a `?key=` query parameter on all object endpoints.
//! Example: `PUT /s3/1/object?key=photos/cat.jpg`

use crate::api::check_role;
use crate::auth::RequiredRole;
use crate::error::Error;
use crate::ProviderState;
use axum::{
    body::Bytes,
    extract::{Path, Query, State},
    http::{header::HeaderName, HeaderMap, HeaderValue, StatusCode},
    response::{IntoResponse, Response},
    Json,
};
use provider_storage::{build_padded_merkle_tree, ListResult, ObjectMeta};
use serde::{Deserialize, Serialize};
use sp_core::H256;
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};
use storage_primitives::{blake2_256, BucketId};

/// Query parameter for object key.
#[derive(Debug, Deserialize)]
pub struct ObjectKeyParam {
    pub key: String,
}

/// PUT /s3/:bucket_id/object?key=...
///
/// Accepts raw bytes, chunks internally, builds Merkle tree, commits to MMR, updates index.
pub async fn s3_put_object(
    State(state): State<Arc<ProviderState>>,
    Path(bucket_id): Path<BucketId>,
    Query(params): Query<ObjectKeyParam>,
    headers: HeaderMap,
    body: Bytes,
) -> Result<Json<PutObjectResponse>, Error> {
    check_role(&state, &headers, "PUT", bucket_id, RequiredRole::Writer).await?;
    let key = params.key;
    if key.is_empty() {
        return Err(Error::InvalidObjectKey("empty key".to_string()));
    }

    let data = body.to_vec();
    let size = data.len() as u64;

    // Initialize bucket if needed
    let _ = state.storage.init_bucket(bucket_id, u64::MAX);

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

    // 5. Extract metadata from headers
    let content_type = headers
        .get("content-type")
        .and_then(|v| v.to_str().ok())
        .unwrap_or("application/octet-stream")
        .to_string();

    let user_metadata: Vec<(String, String)> = headers
        .iter()
        .filter_map(|(name, value)| {
            let name_str = name.as_str().to_lowercase();
            if let Some(meta_key) = name_str.strip_prefix("x-amz-meta-") {
                value
                    .to_str()
                    .ok()
                    .map(|v| (meta_key.to_string(), v.to_string()))
            } else {
                None
            }
        })
        .collect();

    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs();

    let etag = format!("0x{}", hex::encode(data_root.as_bytes()));

    // 6. Create ObjectMeta and insert into index
    let meta = ObjectMeta {
        data_root,
        size,
        content_type,
        etag: etag.clone(),
        last_modified: now,
        user_metadata,
        leaf_index,
    };

    state.s3_index.put_object(bucket_id, key, meta);

    Ok(Json(PutObjectResponse {
        etag,
        data_root: format!("0x{}", hex::encode(data_root.as_bytes())),
        size,
        leaf_index,
    }))
}

/// GET /s3/:bucket_id/object?key=...
///
/// Reassembles data from stored chunks and returns with Content-Type.
pub async fn s3_get_object(
    State(state): State<Arc<ProviderState>>,
    Path(bucket_id): Path<BucketId>,
    Query(params): Query<ObjectKeyParam>,
    headers: HeaderMap,
) -> Result<Response, Error> {
    check_role(&state, &headers, "GET", bucket_id, RequiredRole::Reader).await?;
    let key = params.key;
    let meta = state
        .s3_index
        .get_object(bucket_id, &key)
        .ok_or_else(|| Error::ObjectNotFound {
            bucket_id,
            key: key.clone(),
        })?;

    // Collect chunks and reassemble
    let chunks = state.storage.collect_chunks(meta.data_root);
    let mut data = Vec::with_capacity(meta.size as usize);
    for chunk in chunks {
        data.extend_from_slice(&chunk);
    }
    // Truncate to original size
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
        "etag",
        meta.etag.parse().unwrap_or_else(|_| "".parse().unwrap()),
    );
    headers.insert(
        "last-modified",
        meta.last_modified
            .to_string()
            .parse()
            .unwrap_or_else(|_| "0".parse().unwrap()),
    );
    headers.insert(
        "content-length",
        meta.size
            .to_string()
            .parse()
            .unwrap_or_else(|_| "0".parse().unwrap()),
    );

    // Add user metadata headers
    for (k, v) in &meta.user_metadata {
        let header_name = format!("x-amz-meta-{k}");
        if let (Ok(name), Ok(val)) = (header_name.parse::<HeaderName>(), v.parse::<HeaderValue>()) {
            headers.insert(name, val);
        }
    }

    Ok(response)
}

/// HEAD /s3/:bucket_id/object?key=...
///
/// Returns metadata headers without body.
pub async fn s3_head_object(
    State(state): State<Arc<ProviderState>>,
    Path(bucket_id): Path<BucketId>,
    Query(params): Query<ObjectKeyParam>,
    headers: HeaderMap,
) -> Result<Response, Error> {
    check_role(&state, &headers, "HEAD", bucket_id, RequiredRole::Reader).await?;
    let key = params.key;
    let meta = state
        .s3_index
        .get_object(bucket_id, &key)
        .ok_or_else(|| Error::ObjectNotFound {
            bucket_id,
            key: key.clone(),
        })?;

    let mut response = StatusCode::OK.into_response();
    let headers = response.headers_mut();
    headers.insert(
        "content-type",
        meta.content_type
            .parse()
            .unwrap_or_else(|_| "application/octet-stream".parse().unwrap()),
    );
    headers.insert(
        "etag",
        meta.etag.parse().unwrap_or_else(|_| "".parse().unwrap()),
    );
    headers.insert(
        "last-modified",
        meta.last_modified
            .to_string()
            .parse()
            .unwrap_or_else(|_| "0".parse().unwrap()),
    );
    headers.insert(
        "content-length",
        meta.size
            .to_string()
            .parse()
            .unwrap_or_else(|_| "0".parse().unwrap()),
    );
    headers.insert(
        "x-amz-data-root",
        format!("0x{}", hex::encode(meta.data_root.as_bytes()))
            .parse()
            .unwrap_or_else(|_| "".parse().unwrap()),
    );

    for (k, v) in &meta.user_metadata {
        let header_name = format!("x-amz-meta-{k}");
        if let (Ok(name), Ok(val)) = (header_name.parse::<HeaderName>(), v.parse::<HeaderValue>()) {
            headers.insert(name, val);
        }
    }

    Ok(response)
}

/// DELETE /s3/:bucket_id/object?key=...
///
/// Removes from index. Data stays in MMR for challenge proofs.
pub async fn s3_delete_object(
    State(state): State<Arc<ProviderState>>,
    Path(bucket_id): Path<BucketId>,
    Query(params): Query<ObjectKeyParam>,
    headers: HeaderMap,
) -> Result<Json<DeleteObjectResponse>, Error> {
    check_role(&state, &headers, "DELETE", bucket_id, RequiredRole::Writer).await?;
    let key = params.key;
    let deleted = state.s3_index.delete_object(bucket_id, &key).is_some();

    Ok(Json(DeleteObjectResponse { deleted }))
}

/// GET /s3/:bucket_id/objects
///
/// List objects with S3-compatible query parameters.
pub async fn s3_list_objects(
    State(state): State<Arc<ProviderState>>,
    Path(bucket_id): Path<BucketId>,
    Query(params): Query<ListObjectsParams>,
    headers: HeaderMap,
) -> Result<Json<ListResult>, Error> {
    check_role(&state, &headers, "GET", bucket_id, RequiredRole::Reader).await?;
    let result = state.s3_index.list_objects(
        bucket_id,
        params.prefix.as_deref(),
        params.delimiter.as_deref(),
        params.start_after.as_deref(),
        params.continuation_token.as_deref(),
        params.max_keys.unwrap_or(1000),
    );

    Ok(Json(result))
}

/// GET /s3/:bucket_id/index_root
///
/// Returns the metadata Merkle root and stats for checkpoint integration.
pub async fn s3_index_root(
    State(state): State<Arc<ProviderState>>,
    Path(bucket_id): Path<BucketId>,
    headers: HeaderMap,
) -> Result<Json<IndexRootResponse>, Error> {
    check_role(&state, &headers, "GET", bucket_id, RequiredRole::Reader).await?;
    let root = state.s3_index.metadata_merkle_root(bucket_id);
    let (object_count, total_size) = state.s3_index.bucket_stats(bucket_id);

    Ok(Json(IndexRootResponse {
        bucket_id,
        metadata_merkle_root: format!("0x{}", hex::encode(root.as_bytes())),
        object_count,
        total_size,
    }))
}

// ─────────────────────────────────────────────────────────────────────────────
// Request/Response types
// ─────────────────────────────────────────────────────────────────────────────

#[derive(Debug, Serialize)]
pub struct PutObjectResponse {
    pub etag: String,
    pub data_root: String,
    pub size: u64,
    pub leaf_index: u64,
}

#[derive(Debug, Serialize)]
pub struct DeleteObjectResponse {
    pub deleted: bool,
}

#[derive(Debug, Deserialize)]
pub struct ListObjectsParams {
    pub prefix: Option<String>,
    pub delimiter: Option<String>,
    pub start_after: Option<String>,
    pub continuation_token: Option<String>,
    pub max_keys: Option<u32>,
}

#[derive(Debug, Serialize)]
pub struct IndexRootResponse {
    pub bucket_id: BucketId,
    pub metadata_merkle_root: String,
    pub object_count: u64,
    pub total_size: u64,
}
