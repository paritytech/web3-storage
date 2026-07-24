// SPDX-License-Identifier: Apache-2.0

//! Substrate/chain integration for S3 client.

use crate::{BucketInfo, S3ClientError};
use s3_primitives::{ListObjectsParams, ListObjectsResponse, S3BucketId};
use sp_core::H256;
use sp_runtime::AccountId32;
use storage_client::Signer;
use storage_subxt::api::s3_registry::events::S3BucketCreated;
use subxt::ext::scale_value::{At, Composite, Value, ValueDef};
use subxt::{OnlineClient, PolkadotConfig};
use tracing::{debug, info};

/// Pallet name in the runtime configuration.
pub const PALLET_NAME: &str = "S3Registry";

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
    pub async fn new(chain_url: &str, signer: Signer) -> std::result::Result<Self, String> {
        info!("Connecting to chain at {}", chain_url);

        let client = OnlineClient::<PolkadotConfig>::from_url(chain_url)
            .await
            .map_err(|e| format!("Failed to connect to chain: {e}"))?;

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
    async fn submit_and_finalize(
        &self,
        tx: subxt::tx::DynamicPayload<Vec<Value>>,
    ) -> std::result::Result<subxt::extrinsics::ExtrinsicEvents<PolkadotConfig>, String> {
        let mut last_err = String::new();
        for attempt in 0..3u32 {
            let at = match self.client.at_current_block().await {
                Ok(at) => at,
                Err(e) => return Err(format!("Failed to submit tx: {e}")),
            };
            match at
                .transactions()
                .sign_and_submit_then_watch_default(&tx, &self.signer)
                .await
            {
                Ok(progress) => {
                    return progress
                        .wait_for_finalized_success()
                        .await
                        .map_err(|e| format!("Transaction failed: {e}"));
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
                    return Err(format!("Failed to submit tx: {e}"));
                }
            }
        }

        Err(format!("Failed to submit tx after retries: {last_err}"))
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
    ) -> std::result::Result<S3BucketId, String> {
        debug!("Creating S3 bucket: {}", name);

        let tx = subxt::dynamic::tx(
            PALLET_NAME,
            "create_s3_bucket",
            vec![
                Value::from_bytes(name.as_bytes()),
                Value::from_bytes(provider.as_ref() as &[u8]),
                storage_client::substrate::extrinsics::dynamic_agreement_terms(terms),
                storage_client::substrate::extrinsics::dynamic_multi_signature(sig),
            ],
        );

        let events = self.submit_and_finalize(tx).await?;

        match events.find_first::<S3BucketCreated>() {
            Some(Ok(ev)) => return Ok(ev.s3_bucket_id),
            Some(Err(e)) => debug!("Failed to decode S3BucketCreated event: {e}"),
            None => debug!("S3BucketCreated event not found in transaction"),
        }

        // Fallback: query by name
        self.get_bucket_id_by_name(name)
            .await
            .map_err(|e| e.to_string())?
            .ok_or_else(|| "Failed to get bucket ID after creation".to_string())
    }

    /// Delete an S3 bucket.
    pub async fn delete_s3_bucket(&self, bucket_id: S3BucketId) -> std::result::Result<(), String> {
        debug!("Deleting S3 bucket: {}", bucket_id);

        let tx = subxt::dynamic::tx(
            PALLET_NAME,
            "delete_s3_bucket",
            vec![Value::u128(bucket_id as u128)],
        );

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
    ) -> std::result::Result<(), String> {
        debug!("Putting object metadata: bucket={}, key={}", bucket_id, key);

        // Build metadata tuples
        let metadata_values: Vec<Value> = user_metadata
            .into_iter()
            .map(|(k, v)| Value::unnamed_composite([Value::from_bytes(&k), Value::from_bytes(&v)]))
            .collect();

        let tx = subxt::dynamic::tx(
            PALLET_NAME,
            "put_object_metadata",
            vec![
                Value::u128(bucket_id as u128),
                Value::from_bytes(key.as_bytes()),
                Value::from_bytes(cid.as_bytes()),
                Value::u128(size as u128),
                Value::from_bytes(content_type.as_bytes()),
                Value::unnamed_composite(metadata_values),
            ],
        );

        self.submit_and_finalize(tx).await?;
        Ok(())
    }

    /// Delete object metadata.
    pub async fn delete_object_metadata(
        &self,
        bucket_id: S3BucketId,
        key: &str,
    ) -> std::result::Result<(), String> {
        debug!(
            "Deleting object metadata: bucket={}, key={}",
            bucket_id, key
        );

        let tx = subxt::dynamic::tx(
            PALLET_NAME,
            "delete_object_metadata",
            vec![
                Value::u128(bucket_id as u128),
                Value::from_bytes(key.as_bytes()),
            ],
        );

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
    ) -> std::result::Result<(), String> {
        debug!(
            "Copying object metadata: {}:{} -> {}:{}",
            src_bucket_id, src_key, dst_bucket_id, dst_key
        );

        let tx = subxt::dynamic::tx(
            PALLET_NAME,
            "copy_object_metadata",
            vec![
                Value::u128(src_bucket_id as u128),
                Value::from_bytes(src_key.as_bytes()),
                Value::u128(dst_bucket_id as u128),
                Value::from_bytes(dst_key.as_bytes()),
            ],
        );

        self.submit_and_finalize(tx).await?;
        Ok(())
    }

    /// Get bucket ID by name.
    pub async fn get_bucket_id_by_name(
        &self,
        name: &str,
    ) -> std::result::Result<Option<S3BucketId>, S3ClientError> {
        let storage_query =
            subxt::dynamic::storage::<(Value,), Value>(PALLET_NAME, "BucketNameToId");

        let at = self
            .client
            .at_current_block()
            .await
            .map_err(|e| S3ClientError::InternalError(e.to_string()))?;
        let result = at
            .storage()
            .try_fetch(storage_query, (Value::from_bytes(name.as_bytes()),))
            .await
            .map_err(|e| S3ClientError::InternalError(e.to_string()))?;

        Ok(result.and_then(|v| v.decode_as::<u64>().ok()))
    }

    /// Get bucket info by ID.
    pub async fn get_bucket_info(
        &self,
        bucket_id: S3BucketId,
    ) -> std::result::Result<Option<BucketInfo>, String> {
        let storage_query = subxt::dynamic::storage::<(Value,), Value>(PALLET_NAME, "S3Buckets");

        let at = self
            .client
            .at_current_block()
            .await
            .map_err(|e| e.to_string())?;
        let result = at
            .storage()
            .try_fetch(storage_query, (Value::u128(bucket_id as u128),))
            .await
            .map_err(|e| e.to_string())?;

        match result {
            Some(value) => {
                let decoded = value.decode().map_err(|e| e.to_string())?;

                let name = extract_bytes_field(&decoded, "name").unwrap_or_default();
                let layer0_bucket_id = extract_u64_field(&decoded, "layer0_bucket_id").unwrap_or(0);
                let object_count = extract_u64_field(&decoded, "object_count").unwrap_or(0);
                let total_size = extract_u64_field(&decoded, "total_size").unwrap_or(0);
                let created_at = extract_u64_field(&decoded, "created_at").unwrap_or(0) as u32;

                Ok(Some(BucketInfo {
                    s3_bucket_id: bucket_id,
                    name: String::from_utf8_lossy(&name).to_string(),
                    layer0_bucket_id,
                    object_count,
                    total_size,
                    created_at,
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
    ) -> std::result::Result<Option<ChainObjectMetadata>, String> {
        let storage_query =
            subxt::dynamic::storage::<(Value, Value), Value>(PALLET_NAME, "Objects");

        let at = self
            .client
            .at_current_block()
            .await
            .map_err(|e| e.to_string())?;
        let result = at
            .storage()
            .try_fetch(
                storage_query,
                (
                    Value::u128(bucket_id as u128),
                    Value::from_bytes(key.as_bytes()),
                ),
            )
            .await
            .map_err(|e| e.to_string())?;

        match result {
            Some(value) => {
                let decoded = value.decode().map_err(|e| e.to_string())?;

                let cid_bytes = extract_bytes_field(&decoded, "cid").unwrap_or_default();
                let cid = if cid_bytes.len() == 32 {
                    H256::from_slice(&cid_bytes)
                } else {
                    H256::zero()
                };

                let size = extract_u64_field(&decoded, "size").unwrap_or(0);
                let last_modified = extract_u64_field(&decoded, "last_modified").unwrap_or(0);
                let content_type =
                    extract_bytes_field(&decoded, "content_type").unwrap_or_default();
                let etag = extract_bytes_field(&decoded, "etag").unwrap_or_default();
                let user_metadata = extract_metadata_entries(&decoded, "user_metadata");

                Ok(Some(ChainObjectMetadata {
                    cid,
                    size,
                    last_modified,
                    content_type,
                    etag,
                    user_metadata,
                }))
            }
            None => Ok(None),
        }
    }

    /// List user's buckets.
    pub async fn list_user_buckets(&self) -> std::result::Result<Vec<BucketInfo>, String> {
        let storage_query = subxt::dynamic::storage::<(Value,), Value>(PALLET_NAME, "UserBuckets");

        let at = self
            .client
            .at_current_block()
            .await
            .map_err(|e| e.to_string())?;
        let result = at
            .storage()
            .try_fetch(storage_query, (Value::from_bytes(self.account_id),))
            .await
            .map_err(|e| e.to_string())?;

        let bucket_ids: Vec<u64> = match result {
            Some(value) => {
                let decoded = value.decode().map_err(|e| e.to_string())?;
                extract_u64_vec(&decoded)
            }
            None => vec![],
        };

        let mut buckets = Vec::new();
        for id in bucket_ids {
            if let Ok(Some(info)) = self.get_bucket_info(id).await {
                buckets.push(info);
            }
        }

        Ok(buckets)
    }

    /// List objects in a bucket (basic implementation).
    pub async fn list_objects(
        &self,
        bucket_id: S3BucketId,
        params: ListObjectsParams,
    ) -> std::result::Result<ListObjectsResponse, String> {
        let bucket_info = self
            .get_bucket_info(bucket_id)
            .await?
            .ok_or("Bucket not found")?;

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

// Helper functions for extracting values from scale_value::Value
// Using pattern matching on ValueDef for composite access

/// Extract bytes from a named field.
fn extract_bytes_field<T: Clone>(value: &Value<T>, field: &str) -> Option<Vec<u8>> {
    let field_value = value.at(field)?;
    extract_bytes_from_value(field_value)
}

/// Extract u64 from a named field.
fn extract_u64_field<T: Clone>(value: &Value<T>, field: &str) -> Option<u64> {
    let field_value = value.at(field)?;
    field_value.as_u128().map(|v| v as u64)
}

/// Extract a vec of u64 values from a sequence/composite.
fn extract_u64_vec<T: Clone>(value: &Value<T>) -> Vec<u64> {
    let mut result = Vec::new();
    match &value.value {
        ValueDef::Composite(Composite::Unnamed(values)) => {
            for item in values {
                if let Some(v) = item.as_u128() {
                    result.push(v as u64);
                }
            }
        }
        ValueDef::Composite(Composite::Named(values)) => {
            for (_name, item) in values {
                if let Some(v) = item.as_u128() {
                    result.push(v as u64);
                }
            }
        }
        _ => {}
    }
    result
}

/// Extract bytes from a Value (handles BoundedVec and H256 encoding).
///
/// Subxt decodes wrapper types like H256 and BoundedVec<u8> as nested composites:
/// e.g. H256 becomes Composite::Unnamed([Composite::Unnamed([b0..b31])]).
/// This function handles both flat byte sequences and single-element wrapper composites.
fn extract_bytes_from_value<T: Clone>(value: &Value<T>) -> Option<Vec<u8>> {
    match &value.value {
        ValueDef::Composite(Composite::Unnamed(values)) => {
            // Try direct byte extraction (each element is a u8 encoded as u128)
            let bytes: Vec<u8> = values
                .iter()
                .filter_map(|v| v.as_u128().map(|n| n as u8))
                .collect();
            if bytes.len() == values.len() && !bytes.is_empty() {
                return Some(bytes);
            }
            // Single-element wrapper (e.g., H256 wrapping [u8; 32], BoundedVec wrapping Vec<u8>)
            if values.len() == 1 {
                return extract_bytes_from_value(&values[0]);
            }
            None
        }
        ValueDef::Composite(Composite::Named(values)) => {
            let bytes: Vec<u8> = values
                .iter()
                .filter_map(|(_, v)| v.as_u128().map(|n| n as u8))
                .collect();
            if bytes.len() == values.len() && !bytes.is_empty() {
                return Some(bytes);
            }
            if values.len() == 1 {
                return extract_bytes_from_value(&values[0].1);
            }
            None
        }
        _ => None,
    }
}

/// Extract metadata entries from user_metadata field.
fn extract_metadata_entries<T: Clone>(value: &Value<T>, field: &str) -> Vec<MetadataEntry> {
    let mut entries = Vec::new();
    if let Some(field_value) = value.at(field) {
        match &field_value.value {
            ValueDef::Composite(Composite::Unnamed(items)) => {
                for entry_value in items {
                    let key = entry_value
                        .at("key")
                        .and_then(extract_bytes_from_value)
                        .unwrap_or_default();
                    let val = entry_value
                        .at("value")
                        .and_then(extract_bytes_from_value)
                        .unwrap_or_default();
                    if !key.is_empty() {
                        entries.push(MetadataEntry { key, value: val });
                    }
                }
            }
            ValueDef::Composite(Composite::Named(items)) => {
                for (_name, entry_value) in items {
                    let key = entry_value
                        .at("key")
                        .and_then(extract_bytes_from_value)
                        .unwrap_or_default();
                    let val = entry_value
                        .at("value")
                        .and_then(extract_bytes_from_value)
                        .unwrap_or_default();
                    if !key.is_empty() {
                        entries.push(MetadataEntry { key, value: val });
                    }
                }
            }
            _ => {}
        }
    }
    entries
}
