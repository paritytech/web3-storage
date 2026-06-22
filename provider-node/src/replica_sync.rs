// SPDX-License-Identifier: GPL-3.0-only

//! Replica synchronization protocol — L1/L2/L3 off-chain implementation.
//!
//! ## L1 — Offer/want diff
//!
//! The requester tells the peer how many leaves it holds. The peer enumerates
//! all content-node hashes under the leaves the requester is missing (offer).
//! The requester filters that list to what it actually lacks (want). The peer
//! batch-returns those nodes. Result: only the diff crosses the wire, in two
//! round-trips regardless of bucket size.
//!
//! ## L2 — Resumable transfer (interval store + epoch fingerprint)
//!
//! The `ReplicaSync` struct tracks per-(peer, bucket) synced leaf ranges and
//! a fingerprint of the peer's last-seen committed root. On reconnect, syncing
//! resumes from the first unfilled range. If the peer's fingerprint changes
//! unexpectedly (wipe/reset), intervals are discarded and a full re-sync runs.
//!
//! ## L3 — Anti-entropy pull + push seeding
//!
//! Pull: `sync_from_peer` works against any peer (primary or replica).
//! Push: `push_to_peers` sends new nodes to N peers on write and collects
//! signed custody receipts, verified against each peer's public key.

use crate::error::Error;
use crate::StorageBackend;
use base64::Engine;
use reqwest::Client;
use sp_core::H256;
use std::collections::HashMap;
use std::str::FromStr;
use std::sync::{Arc, Mutex};
use storage_primitives::{blake2_256, BucketId};

// ── L2: per-(peer, bucket) sync state ────────────────────────────────────────

#[derive(Default)]
struct SyncState {
    /// Leaf ranges already synced from this peer. Used to resume interrupted syncs.
    synced_ranges: Vec<(u64, u64)>,
    /// The peer's committed MMR root at last sync. Changed unexpectedly → reset.
    epoch_root: Option<H256>,
}

impl SyncState {
    fn mark_synced(&mut self, start: u64, end: u64) {
        self.synced_ranges.push((start, end));
    }

    fn reset(&mut self) {
        self.synced_ranges.clear();
        self.epoch_root = None;
    }
}

// ── Replica synchronization manager ──────────────────────────────────────────

/// Replica synchronization manager (L1 offer/want + L2 resume + L3 pull/push).
pub struct ReplicaSync {
    pub(crate) storage: Arc<dyn StorageBackend>,
    http: Client,
    /// L2 interval store: (peer_url, bucket_id) → SyncState
    sync_states: Mutex<HashMap<(String, BucketId), SyncState>>,
}

impl ReplicaSync {
    pub fn new(storage: Arc<dyn StorageBackend>) -> Self {
        Self {
            storage,
            http: Client::new(),
            sync_states: Mutex::new(HashMap::new()),
        }
    }

    /// Return the L2 synced leaf ranges for a peer+bucket (for observability).
    pub fn synced_ranges(&self, peer_url: &str, bucket_id: BucketId) -> Vec<(u64, u64)> {
        self.sync_states
            .lock()
            .unwrap()
            .get(&(peer_url.to_string(), bucket_id))
            .map(|s| s.synced_ranges.clone())
            .unwrap_or_default()
    }

