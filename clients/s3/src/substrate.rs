// SPDX-License-Identifier: Apache-2.0

//! Substrate/chain integration for S3 client.

use crate::{BucketInfo, Result, S3ClientError};
use s3_primitives::{ListObjectsParams, ListObjectsResponse, S3BucketId};
use sp_core::H256;
use sp_runtime::AccountId32;
use storage_client::convert;
use storage_client::Signer;
use storage_subxt::api;
use storage_subxt::api::s3_registry::events::S3BucketCreated;
use subxt::{OnlineClient, PolkadotConfig};
use tracing::{debug, info, warn};

/// Object metadata from chain storage.
#[derive(Clone, Debug)]
pub struct ChainObjectMetadata {
    pub cid: H256,
    pub size: u64,
    pub last_modified: u64,
    pub content_type: Vec<u8>,
    pub etag: Vec<u8>,
    pub user_metadata: Vec<MetadataEntry>,
}

/// Metadata entry from chain.
#[derive(Clone, Debug)]
pub struct MetadataEntry {
    pub key: Vec<u8>,
    pub value: Vec<u8>,
}

/// Client for interacting with the substrate chain.
#[derive(Clone)]
pub struct SubstrateClient {
    /// Subxt online client.
    client: OnlineClient<PolkadotConfig>,
    /// Signer for extrinsics and provider auth.
    signer: Signer,
    /// Account ID (32 bytes).
    account_id: [u8; 32],
    /// Endpoint URL.
    #[allow(dead_code)]
    endpoint: String,
}

impl SubstrateClient {
    /// Create a new substrate client.
    pub async fn new(chain_url: &str, signer: Signer) -> Result<Self> {
        info!("Connecting to chain at {}", chain_url);

        let client = OnlineClient::<PolkadotConfig>::from_url(chain_url)
            .await
            .map_err(|e| S3ClientError::ChainError(format!("Failed to connect to chain: {e}")))?;

        let account_id: [u8; 32] = signer.keypair().public_key().0;
        info!("Connected to chain, account: 0x{}", hex::encode(account_id));

        Ok(Self {
            client,
            signer,
            account_id,
            endpoint: chain_url.to_string(),
        })
    }

    /// Sign, submit, and wait for a transaction to finalize successfully.
    ///
    /// Retries on stale-nonce (error 1010) which can happen when submitting
    /// multiple transactions in quick succession — the RPC node's cached nonce
    /// may not yet reflect the previous tx's inclusion.
    async fn submit_and_finalize<Call: subxt::tx::Payload>(
        &self,
        tx: Call,
    ) -> Result<subxt::extrinsics::ExtrinsicEvents<PolkadotConfig>> {
        let mut last_err = String::new();
        for attempt in 0..3u32 {
            let at = match self.client.at_current_block().await {
                Ok(at) => at,
                Err(e) => {
                    return Err(S3ClientError::ChainError(format!(
                        "Failed to submit tx: {e}"
                    )))
                }
            };
            match at
                .transactions()
                .sign_and_submit_then_watch_default(&tx, &self.signer)
                .await
            {
                Ok(progress) => {
                    return progress.wait_for_finalized_success().await.map_err(|e| {
                        S3ClientError::ChainError(format!("Transaction failed: {e}"))
                    });
                }
                Err(e) => {
                    last_err = e.to_string();
                    // Error 1010 = InvalidTransaction::Stale (nonce already used).
                    // Wait briefly for the RPC node state to catch up, then retry.
                    if last_err.contains("1010") && attempt < 2 {
                        debug!(
                            "Stale nonce (attempt {}), retrying in {}s...",
                            attempt + 1,
                            attempt + 1
                        );
                        tokio::time::sleep(std::time::Duration::from_secs((attempt + 1) as u64))
                            .await;
                        continue;
                    }
                    return Err(S3ClientError::ChainError(format!(
                        "Failed to submit tx: {e}"
                    )));
                }
            }
        }

        Err(S3ClientError::ChainError(format!(
            "Failed to submit tx after retries: {last_err}"
        )))
    }

