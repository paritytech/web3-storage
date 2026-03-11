//! HTTP API handlers for the provider node.

use crate::checkpoint_coordinator::{
    CheckpointDutyQuery, CheckpointDutyResponse, SignProposalRequest, SignProposalResponse,
};
use crate::error::Error;
use crate::s3_api;
use crate::storage::{hex_decode, hex_encode};
use crate::types::*;
use crate::ProviderState;
use axum::{
    extract::{DefaultBodyLimit, Query, State},
    routing::{get, post, put},
    Json, Router,
};
use base64::{engine::general_purpose::STANDARD as BASE64, Engine};
use codec::Encode;
use sp_core::{Pair, H256};
use std::sync::Arc;
use storage_primitives::{CheckpointProposal, CommitmentPayload};
use tower_http::cors::CorsLayer;
use tower_http::trace::TraceLayer;

/// Create the API router with all endpoints.
pub fn create_router(state: Arc<ProviderState>) -> Router {
    Router::new()
        // Health and info
        .route("/health", get(health))
        .route("/info", get(info))
        .route("/stats", get(stats))
        // Node operations
        .route("/node", get(get_node).put(upload_node))
        .route("/exists", post(check_exists))
        // Commit and read
        .route("/commit", post(commit))
        .route("/read", get(read_chunks))
        // Commitment and proofs
        .route("/commitment", get(get_commitment))
        .route("/checkpoint-signature", get(get_checkpoint_signature))
        .route("/mmr_proof", get(get_mmr_proof))
        .route("/chunk_proof", get(get_chunk_proof))
        // Bucket operations
        .route("/buckets", get(list_buckets))
        .route("/delete", post(delete_data))
        // Replica sync
        .route("/mmr_peaks", get(get_mmr_peaks))
        .route("/mmr_subtree", get(get_mmr_subtree))
        .route("/fetch_nodes", post(fetch_nodes))
        // Checkpoint coordination
        .route("/checkpoint/sign", post(sign_checkpoint_proposal))
        .route("/checkpoint/duty", get(get_checkpoint_duty))
        // Replica sync status
        .route("/replica/historical_roots", get(get_historical_roots))
        .route("/replica/sync_status", get(get_replica_sync_status))
        // S3-compatible object storage (key passed as ?key= query param)
        .route(
            "/s3/:bucket_id/object",
            put(s3_api::s3_put_object)
                .get(s3_api::s3_get_object)
                .head(s3_api::s3_head_object)
                .delete(s3_api::s3_delete_object),
        )
        .route("/s3/:bucket_id/objects", get(s3_api::s3_list_objects))
        .route("/s3/:bucket_id/index_root", get(s3_api::s3_index_root))
        .layer(DefaultBodyLimit::max(256 * 1024 * 1024)) // 256 MB
        .layer(TraceLayer::new_for_http())
        .layer(CorsLayer::permissive())
        .with_state(state)
}

// ─────────────────────────────────────────────────────────────────────────────
// Health and Info
// ─────────────────────────────────────────────────────────────────────────────

async fn health() -> Json<HealthResponse> {
    Json(HealthResponse {
        status: "healthy".to_string(),
        version: env!("CARGO_PKG_VERSION").to_string(),
    })
}

async fn info() -> Json<InfoResponse> {
    Json(InfoResponse {
        status: "healthy".to_string(),
        version: env!("CARGO_PKG_VERSION").to_string(),
    })
}

async fn stats(State(state): State<Arc<ProviderState>>) -> Json<StatsResponse> {
    let bucket_stats = state.storage.get_bucket_stats();
    let total_bytes = state.storage.total_bytes();
    let total_nodes = state.storage.total_nodes();

    Json(StatsResponse {
        provider_id: state.provider_id.clone(),
        total_buckets: bucket_stats.len(),
        total_nodes,
        total_bytes,
        buckets: bucket_stats,
    })
}

// ─────────────────────────────────────────────────────────────────────────────
// Node Operations
// ─────────────────────────────────────────────────────────────────────────────

#[derive(serde::Deserialize)]
struct GetNodeQuery {
    hash: String,
}

async fn get_node(
    State(state): State<Arc<ProviderState>>,
    Query(query): Query<GetNodeQuery>,
) -> Result<Json<DownloadNodeResponse>, Error> {
    let hash_bytes = hex_decode(&query.hash).map_err(|_| Error::InvalidHash {
        expected: query.hash.clone(),
        actual: "invalid hex".to_string(),
    })?;
    let hash = H256::from_slice(&hash_bytes);

    let node = state
        .storage
        .get_node(&hash)
        .ok_or_else(|| Error::NodeNotFound(query.hash.clone()))?;

    Ok(Json(DownloadNodeResponse {
        hash: query.hash,
        data: BASE64.encode(&node.data),
        children: node.children.map(|c| {
            c.iter()
                .map(|h| format!("0x{}", hex_encode(h.as_bytes())))
                .collect()
        }),
    }))
}