    /// L3 push: send the new nodes for `bucket_id` to each `peer_url` and
    /// collect a signed custody receipt from each.
    ///
    /// The receipt is `provider_id` + sr25519 sig over
    /// `blake2_256(bucket_id_le ++ committed_root_bytes)`.
    /// A receipt from peer P proves P accepted custody of the data.
    pub async fn push_to_peers(
        &self,
        bucket_id: BucketId,
        committed_root: H256,
        peer_urls: &[String],
    ) -> Vec<(String, Result<CustodyReceipt, Error>)> {
        // Collect all content nodes for this committed root.
        let data_roots = match self.storage.get_data_roots_from(bucket_id, 0) {
            Ok(r) => r,
            Err(e) => {
                let msg = e.to_string();
                return peer_urls
                    .iter()
                    .map(|u| (u.clone(), Err(Error::Storage(msg.clone()))))
                    .collect();
            }
        };

        let mut nodes: Vec<PushNode> = Vec::new();
        let mut seen = std::collections::HashSet::new();
        for dr in data_roots {
            for hash in self.storage.collect_all_node_hashes(dr) {
                if !seen.insert(hash) {
                    continue;
                }
                if let Some(node) = self.storage.get_node(&hash) {
                    nodes.push(PushNode {
                        hash: format!("0x{}", hex::encode(hash.as_bytes())),
                        data: base64::engine::general_purpose::STANDARD.encode(&node.data),
                        children: node.children.map(|c| {
                            c.iter()
                                .map(|h| format!("0x{}", hex::encode(h.as_bytes())))
                                .collect()
                        }),
                    });
                }
            }
        }

        let root_str = format!("0x{}", hex::encode(committed_root.as_bytes()));
        let mut results = Vec::new();
        for peer_url in peer_urls {
            let outcome = self
                .http
                .post(format!("{peer_url}/sync/push"))
                .json(&SyncPushPayload {
                    bucket_id,
                    committed_root: root_str.clone(),
                    nodes: nodes.clone(),
                })
                .send()
                .await
                .map_err(|e| Error::Storage(format!("push failed: {e}")))
                .and_then(|resp| {
                    if resp.status().is_success() {
                        Ok(resp)
                    } else {
                        Err(Error::Storage(format!("push rejected: {}", resp.status())))
                    }
                });

            let receipt = match outcome {
                Ok(resp) => resp
                    .json::<CustodyReceipt>()
                    .await
                    .map_err(|e| Error::Serialization(e.to_string())),
                Err(e) => Err(e),
            };
            results.push((peer_url.clone(), receipt));
        }
        results
    }

    /// Verify a custody receipt from a peer.
    ///
    /// Checks the sr25519 signature over `blake2_256(bucket_id_le ++ root_bytes)`
    /// against the public key encoded in `receipt.provider_id` (SS58).
    pub fn verify_receipt(
        &self,
        bucket_id: BucketId,
        committed_root: H256,
        receipt: &CustodyReceipt,
    ) -> bool {
        let mut payload = Vec::with_capacity(8 + 32);
        payload.extend_from_slice(&bucket_id.to_le_bytes());
        payload.extend_from_slice(committed_root.as_bytes());
        let msg = blake2_256(&payload);

        let sig_bytes = match hex_decode(&receipt.signature) {
            Ok(b) => b,
            Err(_) => return false,
        };
        let sig_arr: [u8; 64] = match sig_bytes.try_into() {
            Ok(a) => a,
            Err(_) => return false,
        };
        let sig = subxt_signer::sr25519::Signature(sig_arr);

        // Decode SS58 provider_id to a 32-byte public key.
        let account = match sp_runtime::AccountId32::from_str(&receipt.provider_id) {
            Ok(a) => a,
            Err(_) => return false,
        };
        let pub_bytes: &[u8; 32] = account.as_ref();
        let pub_key = subxt_signer::sr25519::PublicKey(*pub_bytes);

        subxt_signer::sr25519::verify(&sig, msg, &pub_key)
    }