    /// Create an S3 bucket.
    ///
    /// `terms` + `sig` are the provider-signed agreement bundle returned by
    /// [`storage_client::ProviderClient::negotiate_terms`]. Layer 0 verifies
    /// the signature inside `establish_storage_agreement_internal`; the
    /// underlying bucket + primary agreement open atomically alongside the
    /// S3 bucket.
    pub async fn create_s3_bucket(
        &self,
        name: &str,
        provider: AccountId32,
        terms: &storage_client::AgreementTermsOf,
        sig: &sp_runtime::MultiSignature,
    ) -> Result<S3BucketId> {
        debug!("Creating S3 bucket: {}", name);

        let tx = api::tx().s3_registry().create_s3_bucket(
            name.as_bytes().to_vec(),
            convert::to_subxt_account(&provider),
            convert::agreement_terms(terms),
            convert::multisig(sig),
        );

        let events = self.submit_and_finalize(tx).await?;

        match events.find_first::<S3BucketCreated>() {
            Some(Ok(ev)) => return Ok(ev.s3_bucket_id),
            // A decode failure of a generated event means the bindings drifted
            // from the runtime — recoverable here via the name query, but worth
            // surfacing louder than the benign not-found case.
            Some(Err(e)) => warn!("Failed to decode S3BucketCreated event: {e}"),
            None => debug!("S3BucketCreated event not found in transaction"),
        }

        // Fallback: query by name
        self.get_bucket_id_by_name(name).await?.ok_or_else(|| {
            S3ClientError::ChainError("Failed to get bucket ID after creation".to_string())
        })
    }

    /// Delete an S3 bucket.
    pub async fn delete_s3_bucket(&self, bucket_id: S3BucketId) -> Result<()> {
        debug!("Deleting S3 bucket: {}", bucket_id);

        let tx = api::tx().s3_registry().delete_s3_bucket(bucket_id);

        self.submit_and_finalize(tx).await?;
        Ok(())
    }

    /// Put object metadata on chain.
    pub async fn put_object_metadata(
        &self,
        bucket_id: S3BucketId,
        key: &str,
        cid: H256,
        size: u64,
        content_type: &str,
        user_metadata: Vec<(Vec<u8>, Vec<u8>)>,
    ) -> Result<()> {
        debug!("Putting object metadata: bucket={}, key={}", bucket_id, key);

        let tx = api::tx().s3_registry().put_object_metadata(
            bucket_id,
            key.as_bytes().to_vec(),
            cid,
            size,
            content_type.as_bytes().to_vec(),
            user_metadata,
        );

        self.submit_and_finalize(tx).await?;
        Ok(())
    }

    /// Delete object metadata.
    pub async fn delete_object_metadata(&self, bucket_id: S3BucketId, key: &str) -> Result<()> {
        debug!(
            "Deleting object metadata: bucket={}, key={}",
            bucket_id, key
        );

        let tx = api::tx()
            .s3_registry()
            .delete_object_metadata(bucket_id, key.as_bytes().to_vec());

        self.submit_and_finalize(tx).await?;
        Ok(())
    }

    /// Copy object metadata.
    pub async fn copy_object_metadata(
        &self,
        src_bucket_id: S3BucketId,
        src_key: &str,
        dst_bucket_id: S3BucketId,
        dst_key: &str,
    ) -> Result<()> {
        debug!(
            "Copying object metadata: {}:{} -> {}:{}",
            src_bucket_id, src_key, dst_bucket_id, dst_key
        );

        let tx = api::tx().s3_registry().copy_object_metadata(
            src_bucket_id,
            src_key.as_bytes().to_vec(),
            dst_bucket_id,
            dst_key.as_bytes().to_vec(),
        );

        self.submit_and_finalize(tx).await?;
        Ok(())
    }

    /// Get bucket ID by name.
    pub async fn get_bucket_id_by_name(&self, name: &str) -> Result<Option<S3BucketId>> {
        let at = self
            .client
            .at_current_block()
            .await
            .map_err(|e| S3ClientError::ChainError(e.to_string()))?;
        let result = at
            .storage()
            .try_fetch(
                api::storage().s3_registry().bucket_name_to_id(),
                (convert::bounded(name.as_bytes().to_vec()),),
            )
            .await
            .map_err(|e| S3ClientError::ChainError(e.to_string()))?;

        Ok(result.and_then(|v| v.decode().ok()))
    }

