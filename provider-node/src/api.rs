// SPDX-License-Identifier: GPL-3.0-only

//! HTTP API handlers for the provider node.

use crate::auth::{self, RequiredRole};
use crate::checkpoint_coordinator::{
    CheckpointDutyQuery, CheckpointDutyResponse, SignProposalRequest, SignProposalResponse,
};
use crate::error::Error;
use crate::fs_api;
use crate::negotiate::{self, AgreementTermsOf, NegotiateRequest, SignedTerms};
use crate::s3_api;
use crate::storage::{hex_decode, hex_encode};
use crate::types::*;
use crate::ProviderState;
use axum::{
    extract::{ConnectInfo, DefaultBodyLimit, Query, Request, State},
    middleware::{from_fn_with_state, Next},
    response::Response,
    routing::{get, post, put},
    Json, Router,
};
use base64::{engine::general_purpose::STANDARD as BASE64, Engine};
use codec::Encode;
use sp_core::H256;
use std::net::SocketAddr;
use std::sync::Arc;
use storage_primitives::AgreementTerms;
use storage_primitives::{CheckpointProposal, CommitmentPayload};
use tokio_rate_limit::RateLimiter;
use tower_http::cors::CorsLayer;
use tower_http::trace::TraceLayer;

/// Sustained `/negotiate` requests allowed per-IP / second (token refill rate).
const NEGOTIATE_RATE_LIMIT_PER_SEC: u64 = 5;
/// Per-IP token-bucket capacity: a single client may burst up to this many
/// `/negotiate` requests before being throttled to the refill rate.
const NEGOTIATE_RATE_LIMIT_BURST: u64 = 16;

/// Create the API router with all endpoints.
pub fn create_router(state: Arc<ProviderState>) -> Router {
    // Per-IP token-bucket limiter for `/negotiate`.
    let negotiate_limiter = Arc::new(
        RateLimiter::builder()
            .requests_per_second(NEGOTIATE_RATE_LIMIT_PER_SEC)
            .burst(NEGOTIATE_RATE_LIMIT_BURST)
            .build()
            .expect("static `/negotiate` rate-limit config is valid"),
    );

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
        // L1 offer/want + L3 push protocol
        .route("/sync/offer", post(sync_offer))
        .route("/sync/want", post(sync_want))
        .route("/sync/push", post(sync_push))
        // Off-chain term negotiation (signed AgreementTerms for `establish_storage_agreement`)
        .route(
            "/negotiate",
            post(negotiate_terms).layer(from_fn_with_state(
                negotiate_limiter,
                rate_limit_by_ip_middleware,
            )),
        )
        // Checkpoint coordination
        .route("/checkpoint/sign", post(sign_checkpoint_proposal))
        .route("/checkpoint/duty", get(get_checkpoint_duty))
        .route("/checkpoint/trigger", post(trigger_checkpoint))
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
        // File system endpoints (path passed as ?path= query param)
        .route(
            "/fs/:bucket_id/file",
            put(fs_api::fs_put_file)
                .get(fs_api::fs_get_file)
                .delete(fs_api::fs_delete_file),
        )
        .route("/fs/:bucket_id/mkdir", post(fs_api::fs_mkdir))
        .route("/fs/:bucket_id/ls", get(fs_api::fs_list_dir))
        .route("/fs/:bucket_id/index_root", get(fs_api::fs_index_root))
        .layer(DefaultBodyLimit::max(256 * 1024 * 1024)) // 256 MB
        .layer(TraceLayer::new_for_http())
        .layer(CorsLayer::permissive())
        .with_state(state)
}

async fn rate_limit_by_ip_middleware(
    State(limiter): State<Arc<RateLimiter>>,
    ConnectInfo(addr): ConnectInfo<SocketAddr>,
    request: Request,
    next: Next,
) -> Result<Response, Error> {
    let ip = addr.ip().to_string();
    match limiter.check(&ip).await {
        Ok(decision) if decision.permitted => Ok(next.run(request).await),
        Ok(_) => Err(Error::RateLimited),
        // Fail closed: a limiter-internal error rejects the request rather than
        // letting unbounded traffic through, which could overwhelm the node and
        // hide detail error from client.
        Err(err) => {
            tracing::warn!("rate limiter error for {ip}: {err}; rejecting request");
            Err(Error::Internal("RateLimited".to_string()))
        }
    }
}

