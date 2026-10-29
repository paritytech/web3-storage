// SPDX-License-Identifier: Apache-2.0

//! Shared harness for the crate's integration tests: a storage fixture and a
//! mock primary provider.
//!
//! Each test binary compiles this module separately and uses part of it, so
//! unused items here are expected.
#![allow(dead_code)]

use axum::extract::{Query, State};
use axum::http::{header, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Json, Router};
use base64::Engine;
use provider_storage::{build_merkle_tree, ChunkTreeNode, StorageBackend};
use sp_core::H256;
use std::collections::{HashMap, HashSet};
use std::sync::{Arc, Mutex};
use std::time::Duration;
use storage_primitives::{hash_leaf, mmr::MmrLeaf, Commitment, DEFAULT_CHUNK_SIZE};

/// One canned answer from the mock primary: a status and a body. Tests serve
/// bodies a real primary could not produce, so this is a raw string rather
/// than a typed response.
pub type Reply = (StatusCode, String);

/// A non-success answer with an empty body.
pub fn status(status: StatusCode) -> Reply {
    (status, String::new())
}

/// A 200 answer with this body, valid JSON or not.
pub fn body(body: impl Into<String>) -> Reply {
    (StatusCode::OK, body.into())
}

/// `0x`-prefixed hex, the form both provider endpoints use for hashes.
pub fn hex_hash(hash: H256) -> String {
    format!("{hash:#x}")
}

pub fn base64(data: &[u8]) -> String {
    base64::engine::general_purpose::STANDARD.encode(data)
}

/// A `/mmr_peaks` body naming `mmr_root` and `peaks` verbatim.
pub fn peaks_body(mmr_root: &str, peaks: &[String]) -> Reply {
    body(serde_json::json!({ "bucket_id": 1, "mmr_root": mmr_root, "peaks": peaks }).to_string())
}

/// A `/node` body carrying `data` and `children` verbatim. A `None` field is
/// left out, as a real provider leaves it out.
pub fn node_body(hash: &str, data: Option<&str>, children: Option<Vec<String>>) -> Reply {
    let mut node = serde_json::json!({ "hash": hash });
    if let Some(data) = data {
        node["data"] = data.into();
    }
    if let Some(children) = children {
        node["children"] = children.into();
    }
    body(node.to_string())
}

/// A `/node` body for a chunk.
pub fn chunk_body(hash: H256, data: &[u8]) -> Reply {
    node_body(&hex_hash(hash), Some(&base64(data)), None)
}

/// A `/node` body for an internal node.
pub fn internal_body(hash: H256, left: H256, right: H256) -> Reply {
    node_body(
        &hex_hash(hash),
        None,
        Some(vec![hex_hash(left), hex_hash(right)]),
    )
}

#[derive(serde::Deserialize)]
struct NodeQuery {
    #[serde(rename = "bucket_id")]
    _bucket_id: u64,
    hash: String,
}

fn render(reply: &Reply) -> Response {
    (
        reply.0,
        [(header::CONTENT_TYPE, "application/json")],
        reply.1.clone(),
    )
        .into_response()
}

/// Mock primary serving only `/mmr_peaks`.
pub async fn spawn_primary(peaks: Reply) -> String {
    spawn_primary_with_nodes(peaks, HashMap::new()).await
}

/// Mock primary: `/mmr_peaks` always answers `peaks`; `/node` answers the
/// entry keyed by the requested `0x`-prefixed hash, or 404 for a hash it was
/// not given - as a real primary does for a hash it never stored.
pub async fn spawn_primary_with_nodes(peaks: Reply, nodes: HashMap<String, Reply>) -> String {
    let nodes = Arc::new(nodes);
    let app = Router::new()
        .route(
            "/mmr_peaks",
            get(move || {
                let peaks = peaks.clone();
                async move { render(&peaks) }
            }),
        )
        .route(
            "/node",
            get(move |Query(query): Query<NodeQuery>| {
                let nodes = Arc::clone(&nodes);
                async move {
                    render(
                        nodes
                            .get(&query.hash)
                            .unwrap_or(&status(StatusCode::NOT_FOUND)),
                    )
                }
            }),
        );

    // `bind` has already called listen(2), so the port accepts connections
    // before the serve task is first polled: no readiness wait is needed.
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    format!("http://{addr}")
}

/// An address with nothing listening on it: bind a port, then release it.
pub async fn dead_address() -> String {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    drop(listener);
    format!("http://{addr}")
}

