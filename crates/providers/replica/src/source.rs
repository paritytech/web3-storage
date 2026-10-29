// SPDX-License-Identifier: Apache-2.0

//! A replica's view of its sync sources: an HTTP client for the paged
//! `/mmr_leaves` and `/fetch_nodes` endpoints that checks every response
//! against the paging rules, and the list of sources one sync pass may use.

use crate::Error;
use base64::Engine;
use provider_storage::{ChunkTreeNode, LeafPage};
use serde::{Deserialize, Serialize};
use sp_core::H256;
use std::collections::HashSet;
use std::time::Duration;
use storage_primitives::{mmr::MmrLeaf, BucketId};

/// The largest response body read from a source. A provider node's page holds
/// at most 16 chunks of `DEFAULT_CHUNK_SIZE` (`PAGE_MAX_BYTES` in the
/// node's `api` module), about 5.3 MiB as base64, so this leaves room for other
/// page limits without letting a source fill the replica's memory.
const MAX_RESPONSE_BYTES: usize = 16 * 1024 * 1024;

/// A page of nodes from `/fetch_nodes`. Every requested hash is in exactly one
/// of the three lists.
#[derive(Debug, Clone, PartialEq)]
pub struct NodePage {
    /// Nodes whose content hashes to their hash, stored under that hash.
    pub nodes: Vec<(H256, ChunkTreeNode)>,
    /// Hashes the source does not store.
    pub missing: Vec<H256>,
    /// Hashes the source did not process because of its page limits.
    pub next: Vec<H256>,
}

#[derive(Deserialize)]
struct LeavesWire {
    bucket_id: BucketId,
    from_seq: u64,
    next_seq: u64,
    leaves: Vec<MmrLeaf>,
}

#[derive(Serialize)]
struct FetchNodesRequest<'a> {
    bucket_id: BucketId,
    hashes: &'a [H256],
}

#[derive(Deserialize)]
struct NodesWire {
    nodes: Vec<NodeWire>,
    missing: Vec<H256>,
    next: Vec<H256>,
}

#[derive(Deserialize)]
struct NodeWire {
    hash: H256,
    /// Base64, for a chunk.
    data: Option<String>,
    /// The two child hashes, for an internal node.
    children: Option<Vec<H256>>,
}

/// HTTP client for one sync source's paged endpoints.
#[derive(Clone)]
pub struct SourceClient {
    http: reqwest::Client,
}

impl SourceClient {
    /// `request_timeout` bounds each request, so a source that stops answering
    /// costs one timeout and not the whole sync.
    pub fn new(request_timeout: Duration) -> Self {
        let http = reqwest::Client::builder()
            .timeout(request_timeout)
            .build()
            .expect("the client configuration is valid");
        Self { http }
    }

    /// Fetch one page of the leaves with sequence numbers in
    /// `[from_seq, to_seq)` from `source`.
    ///
    /// Fails with `BadSource` if the page does not start at `from_seq`, reaches
    /// past `to_seq`, does not hold as many leaves as it claims, or makes no
    /// progress for a non-empty range.
    pub async fn mmr_leaves(
        &self,
        source: &str,
        bucket_id: BucketId,
        from_seq: u64,
        to_seq: u64,
    ) -> Result<LeafPage, Error> {
        let response = self
            .http
            .get(format!(
                "{source}/mmr_leaves?bucket_id={bucket_id}&from_seq={from_seq}&to_seq={to_seq}"
            ))
            .send()
            .await
            .map_err(|e| Error::source_request("mmr leaves", e))?;
        let wire: LeavesWire = decode_body("mmr leaves", response).await?;
        check_leaf_page(wire, bucket_id, from_seq, to_seq)
    }

    /// Fetch the nodes with the given hashes from `source`. The source may
    /// answer only part of them: see [`NodePage`].
    ///
    /// Fails with `NodeHashMismatch` if a node does not hash to the hash it is
    /// listed under, and with `BadSource` if the lists break the paging rules.
    pub async fn fetch_nodes(
        &self,
        source: &str,
        bucket_id: BucketId,
        hashes: &[H256],
    ) -> Result<NodePage, Error> {
        let response = self
            .http
            .post(format!("{source}/fetch_nodes"))
            .json(&FetchNodesRequest { bucket_id, hashes })
            .send()
            .await
            .map_err(|e| Error::source_request("fetch nodes", e))?;
        let wire: NodesWire = decode_body("fetch nodes", response).await?;
        check_node_page(wire, hashes)
    }
}