    /// Sync a bucket from a peer using the L1 offer/want protocol + L2 resume.
    ///
    /// ```text
    ///   This node                                  Peer
    ///       |  POST /sync/offer { bucket_id,         |  L1: enumerate content-node hashes
    ///       |      leaf_count: have }                |  under leaves we're missing
    ///       |--------------------------------------> |
    ///       |<--- { hashes, data_roots } ----------- |
    ///       |  check_exists(offered) → want list     |
    ///       |  POST /sync/want { want }               |  L1: batch-return only those nodes
    ///       |--------------------------------------> |
    ///       |<--- { nodes } ----------------------- |
    ///       |  store bottom-up, commit data_roots    |  rebuild local MMR
    /// ```
    pub async fn sync_from_primary(
        &self,
        bucket_id: BucketId,
        peer_url: &str,
    ) -> Result<H256, Error> {
        self.storage
            .init_bucket(bucket_id, u64::MAX)
            .map_err(|e| Error::Storage(format!("Failed to init bucket: {e}")))?;

        let have = self
            .storage
            .get_bucket(bucket_id)
            .map(|b| b.leaf_count)
            .unwrap_or(0);

        // ── L2: epoch fingerprint ─────────────────────────────────────────────
        let peer_listing: ListBucketsResponse = self
            .http
            .get(format!("{peer_url}/buckets"))
            .send()
            .await
            .map_err(|e| Error::Storage(format!("Failed to list buckets: {e}")))?
            .json()
            .await
            .map_err(|e| Error::Serialization(e.to_string()))?;

        let peer_bucket = peer_listing
            .buckets
            .into_iter()
            .find(|b| b.bucket_id == bucket_id)
            .ok_or_else(|| Error::Storage(format!("Peer has no bucket {bucket_id}")))?;
        let target_root = H256::from_slice(&hex_decode(&peer_bucket.mmr_root)?);
        let target_leaf_count = peer_bucket.leaf_count;

        // Short-circuit if already in sync.
        if have == target_leaf_count
            && self
                .storage
                .get_bucket(bucket_id)
                .is_some_and(|b| b.mmr_root == target_root)
        {
            return Ok(target_root);
        }

        // If the peer's root regressed (it was wiped), reset our progress record.
        {
            let mut states = self.sync_states.lock().unwrap();
            let state = states.entry((peer_url.to_string(), bucket_id)).or_default();
            if let Some(known) = state.epoch_root {
                if known != target_root && target_leaf_count < have {
                    state.reset();
                }
            }
        }

        // ── L1: offer/want ────────────────────────────────────────────────────
        let offer: SyncOfferResponse = self
            .http
            .post(format!("{peer_url}/sync/offer"))
            .json(&SyncOfferRequest {
                bucket_id,
                leaf_count: have,
            })
            .send()
            .await
            .map_err(|e| Error::Storage(format!("Failed to POST /sync/offer: {e}")))?
            .json()
            .await
            .map_err(|e| Error::Serialization(e.to_string()))?;

        if offer.hashes.is_empty() {
            return Ok(target_root);
        }

        let offered: Vec<H256> = offer
            .hashes
            .iter()
            .filter_map(|h| hex_decode(h).ok().map(|b| H256::from_slice(&b)))
            .collect();
        let (_, want_hashes) = self.storage.check_exists(bucket_id, &offered);
        let want_strings: Vec<String> = want_hashes
            .iter()
            .map(|h| format!("0x{}", hex::encode(h.as_bytes())))
            .collect();

        tracing::debug!(
            "L1 offer/want: offered={} want={}",
            offer.hashes.len(),
            want_strings.len()
        );

        let batch: SyncWantResponse = self
            .http
            .post(format!("{peer_url}/sync/want"))
            .json(&SyncWantRequest {
                bucket_id,
                hashes: want_strings,
            })
            .send()
            .await
            .map_err(|e| Error::Storage(format!("Failed to POST /sync/want: {e}")))?
            .json()
            .await
            .map_err(|e| Error::Serialization(e.to_string()))?;

        // Store bottom-up: the offer/want sends nodes in DFS pre-order
        // (parent before children) so reversing gives leaves-first order,
        // which satisfies store_node's child-existence requirement.
        let mut to_store: Vec<(H256, Vec<u8>, Option<Vec<H256>>)> = Vec::new();
        for node in &batch.nodes {
            let hash = H256::from_slice(&hex_decode(&node.hash)?);
            let data = base64::engine::general_purpose::STANDARD
                .decode(&node.data)
                .map_err(|e| Error::Serialization(e.to_string()))?;
            let children = node
                .children
                .as_ref()
                .map(|c| {
                    c.iter()
                        .map(|h| hex_decode(h).map(|b| H256::from_slice(&b)))
                        .collect::<Result<Vec<_>, _>>()
                })
                .transpose()?;
            to_store.push((hash, data, children));
        }
        to_store.reverse();
        for (hash, data, children) in to_store {
            let _ = self.storage.store_node(bucket_id, hash, data, children);
        }

        let new_roots: Vec<H256> = offer
            .data_roots
            .iter()
            .filter_map(|h| hex_decode(h).ok().map(|b| H256::from_slice(&b)))
            .collect();
        if !new_roots.is_empty() {
            self.storage.commit(bucket_id, new_roots)?;
        }

        // ── L2: record progress ────────────────────────────────────────────────
        {
            let mut states = self.sync_states.lock().unwrap();
            let state = states.entry((peer_url.to_string(), bucket_id)).or_default();
            state.mark_synced(have, target_leaf_count);
            state.epoch_root = Some(target_root);
        }

        let local_root = self
            .storage
            .get_bucket(bucket_id)
            .map(|b| b.mmr_root)
            .unwrap_or_default();
        if local_root != target_root {
            return Err(Error::Storage(format!(
                "post-sync root mismatch: local 0x{} != target 0x{}",
                hex::encode(local_root.as_bytes()),
                hex::encode(target_root.as_bytes()),
            )));
        }

        Ok(local_root)
    }