async fn upload_node(
    State(state): State<Arc<ProviderState>>,
    Json(request): Json<UploadNodeRequest>,
) -> Result<Json<UploadNodeResponse>, Error> {
    // Decode hash
    let hash_bytes = hex_decode(&request.hash).map_err(|_| Error::InvalidHash {
        expected: request.hash.clone(),
        actual: "invalid hex".to_string(),
    })?;
    let hash = H256::from_slice(&hash_bytes);

    // Decode data
    let data = BASE64
        .decode(&request.data)
        .map_err(|e| Error::Serialization(e.to_string()))?;

    // Decode children
    let children = request
        .children
        .map(|c| {
            c.iter()
                .map(|h| {
                    let bytes = hex_decode(h).map_err(|_| Error::InvalidHash {
                        expected: h.clone(),
                        actual: "invalid hex".to_string(),
                    })?;
                    Ok(H256::from_slice(&bytes))
                })
                .collect::<Result<Vec<_>, Error>>()
        })
        .transpose()?;

    // Initialize bucket if needed
    state.storage.init_bucket(request.bucket_id, u64::MAX);

    // Store node
    state
        .storage
        .store_node(request.bucket_id, hash, data, children)?;

    Ok(Json(UploadNodeResponse { stored: true }))
}

async fn check_exists(
    State(state): State<Arc<ProviderState>>,
    Json(request): Json<ExistsRequest>,
) -> Result<Json<ExistsResponse>, Error> {
    let hashes: Vec<H256> = request
        .hashes
        .iter()
        .map(|h| {
            let bytes = hex_decode(h).map_err(|_| Error::InvalidHash {
                expected: h.clone(),
                actual: "invalid hex".to_string(),
            })?;
            Ok(H256::from_slice(&bytes))
        })
        .collect::<Result<Vec<_>, Error>>()?;

    let (exists, missing) = state.storage.check_exists(request.bucket_id, &hashes);

    Ok(Json(ExistsResponse {
        exists: exists
            .iter()
            .map(|h| format!("0x{}", hex_encode(h.as_bytes())))
            .collect(),
        missing: missing
            .iter()
            .map(|h| format!("0x{}", hex_encode(h.as_bytes())))
            .collect(),
    }))
}

// ─────────────────────────────────────────────────────────────────────────────
// Commit and Read
// ─────────────────────────────────────────────────────────────────────────────

async fn commit(
    State(state): State<Arc<ProviderState>>,
    Json(request): Json<CommitRequest>,
) -> Result<Json<CommitResponse>, Error> {
    let data_roots: Vec<H256> = request
        .data_roots
        .iter()
        .map(|h| {
            let bytes = hex_decode(h).map_err(|_| Error::InvalidHash {
                expected: h.clone(),
                actual: "invalid hex".to_string(),
            })?;
            Ok(H256::from_slice(&bytes))
        })
        .collect::<Result<Vec<_>, Error>>()?;

    let (mmr_root, start_seq, leaf_indices) =
        state.storage.commit(request.bucket_id, data_roots)?;

    // Create commitment payload and sign it
    // Note: leaf_count is set to 0 to match pallet's challenge_offchain verification
    let payload = CommitmentPayload::new(request.bucket_id, mmr_root, start_seq, 0);
    let signature = state.sign(&payload.encode());

    Ok(Json(CommitResponse {
        mmr_root: format!("0x{}", hex_encode(mmr_root.as_bytes())),
        start_seq,
        leaf_indices,
        provider_signature: signature,
    }))
}

async fn read_chunks(
    State(state): State<Arc<ProviderState>>,
    Query(query): Query<ReadQuery>,
) -> Result<Json<ReadResponse>, Error> {
    let root_bytes = hex_decode(&query.data_root).map_err(|_| Error::InvalidHash {
        expected: query.data_root.clone(),
        actual: "invalid hex".to_string(),
    })?;
    let data_root = H256::from_slice(&root_bytes);

    // Calculate chunk indices
    let chunk_size = storage_primitives::DEFAULT_CHUNK_SIZE as u64;
    let start_chunk = query.offset / chunk_size;
    let end_chunk = (query.offset + query.length).div_ceil(chunk_size);

    let mut chunks = Vec::new();
    for chunk_idx in start_chunk..end_chunk {
        match state.storage.get_chunk_at_index(data_root, chunk_idx) {
            Ok((data, proof)) => {
                chunks.push(ChunkWithProof {
                    hash: format!(
                        "0x{}",
                        hex_encode(storage_primitives::blake2_256(&data).as_bytes())
                    ),
                    data: BASE64.encode(&data),
                    proof: proof
                        .siblings
                        .iter()
                        .map(|h| format!("0x{}", hex_encode(h.as_bytes())))
                        .collect(),
                });
            }
            Err(_) => break,
        }
    }

    Ok(Json(ReadResponse { chunks }))
}

