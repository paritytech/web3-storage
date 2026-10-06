// SPDX-License-Identifier: Apache-2.0

//! Replica synchronization protocol.
//!
//! Replicas autonomously sync data from primary providers using:
//! 1. MMR diff detection (compare peaks)
//! 2. Top-down chunk fetching
//!
//! On-chain confirmation of what was synced is the coordinator's
//! (`coordinator::ReplicaSyncCoordinator::confirm_on_chain`).

use crate::Error;
use base64::Engine;
use provider_storage::StorageBackend;
use reqwest::Client;
use sp_core::H256;
use std::sync::Arc;
use storage_primitives::BucketId;

/// Replica synchronization manager.
pub struct ReplicaSync {
    storage: Arc<dyn StorageBackend>,
    http: Client,
}

impl ReplicaSync {
    pub fn new(storage: Arc<dyn StorageBackend>) -> Self {
        Self {
            storage,
            http: Client::new(),
        }
    }

    /// Sync a bucket from a primary provider.
    ///
    /// This implements the top-down sync algorithm:
    /// 1. Fetch MMR peaks from primary
    /// 2. Compare with local state
    /// 3. Fetch missing MMR subtrees
    /// 4. Fetch missing chunks
    ///
    /// Returns the synced MMR root.
    pub async fn sync_from_primary(
        &self,
        bucket_id: BucketId,
        primary_url: &str,
    ) -> Result<H256, Error> {
        // TODO: (we need to put node's RPC to separate crate,
        //               something like provider's versioned protocol)
        // Get primary's current MMR state
        let response = self
            .http
            .get(format!("{primary_url}/mmr_peaks"))
            .query(&[("bucket_id", bucket_id.to_string())])
            .send()
            .await
            .map_err(|e| Error::primary_request("mmr peaks", e))?;

        if !response.status().is_success() {
            return Err(Error::PrimaryUnavailable {
                what: "mmr peaks",
                status: response.status().as_u16(),
            });
        }

        let peaks_response: MmrPeaksResponse = response
            .json()
            .await
            .map_err(|e| Error::decode("mmr peaks response", e))?;

        // Initialize bucket if needed
        self.storage.init_bucket(bucket_id, u64::MAX)?;

        // Get local state
        let local_bucket = self.storage.get_bucket(bucket_id).ok().flatten();

        // Determine what we need to fetch
        let target_root = peaks_response.mmr_root;

        // If we already have this root, we're done
        if let Some(bucket) = local_bucket {
            if bucket.mmr_root == target_root {
                return Ok(target_root);
            }
        }

        // Fetch missing nodes
        // In a full implementation, we would:
        // 1. Walk the MMR tree from peaks down
        // 2. Identify missing subtrees
        // 3. Batch fetch missing nodes
        // 4. Verify each node against its hash
        //
        // For now, we'll fetch all hashes from the primary

        // Fetch nodes for each peak
        for peak in peaks_response.peaks {
            self.fetch_subtree(bucket_id, peak, primary_url).await?;
        }

        Ok(target_root)
    }

    /// Recursively fetch a subtree from a primary provider.
    fn fetch_subtree<'a>(
        &'a self,
        bucket_id: BucketId,
        root_hash: H256,
        primary_url: &'a str,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = Result<(), Error>> + Send + 'a>> {
        Box::pin(async move {
            // Check if we already have this node
            if matches!(self.storage.get_node(&root_hash), Ok(Some(_))) {
                return Ok(());
            }

            // Fetch the node from primary
            let response = self
                .http
                .get(format!("{primary_url}/node"))
                .query(&[("hash", format!("0x{}", hex::encode(root_hash.as_bytes())))])
                .send()
                .await
                .map_err(|e| Error::primary_request("node", e))?;

            if !response.status().is_success() {
                return Err(Error::PrimaryUnavailable {
                    what: "node",
                    status: response.status().as_u16(),
                });
            }

            let node_response: DownloadNodeResponse = response
                .json()
                .await
                .map_err(|e| Error::decode("node response", e))?;

            // Decode node data
            let data = base64::engine::general_purpose::STANDARD
                .decode(&node_response.data)
                .map_err(|e| Error::decode("node data", e))?;

            let children = node_response.children;

            // Store locally
            self.storage
                .store_node(bucket_id, root_hash, data, children.clone())?;

            // Recursively fetch children
            if let Some(child_hashes) = children {
                for child in child_hashes {
                    self.fetch_subtree(bucket_id, child, primary_url).await?;
                }
            }

            Ok(())
        })
    }
}

// Helper types matching the API responses

// The endpoints serve hashes as `0x`-prefixed hex, which is what `H256`'s
// serde impl reads, so deserialization rejects a malformed or wrong-length
// hash instead of leaving it to a later `H256::from_slice` panic.

#[derive(serde::Deserialize)]
#[allow(dead_code)]
struct MmrPeaksResponse {
    bucket_id: u64,
    mmr_root: H256,
    peaks: Vec<H256>,
}

#[derive(serde::Deserialize)]
#[allow(dead_code)]
struct DownloadNodeResponse {
    hash: H256,
    data: String,
    children: Option<Vec<H256>>,
}