    /// Get bucket info by ID.
    pub async fn get_bucket_info(&self, bucket_id: S3BucketId) -> Result<Option<BucketInfo>> {
        let at = self
            .client
            .at_current_block()
            .await
            .map_err(|e| S3ClientError::ChainError(e.to_string()))?;
        let result = at
            .storage()
            .try_fetch(api::storage().s3_registry().s3_buckets(), (bucket_id,))
            .await
            .map_err(|e| S3ClientError::ChainError(e.to_string()))?;

        match result {
            Some(value) => {
                let info = value
                    .decode()
                    .map_err(|e| S3ClientError::ChainError(e.to_string()))?;

                Ok(Some(BucketInfo {
                    s3_bucket_id: bucket_id,
                    name: String::from_utf8_lossy(&convert::unbounded(info.name)).to_string(),
                    layer0_bucket_id: info.layer0_bucket_id,
                    object_count: info.object_count,
                    total_size: info.total_size,
                    created_at: info.created_at,
                }))
            }
            None => Ok(None),
        }
    }

    /// Get object metadata.
    pub async fn get_object_metadata(
        &self,
        bucket_id: S3BucketId,
        key: &str,
    ) -> Result<Option<ChainObjectMetadata>> {
        let at = self
            .client
            .at_current_block()
            .await
            .map_err(|e| S3ClientError::ChainError(e.to_string()))?;
        let result = at
            .storage()
            .try_fetch(
                api::storage().s3_registry().objects(),
                (bucket_id, convert::bounded(key.as_bytes().to_vec())),
            )
            .await
            .map_err(|e| S3ClientError::ChainError(e.to_string()))?;

        match result {
            Some(value) => {
                let metadata = value
                    .decode()
                    .map_err(|e| S3ClientError::ChainError(e.to_string()))?;

                Ok(Some(ChainObjectMetadata {
                    cid: metadata.cid,
                    size: metadata.size,
                    last_modified: metadata.last_modified,
                    content_type: convert::unbounded(metadata.content_type),
                    etag: convert::unbounded(metadata.etag),
                    user_metadata: convert::unbounded(metadata.user_metadata)
                        .into_iter()
                        .map(|e| MetadataEntry {
                            key: convert::unbounded(e.key),
                            value: convert::unbounded(e.value),
                        })
                        .collect(),
                }))
            }
            None => Ok(None),
        }
    }

    /// List user's buckets.
    pub async fn list_user_buckets(&self) -> Result<Vec<BucketInfo>> {
        let at = self
            .client
            .at_current_block()
            .await
            .map_err(|e| S3ClientError::ChainError(e.to_string()))?;
        let result = at
            .storage()
            .try_fetch(
                api::storage().s3_registry().user_buckets(),
                (convert::to_subxt_account(&AccountId32::new(
                    self.account_id,
                )),),
            )
            .await
            .map_err(|e| S3ClientError::ChainError(e.to_string()))?;

        let bucket_ids: Vec<u64> = match result {
            Some(value) => convert::unbounded(
                value
                    .decode()
                    .map_err(|e| S3ClientError::ChainError(e.to_string()))?,
            ),
            None => vec![],
        };

        // One read per bucket, but issued together rather than in series.
        let entry = at
            .storage()
            .entry(api::storage().s3_registry().s3_buckets())
            .map_err(|e| S3ClientError::ChainError(e.to_string()))?;

        let fetched = futures::future::join_all(
            bucket_ids
                .iter()
                .map(|id| async { (*id, entry.try_fetch((*id,)).await) }),
        )
        .await;

        let mut buckets = Vec::new();
        for (id, result) in fetched {
            let Ok(Some(value)) = result else { continue };
            let Ok(info) = value.decode() else { continue };
            buckets.push(BucketInfo {
                s3_bucket_id: id,
                name: String::from_utf8_lossy(&convert::unbounded(info.name)).to_string(),
                layer0_bucket_id: info.layer0_bucket_id,
                object_count: info.object_count,
                total_size: info.total_size,
                created_at: info.created_at,
            });
        }

        Ok(buckets)
    }

    /// List objects in a bucket (basic implementation).
    pub async fn list_objects(
        &self,
        bucket_id: S3BucketId,
        params: ListObjectsParams,
    ) -> Result<ListObjectsResponse> {
        let bucket_info = self
            .get_bucket_info(bucket_id)
            .await?
            .ok_or(S3ClientError::BucketNotFound(bucket_id.to_string()))?;

        // TODO: Implement proper pagination by iterating over Objects storage
        // For now, return empty list (objects can be queried individually)
        Ok(ListObjectsResponse {
            name: bucket_info.name.into_bytes(),
            prefix: params.prefix,
            delimiter: params.delimiter,
            max_keys: params.max_keys.unwrap_or(1000),
            is_truncated: false,
            next_continuation_token: None,
            contents: vec![],
            common_prefixes: vec![],
            key_count: 0,
        })
    }
}