// ─────────────────────────────────────────────────────────────────────────────
// Commitment and Proofs
// ─────────────────────────────────────────────────────────────────────────────

async fn get_commitment(
    State(state): State<Arc<ProviderState>>,
    Query(query): Query<CommitmentQuery>,
) -> Result<Json<CommitmentResponse>, Error> {
    let bucket = state
        .storage
        .get_bucket(query.bucket_id)
        .ok_or(Error::BucketNotFound(query.bucket_id))?;

    // Create commitment payload and sign it
    // Note: leaf_count is set to 0 to match pallet's challenge_offchain verification
    let payload = CommitmentPayload::new(query.bucket_id, bucket.mmr_root, bucket.start_seq, 0);
    let signature = state.sign(&payload.encode());

    Ok(Json(CommitmentResponse {
        bucket_id: query.bucket_id,
        mmr_root: format!("0x{}", hex_encode(bucket.mmr_root.as_bytes())),
        start_seq: bucket.start_seq,
        leaf_count: bucket.leaf_count(),
        provider_signature: signature,
    }))
}

/// Return a checkpoint-compatible signature (signs with real leaf_count).
///
/// Unlike `/commitment` which signs with leaf_count=0 for challenge_offchain,
/// this endpoint signs with the actual leaf_count so the signature can be used
/// in the on-chain `checkpoint` extrinsic.
async fn get_checkpoint_signature(
    State(state): State<Arc<ProviderState>>,
    Query(query): Query<CommitmentQuery>,
) -> Result<Json<CheckpointSignatureResponse>, Error> {
    let bucket = state
        .storage
        .get_bucket(query.bucket_id)
        .ok_or(Error::BucketNotFound(query.bucket_id))?;

    let leaf_count = bucket.leaf_count();

    // Sign with real leaf_count for on-chain checkpoint verification
    let payload = CommitmentPayload::new(
        query.bucket_id,
        bucket.mmr_root,
        bucket.start_seq,
        leaf_count,
    );
    let signature = state.sign(&payload.encode());

    Ok(Json(CheckpointSignatureResponse {
        bucket_id: query.bucket_id,
        mmr_root: format!("0x{}", hex_encode(bucket.mmr_root.as_bytes())),
        start_seq: bucket.start_seq,
        leaf_count,
        provider_signature: signature,
    }))
}

async fn get_mmr_proof(
    State(state): State<Arc<ProviderState>>,
    Query(query): Query<MmrProofQuery>,
) -> Result<Json<MmrProofResponse>, Error> {
    let mmr_proof = state
        .storage
        .get_mmr_proof(query.bucket_id, query.leaf_index)?;

    Ok(Json(MmrProofResponse {
        leaf: MmrLeafData {
            data_root: format!("0x{}", hex_encode(mmr_proof.leaf.data_root.as_bytes())),
            data_size: mmr_proof.leaf.data_size,
            total_size: mmr_proof.leaf.total_size,
        },
        proof: MmrProofData {
            peaks: mmr_proof
                .peaks
                .iter()
                .map(|h| format!("0x{}", hex_encode(h.as_bytes())))
                .collect(),
            siblings: mmr_proof
                .leaf_proof
                .siblings
                .iter()
                .map(|h| format!("0x{}", hex_encode(h.as_bytes())))
                .collect(),
            path: mmr_proof.leaf_proof.path,
        },
    }))
}