/// Extract the Authorization header value from a header map.
pub(crate) fn auth_header(headers: &axum::http::HeaderMap) -> Option<&str> {
    headers
        .get(axum::http::header::AUTHORIZATION)
        .and_then(|v| v.to_str().ok())
}

/// Convenience wrapper around `auth::require_role` using request headers.
pub(crate) async fn check_role(
    state: &ProviderState,
    headers: &axum::http::HeaderMap,
    method: &str,
    bucket_id: u64,
    required: RequiredRole,
) -> Result<(), Error> {
    auth::require_role(
        state,
        auth_header(headers),
        method,
        bucket_id,
        required,
        state.auth_max_skew,
    )
    .await
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

async fn info(State(state): State<Arc<ProviderState>>) -> Json<InfoResponse> {
    let provider_registration_info = state.chain_state.provider_info.read().clone();

    Json(InfoResponse {
        provider_id: state.provider_id.clone(),
        readiness: ProviderReadiness {
            signing_configured: state.keypair.is_some(),
            nonce_counter_ready: state.chain_state.nonce_counter.read().is_some(),
            provider_info_loaded: provider_registration_info.is_some(),
            deregistering: provider_registration_info
                .as_ref()
                .is_some_and(|info| info.deregister_at.is_some()),
        },
        provider_registration_info,
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
    state.storage.init_bucket(request.bucket_id, u64::MAX)?;

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
    let signature = state.sign(&payload.encode())?;

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
    let signature = state.sign(&payload.encode())?;

    Ok(Json(CommitmentResponse {
        bucket_id: query.bucket_id,
        mmr_root: format!("0x{}", hex_encode(bucket.mmr_root.as_bytes())),
        start_seq: bucket.start_seq,
        leaf_count: bucket.leaf_count,
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

    let leaf_count = bucket.leaf_count;

    // Sign with real leaf_count for on-chain checkpoint verification
    let payload = CommitmentPayload::new(
        query.bucket_id,
        bucket.mmr_root,
        bucket.start_seq,
        leaf_count,
    );
    let signature = state.sign(&payload.encode())?;

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
    let signature = state.sign(&payload.encode())?;

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
// L1 offer/want sync protocol
// ─────────────────────────────────────────────────────────────────────────────

/// POST /sync/offer
///
/// The requester tells us how many leaves it holds. We enumerate all
/// content-node hashes under every leaf the requester is missing and return
/// them as the offer, along with the ordered data_roots so the requester can
/// commit after transfer.
async fn sync_offer(
    State(state): State<Arc<ProviderState>>,
    Json(req): Json<SyncOfferRequest>,
) -> Result<Json<SyncOfferResponse>, Error> {
    let data_roots = state
        .storage
        .get_data_roots_from(req.bucket_id, req.leaf_count)?;

    let mut hashes: Vec<String> = Vec::new();
    for &dr in &data_roots {
        for h in state.storage.collect_all_node_hashes(dr) {
            hashes.push(format!("0x{}", hex_encode(h.as_bytes())));
        }
    }
    hashes.dedup();

    Ok(Json(SyncOfferResponse {
        hashes,
        data_roots: data_roots
            .iter()
            .map(|h| format!("0x{}", hex_encode(h.as_bytes())))
            .collect(),
    }))
}

/// POST /sync/want
///
/// The requester sends the subset of offered hashes it actually lacks.
/// We return those nodes in a batch. Identical to /fetch_nodes semantically
/// but with explicit L1 protocol framing.
async fn sync_want(
    State(state): State<Arc<ProviderState>>,
    Json(req): Json<SyncWantRequest>,
) -> Result<Json<SyncWantResponse>, Error> {
    let mut nodes = Vec::new();
    for hash_str in &req.hashes {
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
    Ok(Json(SyncWantResponse { nodes }))
}

/// POST /sync/push
///
/// L3 push: the writer sends a batch of nodes to this peer for immediate
/// replication and expects a signed custody receipt back. The peer stores the
/// nodes and signs `blake2_256(bucket_id_le ++ committed_root)` to prove it
/// accepted custody.
async fn sync_push(
    State(state): State<Arc<ProviderState>>,
    Json(req): Json<SyncPushRequest>,
) -> Result<Json<CustodyReceipt>, Error> {
    use storage_primitives::blake2_256;

    // Ensure the bucket exists before storing.
    let _ = state.storage.init_bucket(req.bucket_id, u64::MAX);

    // Store the pushed nodes bottom-up (reverse DFS pre-order).
    let mut to_store: Vec<(H256, Vec<u8>, Option<Vec<H256>>)> = Vec::new();
    for node in &req.nodes {
        let hash_bytes = hex_decode(&node.hash).map_err(|_| Error::InvalidHash {
            expected: node.hash.clone(),
            actual: "invalid hex".to_string(),
        })?;
        let hash = H256::from_slice(&hash_bytes);
        let data = BASE64
            .decode(&node.data)
            .map_err(|e| Error::Serialization(e.to_string()))?;
        let children = node
            .children
            .as_ref()
            .map(|c| {
                c.iter()
                    .map(|h| {
                        hex_decode(h).map(|b| H256::from_slice(&b)).map_err(|_| {
                            Error::InvalidHash {
                                expected: h.clone(),
                                actual: "invalid hex".to_string(),
                            }
                        })
                    })
                    .collect::<Result<Vec<_>, _>>()
            })
            .transpose()?;
        to_store.push((hash, data, children));
    }
    to_store.reverse();
    for (hash, data, children) in to_store {
        let _ = state
            .storage
            .store_node(req.bucket_id, hash, data, children);
    }

    // Sign the receipt: blake2_256(bucket_id_le_bytes ++ committed_root_bytes).
    let root_bytes = hex_decode(&req.committed_root).map_err(|_| Error::InvalidHash {
        expected: req.committed_root.clone(),
        actual: "invalid hex".to_string(),
    })?;
    let mut payload = Vec::with_capacity(8 + 32);
    payload.extend_from_slice(&req.bucket_id.to_le_bytes());
    payload.extend_from_slice(&root_bytes);
    let payload_hash = blake2_256(&payload);
    let signature = state.sign(payload_hash.as_ref())?;

    Ok(Json(CustodyReceipt {
        provider_id: state.provider_id.clone(),
        signature,
    }))
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
        && bucket.leaf_count == request.leaf_count;

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

    let signature = state.sign(&encoded)?;

    Ok(Json(SignProposalResponse {
        signer: state.provider_id.clone(),
        signature,
        agreed: true,
        local_mmr_root: Some(local_mmr_root),
    }))
}

/// Trigger checkpoint submission for a bucket.
///
/// Sends a ForceCheckpoint command to the checkpoint coordinator,
/// which handles leader election, signature collection, and on-chain submission.
async fn trigger_checkpoint(
    State(state): State<Arc<ProviderState>>,
    Query(query): Query<CheckpointDutyQuery>,
) -> Result<Json<TriggerCheckpointResponse>, Error> {
    tracing::info!(
        "Checkpoint trigger requested for bucket {}",
        query.bucket_id
    );

    // Verify bucket has data locally
    let bucket = state.storage.get_bucket(query.bucket_id);
    if bucket.is_none() {
        return Err(Error::Internal(format!(
            "Bucket {} not found in local storage. Upload data first.",
            query.bucket_id
        )));
    }
    let bucket = bucket.unwrap();
    if bucket.leaf_count == 0 {
        return Err(Error::Internal(format!(
            "Bucket {} has no committed data (leaf_count=0). Upload and commit data first.",
            query.bucket_id
        )));
    }

    let sender = state
        .checkpoint_cmd_tx
        .lock()
        .map_err(|_| Error::Internal("Lock poisoned".to_string()))?
        .clone();

    let sender = sender.ok_or_else(|| {
        Error::Internal(
            "Checkpoint coordinator not running. Start provider with --enable-checkpoint-coordinator"
                .to_string(),
        )
    })?;

    sender
        .send(crate::checkpoint_coordinator::CoordinatorCommand::ForceCheckpoint(query.bucket_id))
        .await
        .map_err(|_| Error::Internal(
            "Coordinator channel closed — the coordinator task may have crashed. Check provider logs and restart.".to_string(),
        ))?;

    tracing::info!(
        "ForceCheckpoint command sent for bucket {} (leaves={}, mmr_root=0x{})",
        query.bucket_id,
        bucket.leaf_count,
        hex_encode(&bucket.mmr_root.as_bytes()[..4])
    );

    Ok(Json(TriggerCheckpointResponse {
        bucket_id: query.bucket_id,
        triggered: true,
        message: format!(
            "Checkpoint triggered for bucket {} with {} leaves. The coordinator will handle submission.",
            query.bucket_id, bucket.leaf_count
        ),
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
    let ready = bucket.leaf_count > 0;

    Ok(Json(CheckpointDutyResponse {
        bucket_id: query.bucket_id,
        mmr_root: format!("0x{}", hex_encode(bucket.mmr_root.as_bytes())),
        start_seq: bucket.start_seq,
        leaf_count: bucket.leaf_count,
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

/// Sign [`AgreementTerms`] for a bucket owner, echoing the request but pinning
/// `price_per_byte` to the provider's own listed price (the client may have
/// proposed more).
///
/// TODO: requests are accepted automatically; let providers vet them later.
///
/// Returns one of several `503`s when a prerequisite is missing:
/// - `signing_unavailable` — no `--keyfile`.
/// - `chain_state_not_ready` — `current_block` and `request_timeout` are not both
///   known from the chain yet.
/// - `provider_info_unavailable` — provider not registered on chain yet; the
///   background reconciler clears this automatically once registration lands, no
///   restart needed.
/// - `nonce_counter_unavailable` — counter not yet aligned with the chain's replay
///   window.
/// - `provider_deregistering` — provider has announced deregistration and no longer
///   signs new terms.
async fn negotiate_terms(
    State(state): State<Arc<ProviderState>>,
    Json(req): Json<NegotiateRequest>,
) -> Result<Json<SignedTerms>, Error> {
    let keypair = state.keypair.as_ref().ok_or(Error::SigningUnavailable)?;

    // Both current_block and RequestTimeout must be known before we can sign —
    // otherwise we'd emit unbounded or already-expired terms.
    let current_block = state
        .chain_state
        .current_block
        .load(std::sync::atomic::Ordering::Relaxed);
    let request_timeout = state
        .chain_state
        .constants
        .read()
        .as_ref()
        .map(|c| c.request_timeout)
        .unwrap_or(0);
    if current_block == 0 || request_timeout == 0 {
        return Err(Error::ChainStateNotReady);
    }

    // Validate against the provider's on-chain settings. `None` means not
    // registered yet; the coordinator populates it once registration lands.
    let info = state
        .chain_state
        .provider_info
        .read()
        .clone()
        .ok_or(Error::ProviderInfoUnavailable)?;

    // A provider that has announced deregistration is winding down and must not
    // sign new terms — the on-chain pallet rejects them too once deregistering.
    if info.deregister_at.is_some() {
        return Err(Error::ProviderDeregistering);
    }

    negotiate::validate_request(&req, &info)?;

    // The coordinator bootstraps the nonce counter before publishing
    // `provider_info`, so a loaded `info` implies a ready counter. Guard
    // anyway so we never sign a nonce not derived from on-chain replay state.
    let nonce_counter = state
        .chain_state
        .nonce_counter
        .read()
        .clone()
        .ok_or(Error::NonceCounterUnavailable)?;

    let terms: AgreementTermsOf = AgreementTerms {
        owner: req.owner,
        max_bytes: req.max_bytes,
        duration: req.duration,
        price_per_byte: info.price_per_byte,
        valid_until: current_block.saturating_add(request_timeout),
        nonce: nonce_counter.next(),
        bucket_id: req.bucket_id,
        replica_params: req.replica_params.map(Into::into),
    };
    let signature = storage_client::sign_terms(keypair, &terms);

    Ok(Json(SignedTerms { terms, signature }))
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
        local_leaf_count: bucket.leaf_count,
        last_sync_block: None, // Would be tracked by coordinator
        syncing: false,        // Would check coordinator state
    }))
}
