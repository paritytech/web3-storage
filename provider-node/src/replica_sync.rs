// SPDX-License-Identifier: GPL-3.0-only

//! Replica synchronization protocol.
//!
//! Replicas autonomously sync data from other providers using:
//! 1. MMR diff detection (compare peaks)
//! 2. Top-down chunk fetching
//! 3. On-chain sync confirmation
//!
//! A source can be a primary or another replica: primaries gate reads on
//! private buckets (a replica is not a bucket member, so an honest primary
//! refuses it — the design's "primary gate"), while replicas serve everyone,
//! so a private bucket's replicas seed further replicas.

use crate::error::Error;
use crate::StorageBackend;
use base64::Engine;
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

    /// Sync a bucket from another provider (a primary, or another replica).
    ///
    /// This implements the top-down sync algorithm:
    /// 1. Fetch MMR peaks from the source
    /// 2. Compare with local state
    /// 3. Fetch missing MMR subtrees
    /// 4. Fetch missing chunks
    ///
    /// Returns the synced MMR root. A 401 from a source (a private bucket's
    /// primary) is just an error here — the caller tries the next source.
    pub async fn sync_from_source(
        &self,
        bucket_id: BucketId,
        source_url: &str,
    ) -> Result<H256, Error> {
        // Get the source's current MMR state
        let response = self
            .http
            .get(format!("{source_url}/mmr_peaks"))
            .query(&[("bucket_id", bucket_id.to_string())])
            .send()
            .await
            .map_err(|e| Error::Storage(format!("Failed to fetch peaks: {e}")))?;

        if !response.status().is_success() {
            return Err(Error::Storage(format!(
                "Source returned error: {}",
                response.status()
            )));
        }

        let peaks_response: MmrPeaksResponse = response
            .json()
            .await
            .map_err(|e| Error::Serialization(e.to_string()))?;

        // Initialize bucket if needed
        self.storage
            .init_bucket(bucket_id, u64::MAX)
            .map_err(|e| Error::Storage(format!("Failed to init bucket: {e}")))?;

        // Get local state
        let local_bucket = self.storage.get_bucket(bucket_id);

        // Determine what we need to fetch
        let target_root = hex_decode(&peaks_response.mmr_root)
            .map_err(|_| Error::Storage("Invalid mmr_root format".to_string()))?;
        let target_root = H256::from_slice(&target_root);

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
        // For now, we'll fetch all hashes from the source

        let peaks: Vec<H256> = peaks_response
            .peaks
            .iter()
            .map(|h| {
                let bytes =
                    hex_decode(h).map_err(|_| Error::Storage("Invalid peak hash".to_string()))?;
                Ok(H256::from_slice(&bytes))
            })
            .collect::<Result<Vec<_>, Error>>()?;

        // Fetch nodes for each peak
        for peak in peaks {
            self.fetch_subtree(bucket_id, peak, source_url).await?;
        }

        Ok(target_root)
    }

    /// Recursively fetch a subtree from a source provider.
    fn fetch_subtree<'a>(
        &'a self,
        bucket_id: BucketId,
        root_hash: H256,
        source_url: &'a str,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = Result<(), Error>> + Send + 'a>> {
        Box::pin(async move {
            // Check if we already have this node
            if self.storage.get_node(&root_hash).is_some() {
                return Ok(());
            }

            // Fetch the node from the source
            let response = self
                .http
                .get(format!("{source_url}/node"))
                .query(&[("hash", format!("0x{}", hex::encode(root_hash.as_bytes())))])
                .send()
                .await
                .map_err(|e| Error::Storage(format!("Failed to fetch node: {e}")))?;

            if !response.status().is_success() {
                return Err(Error::Storage(format!(
                    "Source returned error for node: {}",
                    response.status()
                )));
            }

            let node_response: DownloadNodeResponse = response
                .json()
                .await
                .map_err(|e| Error::Serialization(e.to_string()))?;

            // Decode node data
            let data = base64::engine::general_purpose::STANDARD
                .decode(&node_response.data)
                .map_err(|e| Error::Serialization(e.to_string()))?;

            // Decode children if present
            let children = node_response
                .children
                .map(|c| {
                    c.iter()
                        .map(|h| {
                            let bytes = hex_decode(h)?;
                            Ok(H256::from_slice(&bytes))
                        })
                        .collect::<Result<Vec<_>, Error>>()
                })
                .transpose()?;

            // Store locally
            self.storage
                .store_node(bucket_id, root_hash, data, children.clone())?;

            // Recursively fetch children
            if let Some(child_hashes) = children {
                for child in child_hashes {
                    self.fetch_subtree(bucket_id, child, source_url).await?;
                }
            }

            Ok(())
        })
    }

    /// Continuous sync loop for a replica.
    ///
    /// This would run in a background task and periodically:
    /// 1. Check for new data from the sources
    /// 2. Sync new data
    /// 3. Confirm sync on-chain (if enough time has passed since last sync)
    pub async fn sync_loop(
        &self,
        bucket_id: BucketId,
        source_urls: Vec<String>,
        _min_sync_interval_blocks: u32,
    ) -> Result<(), Error> {
        loop {
            // Try syncing from each source
            for source_url in &source_urls {
                match self.sync_from_source(bucket_id, source_url).await {
                    Ok(new_root) => {
                        tracing::info!(
                            "Successfully synced bucket {} from {}: root = 0x{}",
                            bucket_id,
                            source_url,
                            hex::encode(new_root.as_bytes())
                        );

                        // In a full implementation, we would:
                        // 1. Check time since last on-chain sync
                        // 2. If min_sync_interval has passed, call confirm_replica_sync extrinsic
                        // 3. This would deduct from sync_balance and pay the replica

                        break;
                    }
                    Err(e) => {
                        tracing::warn!(
                            "Failed to sync bucket {} from {}: {}",
                            bucket_id,
                            source_url,
                            e
                        );
                        continue;
                    }
                }
            }

            // Wait before next sync attempt
            tokio::time::sleep(tokio::time::Duration::from_secs(60)).await;
        }
    }
}

// Helper types matching the API responses

#[derive(serde::Deserialize)]
#[allow(dead_code)]
struct MmrPeaksResponse {
    bucket_id: u64,
    mmr_root: String,
    peaks: Vec<String>,
}

#[derive(serde::Deserialize)]
#[allow(dead_code)]
struct DownloadNodeResponse {
    hash: String,
    data: String,
    children: Option<Vec<String>>,
}

/// Decode hex string (with or without 0x prefix).
fn hex_decode(s: &str) -> Result<Vec<u8>, Error> {
    let s = s.strip_prefix("0x").unwrap_or(s);
    hex::decode(s).map_err(|e| Error::Serialization(format!("Invalid hex: {e}")))
}