async fn get_chunk_proof(
    State(state): State<Arc<ProviderState>>,
    Query(query): Query<ChunkProofQuery>,
) -> Result<Json<ChunkProofResponse>, Error> {
    let root_bytes = hex_decode(&query.data_root).map_err(|_| Error::InvalidHash {
        expected: query.data_root.clone(),
        actual: "invalid hex".to_string(),
    })?;
    let data_root = H256::from_slice(&root_bytes);

    let (chunk_data, proof) = state
        .storage
        .get_chunk_at_index(data_root, query.chunk_index)?;
    let chunk_hash = storage_primitives::blake2_256(&chunk_data);

    Ok(Json(ChunkProofResponse {
        chunk_hash: format!("0x{}", hex_encode(chunk_hash.as_bytes())),
        chunk_data: Some(BASE64.encode(&chunk_data)),
        proof: MerkleProofData {
            siblings: proof
                .siblings
                .iter()
                .map(|h| format!("0x{}", hex_encode(h.as_bytes())))
                .collect(),
            path: proof.path,
        },
    }))
}

// ─────────────────────────────────────────────────────────────────────────────
// Bucket Operations
// ─────────────────────────────────────────────────────────────────────────────

async fn list_buckets(State(state): State<Arc<ProviderState>>) -> Json<ListBucketsResponse> {
    Json(ListBucketsResponse {
        buckets: state.storage.list_buckets(),
    })
}

async fn delete_data(
    State(state): State<Arc<ProviderState>>,
    Json(request): Json<DeleteRequest>,
) -> Result<Json<DeleteResponse>, Error> {
    // Note: In production, would verify admin_signature
    let _ = request.admin_signature;

    let (mmr_root, start_seq, leaf_count) = state
        .storage
        .delete_before(request.bucket_id, request.new_start_seq)?;

    // Create commitment payload and sign it
    // Note: leaf_count is set to 0 to match pallet's challenge_offchain verification
    let payload = CommitmentPayload::new(request.bucket_id, mmr_root, start_seq, 0);
    let signature = state.sign(&payload.encode());

    Ok(Json(DeleteResponse {
        mmr_root: format!("0x{}", hex_encode(mmr_root.as_bytes())),
        start_seq,
        leaf_count,
        provider_signature: signature,
    }))
}

// ─────────────────────────────────────────────────────────────────────────────
// Replica Sync
// ─────────────────────────────────────────────────────────────────────────────

async fn get_mmr_peaks(
    State(state): State<Arc<ProviderState>>,
    Query(query): Query<MmrPeaksQuery>,
) -> Result<Json<MmrPeaksResponse>, Error> {
    let (mmr_root, peaks) = state.storage.get_mmr_peaks(query.bucket_id)?;

    Ok(Json(MmrPeaksResponse {
        bucket_id: query.bucket_id,
        mmr_root: format!("0x{}", hex_encode(mmr_root.as_bytes())),
        peaks: peaks
            .iter()
            .map(|h| format!("0x{}", hex_encode(h.as_bytes())))
            .collect(),
    }))
}

async fn get_mmr_subtree(
    State(state): State<Arc<ProviderState>>,
    Query(query): Query<MmrSubtreeQuery>,
) -> Result<Json<MmrSubtreeResponse>, Error> {
    // Simplified implementation
    let bucket = state
        .storage
        .get_bucket(query.bucket_id)
        .ok_or(Error::BucketNotFound(query.bucket_id))?;

    Ok(Json(MmrSubtreeResponse {
        nodes: vec![MmrNode {
            position: 0,
            hash: format!("0x{}", hex_encode(bucket.mmr_root.as_bytes())),
            children: None,
        }],
    }))
}

async fn fetch_nodes(
    State(state): State<Arc<ProviderState>>,
    Json(request): Json<FetchNodesRequest>,
) -> Result<Json<FetchNodesResponse>, Error> {
    let mut nodes = Vec::new();

    for hash_str in &request.hashes {
        let hash_bytes = hex_decode(hash_str).map_err(|_| Error::InvalidHash {
            expected: hash_str.clone(),
            actual: "invalid hex".to_string(),
        })?;
        let hash = H256::from_slice(&hash_bytes);

        if let Some(node) = state.storage.get_node(&hash) {
            nodes.push(FetchedNode {
                hash: hash_str.clone(),
                data: BASE64.encode(&node.data),
                children: node.children.map(|c| {
                    c.iter()
                        .map(|h| format!("0x{}", hex_encode(h.as_bytes())))
                        .collect()
                }),
            });
        }
    }

    Ok(Json(FetchNodesResponse { nodes }))
}

// ─────────────────────────────────────────────────────────────────────────────
// Checkpoint Coordination
// ─────────────────────────────────────────────────────────────────────────────