/// The body of a success response, or the error for any other status. A body
/// above [`MAX_RESPONSE_BYTES`] is a `BadSource` error and is not read to its
/// end.
async fn decode_body<T: serde::de::DeserializeOwned>(
    what: &'static str,
    mut response: reqwest::Response,
) -> Result<T, Error> {
    let status = response.status();
    if !status.is_success() {
        return Err(Error::SourceUnavailable {
            what,
            status: status.as_u16(),
        });
    }
    let mut body = Vec::new();
    while let Some(chunk) = response
        .chunk()
        .await
        .map_err(|e| Error::source_request(what, e))?
    {
        if body.len() + chunk.len() > MAX_RESPONSE_BYTES {
            return Err(Error::bad_source(
                what,
                format!("response is larger than {MAX_RESPONSE_BYTES} bytes"),
            ));
        }
        body.extend_from_slice(&chunk);
    }
    serde_json::from_slice(&body).map_err(|e| Error::decode(what, e))
}

fn check_leaf_page(
    wire: LeavesWire,
    bucket_id: BucketId,
    from_seq: u64,
    to_seq: u64,
) -> Result<LeafPage, Error> {
    let bad = |reason: String| Error::bad_source("mmr leaves", reason);

    if wire.bucket_id != bucket_id {
        return Err(bad(format!("page is for bucket {}", wire.bucket_id)));
    }
    if wire.from_seq != from_seq {
        return Err(bad(format!(
            "page starts at {}, requested {from_seq}",
            wire.from_seq
        )));
    }
    if wire.next_seq < from_seq || wire.next_seq > to_seq {
        return Err(bad(format!(
            "next_seq {} is outside {from_seq}..={to_seq}",
            wire.next_seq
        )));
    }
    if wire.leaves.len() as u64 != wire.next_seq - from_seq {
        return Err(bad(format!(
            "page has {} leaves for {from_seq}..{}",
            wire.leaves.len(),
            wire.next_seq
        )));
    }
    if from_seq < to_seq && wire.next_seq == from_seq {
        return Err(bad("page made no progress".to_string()));
    }
    Ok(LeafPage {
        leaves: wire.leaves,
        next_seq: wire.next_seq,
    })
}

fn check_node_page(wire: NodesWire, requested: &[H256]) -> Result<NodePage, Error> {
    let bad = |reason: String| Error::bad_source("fetch nodes", reason);

    let requested: HashSet<H256> = requested.iter().copied().collect();
    let mut seen = HashSet::new();
    let mut listed = |hash: H256, list: &str| {
        if !requested.contains(&hash) {
            return Err(bad(format!(
                "{list} lists {hash:#x}, which was not requested"
            )));
        }
        if !seen.insert(hash) {
            return Err(bad(format!("{hash:#x} is listed more than once")));
        }
        Ok(())
    };

    let mut nodes = Vec::with_capacity(wire.nodes.len());
    for node in wire.nodes {
        listed(node.hash, "nodes")?;
        let data = node
            .data
            .map(|data| base64::engine::general_purpose::STANDARD.decode(data))
            .transpose()
            .map_err(|e| Error::decode("node data", e))?;
        let parsed = ChunkTreeNode::from_wire(data, node.children).map_err(Error::Backend)?;
        let actual = parsed.hash();
        if actual != node.hash {
            return Err(Error::NodeHashMismatch {
                expected: node.hash,
                actual,
            });
        }
        nodes.push((node.hash, parsed));
    }
    for hash in &wire.missing {
        listed(*hash, "missing")?;
    }
    for hash in &wire.next {
        listed(*hash, "next")?;
    }

    if seen.len() != requested.len() {
        return Err(bad(format!(
            "{} of {} requested hashes are not listed",
            requested.len() - seen.len(),
            requested.len()
        )));
    }
    if nodes.is_empty() && wire.missing.is_empty() && !requested.is_empty() {
        return Err(bad("page made no progress".to_string()));
    }
    Ok(NodePage {
        nodes,
        missing: wire.missing,
        next: wire.next,
    })
}

/// The sources one sync pass may use, in the order to try them: primaries
/// first, then the bucket's other replicas.
///
/// A source that fails is dropped for the rest of the pass. A pass is short,
/// so a failed source stays out instead of being retried after a delay; the
/// next pass starts with every source again.
#[derive(Debug, Clone)]
pub struct SourceSet {
    urls: Vec<String>,
}

impl SourceSet {
    pub fn new(urls: Vec<String>) -> Self {
        Self { urls }
    }

    /// The first source that has not been dropped.
    pub fn current(&self) -> Option<&str> {
        self.urls.first().map(String::as_str)
    }

    /// The sources that have not been dropped, in the order to try them.
    pub fn iter(&self) -> impl Iterator<Item = &str> {
        self.urls.iter().map(String::as_str)
    }

    /// Stop using `url` for the rest of the pass.
    pub fn drop_source(&mut self, url: &str) {
        self.urls.retain(|existing| existing != url);
    }
}