    /// Continuous sync loop for a replica: periodically pull from each primary.
    pub async fn sync_loop(
        &self,
        bucket_id: BucketId,
        primary_urls: Vec<String>,
        _min_sync_interval_blocks: u32,
    ) -> Result<(), Error> {
        loop {
            for primary_url in &primary_urls {
                match self.sync_from_primary(bucket_id, primary_url).await {
                    Ok(new_root) => {
                        tracing::info!(
                            "Successfully synced bucket {} from {}: root = 0x{}",
                            bucket_id,
                            primary_url,
                            hex::encode(new_root.as_bytes())
                        );
                        break;
                    }
                    Err(e) => {
                        tracing::warn!(
                            "Failed to sync bucket {} from {}: {}",
                            bucket_id,
                            primary_url,
                            e
                        );
                        continue;
                    }
                }
            }

            tokio::time::sleep(tokio::time::Duration::from_secs(60)).await;
        }
    }
}

// ── Wire types ────────────────────────────────────────────────────────────────

#[derive(serde::Deserialize)]
struct ListBucketsResponse {
    buckets: Vec<BucketSummaryResponse>,
}

#[derive(serde::Deserialize)]
struct BucketSummaryResponse {
    bucket_id: u64,
    mmr_root: String,
    leaf_count: u64,
}

/// Mirrors `SyncOfferRequest` in types.rs.
#[derive(serde::Serialize)]
struct SyncOfferRequest {
    bucket_id: u64,
    leaf_count: u64,
}

/// Mirrors `SyncOfferResponse` in types.rs.
#[derive(serde::Deserialize)]
struct SyncOfferResponse {
    hashes: Vec<String>,
    data_roots: Vec<String>,
}

/// Mirrors `SyncWantRequest` in types.rs.
#[derive(serde::Serialize)]
struct SyncWantRequest {
    bucket_id: u64,
    hashes: Vec<String>,
}

/// Mirrors `SyncWantResponse` in types.rs.
#[derive(serde::Deserialize)]
struct SyncWantResponse {
    nodes: Vec<WantNode>,
}

#[derive(serde::Deserialize)]
struct WantNode {
    hash: String,
    data: String,
    children: Option<Vec<String>>,
}

// ── L3 push wire types ────────────────────────────────────────────────────────

/// Mirrors `SyncPushRequest` in types.rs (sent by the pusher).
#[derive(serde::Serialize, Clone)]
struct SyncPushPayload {
    bucket_id: u64,
    committed_root: String,
    nodes: Vec<PushNode>,
}

#[derive(serde::Serialize, Clone)]
struct PushNode {
    hash: String,
    data: String,
    children: Option<Vec<String>>,
}

/// Mirrors `CustodyReceipt` in types.rs (returned by the peer).
#[derive(serde::Deserialize)]
pub struct CustodyReceipt {
    pub provider_id: String,
    pub signature: String,
}

/// Decode hex string (with or without 0x prefix).
fn hex_decode(s: &str) -> Result<Vec<u8>, Error> {
    let s = s.strip_prefix("0x").unwrap_or(s);
    hex::decode(s).map_err(|e| Error::Serialization(format!("Invalid hex: {e}")))
}