/// Serve `app` on a free port and return its base URL.
pub async fn spawn_app(app: Router) -> String {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    format!("http://{addr}")
}

/// Mock source that answers every request with `reply`.
pub async fn spawn_fixed(reply: Reply) -> String {
    spawn_app(Router::new().fallback(move || {
        let reply = reply.clone();
        async move { render(&reply) }
    }))
    .await
}

/// Mock source that waits `delay` before answering every request.
pub async fn spawn_slow(delay: Duration) -> String {
    spawn_app(Router::new().fallback(move || async move {
        tokio::time::sleep(delay).await;
        render(&body("{}"))
    }))
    .await
}

/// A `/mmr_leaves` body with the given paging fields and leaves, verbatim.
pub fn leaves_body(bucket_id: u64, from_seq: u64, next_seq: u64, leaves: &[MmrLeaf]) -> Reply {
    body(
        serde_json::json!({
            "bucket_id": bucket_id,
            "from_seq": from_seq,
            "next_seq": next_seq,
            "leaves": leaves,
        })
        .to_string(),
    )
}

/// What a [`spawn_leaf_source`] source has been asked for.
pub struct LeafSource {
    pub url: String,
    /// The `(from_seq, to_seq)` of every request, in order.
    pub requests: Arc<Mutex<Vec<(u64, u64)>>>,
}

impl LeafSource {
    pub fn requests(&self) -> Vec<(u64, u64)> {
        self.requests.lock().unwrap().clone()
    }
}

#[derive(serde::Deserialize)]
struct LeavesQuery {
    bucket_id: u64,
    from_seq: u64,
    to_seq: u64,
}

/// Mock source serving `/mmr_leaves` for `leaves`, which start at `start_seq`,
/// `page_size` leaves per page. A range outside its leaves is a 409, as on a
/// real node. After `max_pages` pages it answers 500 to every request.
pub async fn spawn_leaf_source(
    start_seq: u64,
    leaves: Vec<MmrLeaf>,
    page_size: usize,
    max_pages: usize,
) -> LeafSource {
    let requests = Arc::new(Mutex::new(Vec::new()));
    let log = Arc::clone(&requests);
    let end = start_seq + leaves.len() as u64;
    let app = Router::new().route(
        "/mmr_leaves",
        get(move |Query(query): Query<LeavesQuery>| {
            let leaves = leaves.clone();
            let log = Arc::clone(&log);
            async move {
                let mut log = log.lock().unwrap();
                let served = log.len();
                log.push((query.from_seq, query.to_seq));
                if served >= max_pages {
                    return render(&status(StatusCode::INTERNAL_SERVER_ERROR));
                }
                if query.from_seq < start_seq || query.to_seq > end {
                    return render(&status(StatusCode::CONFLICT));
                }
                let next = query.to_seq.min(query.from_seq + page_size as u64);
                let page =
                    &leaves[(query.from_seq - start_seq) as usize..(next - start_seq) as usize];
                render(&leaves_body(query.bucket_id, query.from_seq, next, page))
            }
        }),
    );
    LeafSource {
        url: spawn_app(app).await,
        requests,
    }
}

/// A bucket that committed one upload per entry of `chunk_counts`, in order.
/// Upload `i` has `chunk_counts[i]` chunks: full-size ones with data of their
/// own, then a short last one. Returns the bucket 1 backend and the commitment
/// of its state.
pub fn source_bucket(
    chunk_counts: &[usize],
) -> (Arc<dyn StorageBackend>, tempfile::TempDir, Commitment) {
    let (storage, dir) = provider_storage::temp_rocksdb();
    storage.init_bucket(1, u64::MAX).unwrap();
    for (upload, &count) in chunk_counts.iter().enumerate() {
        let mut leaves = Vec::new();
        for chunk in 0..count {
            let len = if chunk + 1 == count {
                1000
            } else {
                DEFAULT_CHUNK_SIZE as usize
            };
            let data = vec![(upload * 16 + chunk) as u8 + 1; len];
            let hash = hash_leaf(&data);
            storage
                .store_node(1, hash, ChunkTreeNode::Chunk(data))
                .unwrap();
            leaves.push(hash);
        }
        let root = build_merkle_tree(storage.as_ref(), 1, &leaves).unwrap();
        storage.commit(1, vec![root]).unwrap();
    }
    let commitment = commitment_of(storage.as_ref());
    (storage, dir, commitment)
}