/// Sign a checkpoint proposal from another provider.
///
/// Verifies that the proposal matches our local state and returns a signature
/// if agreed, or disagreement info if our state differs.
async fn sign_checkpoint_proposal(
    State(state): State<Arc<ProviderState>>,
    Json(request): Json<SignProposalRequest>,
) -> Result<Json<SignProposalResponse>, Error> {
    // Get our local bucket state
    let bucket = state
        .storage
        .get_bucket(request.bucket_id)
        .ok_or(Error::BucketNotFound(request.bucket_id))?;

    let local_mmr_root = format!("0x{}", hex_encode(bucket.mmr_root.as_bytes()));

    // Check if we agree with the proposal
    let proposed_root_bytes = hex_decode(&request.mmr_root).map_err(|_| Error::InvalidHash {
        expected: request.mmr_root.clone(),
        actual: "invalid hex".to_string(),
    })?;
    let proposed_root = H256::from_slice(&proposed_root_bytes);

    // We agree if MMR roots match and sequence numbers are compatible
    let agreed = bucket.mmr_root == proposed_root
        && bucket.start_seq == request.start_seq
        && bucket.leaf_count() == request.leaf_count;

    if !agreed {
        return Ok(Json(SignProposalResponse {
            signer: state.provider_id.clone(),
            signature: String::new(),
            agreed: false,
            local_mmr_root: Some(local_mmr_root),
        }));
    }

    // Sign the proposal
    let proposal = CheckpointProposal::new(
        request.bucket_id,
        proposed_root,
        request.start_seq,
        request.leaf_count,
        request.window,
    );
    let encoded = proposal.encode();

    let signature = match &state.keypair {
        Some(kp) => {
            let sig = kp.sign(&encoded);
            format!("0x{}", hex::encode(sig.0))
        }
        None => {
            // No keypair configured - return placeholder
            format!("0x{}", hex::encode([0u8; 64]))
        }
    };

    Ok(Json(SignProposalResponse {
        signer: state.provider_id.clone(),
        signature,
        agreed: true,
        local_mmr_root: Some(local_mmr_root),
    }))
}

/// Get checkpoint duty information for a bucket.
///
/// Returns the current state that would be used for a checkpoint.
async fn get_checkpoint_duty(
    State(state): State<Arc<ProviderState>>,
    Query(query): Query<CheckpointDutyQuery>,
) -> Result<Json<CheckpointDutyResponse>, Error> {
    let bucket = state
        .storage
        .get_bucket(query.bucket_id)
        .ok_or(Error::BucketNotFound(query.bucket_id))?;

    // We're ready if we have data committed
    let ready = bucket.leaf_count() > 0;

    Ok(Json(CheckpointDutyResponse {
        bucket_id: query.bucket_id,
        mmr_root: format!("0x{}", hex_encode(bucket.mmr_root.as_bytes())),
        start_seq: bucket.start_seq,
        leaf_count: bucket.leaf_count(),
        ready,
    }))
}

// ─────────────────────────────────────────────────────────────────────────────
// Replica Sync Endpoints
// ─────────────────────────────────────────────────────────────────────────────

/// Get historical roots for a bucket.
///
/// Returns the current root (position 0) and historical roots (positions 1-6).
/// Note: Provider nodes don't track historical roots; only the chain does.
async fn get_historical_roots(
    State(state): State<Arc<ProviderState>>,
    Query(query): Query<HistoricalRootsQuery>,
) -> Result<Json<HistoricalRootsResponse>, Error> {
    let bucket = state
        .storage
        .get_bucket(query.bucket_id)
        .ok_or(Error::BucketNotFound(query.bucket_id))?;

    Ok(Json(HistoricalRootsResponse {
        bucket_id: query.bucket_id,
        current_root: format!("0x{}", hex_encode(bucket.mmr_root.as_bytes())),
        // Provider node doesn't track historical roots - chain does
        historical_roots: [
            String::new(),
            String::new(),
            String::new(),
            String::new(),
            String::new(),
            String::new(),
        ],
        snapshot_block: 0, // Would need chain query for actual block
    }))
}

/// Get replica sync status for a bucket.
///
/// Returns the local MMR state and sync status.
async fn get_replica_sync_status(
    State(state): State<Arc<ProviderState>>,
    Query(query): Query<BucketSyncStatusQuery>,
) -> Result<Json<BucketSyncStatusResponse>, Error> {
    let bucket = state
        .storage
        .get_bucket(query.bucket_id)
        .ok_or(Error::BucketNotFound(query.bucket_id))?;

    Ok(Json(BucketSyncStatusResponse {
        bucket_id: query.bucket_id,
        local_mmr_root: format!("0x{}", hex_encode(bucket.mmr_root.as_bytes())),
        local_leaf_count: bucket.leaf_count(),
        last_sync_block: None, // Would be tracked by coordinator
        syncing: false,        // Would check coordinator state
    }))
}