/// The commitment of bucket 1's current state.
pub fn commitment_of(storage: &dyn StorageBackend) -> Commitment {
    let info = storage.get_bucket(1).unwrap();
    Commitment {
        mmr_root: info.mmr_root,
        start_seq: info.start_seq,
        leaf_count: info.leaf_count,
    }
}

/// How a [`spawn_node_source`] source misbehaves.
#[derive(Clone)]
pub struct NodeSourceConfig {
    /// Items one `/fetch_nodes` page processes before the rest are `next`.
    pub max_items: usize,
    /// Hashes the source answers as missing although it stores them.
    pub hidden: HashSet<H256>,
    /// Serve chunks whose data does not match their hash.
    pub tamper: bool,
    /// After this many `/fetch_nodes` requests, answer 500.
    pub max_node_requests: usize,
}

impl Default for NodeSourceConfig {
    fn default() -> Self {
        Self {
            max_items: usize::MAX,
            hidden: HashSet::new(),
            tamper: false,
            max_node_requests: usize::MAX,
        }
    }
}

/// A source serving `/mmr_leaves` and `/fetch_nodes` from a real backend.
pub struct NodeSource {
    pub url: String,
    /// The hashes of every `/fetch_nodes` request, in order.
    pub node_requests: Arc<Mutex<Vec<Vec<H256>>>>,
}

impl NodeSource {
    pub fn node_requests(&self) -> Vec<Vec<H256>> {
        self.node_requests.lock().unwrap().clone()
    }

    pub fn requested(&self) -> HashSet<H256> {
        self.node_requests().into_iter().flatten().collect()
    }
}

#[derive(serde::Deserialize)]
struct FetchNodesBody {
    hashes: Vec<H256>,
}

struct NodeSourceState {
    storage: Arc<dyn StorageBackend>,
    config: NodeSourceConfig,
    node_requests: Arc<Mutex<Vec<Vec<H256>>>>,
}

pub async fn spawn_node_source(
    storage: Arc<dyn StorageBackend>,
    config: NodeSourceConfig,
) -> NodeSource {
    let node_requests = Arc::new(Mutex::new(Vec::new()));
    let state = Arc::new(NodeSourceState {
        storage,
        config,
        node_requests: Arc::clone(&node_requests),
    });
    let app = Router::new()
        .route(
            "/mmr_leaves",
            get(
                |State(state): State<Arc<NodeSourceState>>, Query(query): Query<LeavesQuery>| async move {
                    match state.storage.get_leaves(
                        query.bucket_id,
                        query.from_seq,
                        query.to_seq,
                        1000,
                        usize::MAX,
                    ) {
                        Ok(page) => render(&leaves_body(
                            query.bucket_id,
                            query.from_seq,
                            page.next_seq,
                            &page.leaves,
                        )),
                        Err(_) => render(&status(StatusCode::CONFLICT)),
                    }
                },
            ),
        )
        .route(
            "/fetch_nodes",
            post(
                |State(state): State<Arc<NodeSourceState>>, Json(request): Json<FetchNodesBody>| async move {
                    let served = {
                        let mut log = state.node_requests.lock().unwrap();
                        log.push(request.hashes.clone());
                        log.len() - 1
                    };
                    if served >= state.config.max_node_requests {
                        return render(&status(StatusCode::INTERNAL_SERVER_ERROR));
                    }
                    let (mut nodes, mut missing, mut next) = (Vec::new(), Vec::new(), Vec::new());
                    for hash in request.hashes {
                        if nodes.len() + missing.len() >= state.config.max_items {
                            next.push(hash);
                            continue;
                        }
                        let stored = state.storage.get_node(&hash);
                        match stored.filter(|_| !state.config.hidden.contains(&hash)) {
                            None => missing.push(hash),
                            Some(ChunkTreeNode::Chunk(data)) => {
                                let data = if state.config.tamper { vec![0xFF] } else { data };
                                nodes.push(serde_json::json!({ "hash": hash, "data": base64(&data) }));
                            }
                            Some(ChunkTreeNode::Internal { left, right }) => {
                                nodes.push(serde_json::json!({ "hash": hash, "children": [left, right] }));
                            }
                        }
                    }
                    render(&body(
                        serde_json::json!({ "nodes": nodes, "missing": missing, "next": next })
                            .to_string(),
                    ))
                },
            ),
        )
        .with_state(state);
    NodeSource {
        url: spawn_app(app).await,
        node_requests,
    }
}
