//! Substrate/chain integration for S3 client.
//!
//! Only bucket operations interact with the chain.
//! Object operations go directly to the provider HTTP API.

use crate::{BucketInfo, S3ClientError};
use s3_primitives::S3BucketId;
use std::sync::Arc;
use subxt::ext::scale_value::{At, Composite, Value, ValueDef};
use subxt::{OnlineClient, PolkadotConfig};
use subxt_signer::sr25519::Keypair;
use tracing::{debug, info};

/// Client for interacting with the substrate chain.
#[derive(Clone)]
pub struct SubstrateClient {
    /// Subxt online client.
    client: OnlineClient<PolkadotConfig>,
    /// Signer for transactions.
    signer: Option<Arc<Keypair>>,
    /// Account ID (32 bytes).
    account_id: [u8; 32],
    /// Endpoint URL.
    #[allow(dead_code)]
    endpoint: String,
}

impl SubstrateClient {
    /// Create a new substrate client.
    pub async fn new(chain_url: &str, seed_phrase: &str) -> std::result::Result<Self, String> {
        info!("Connecting to chain at {}", chain_url);

        let client = OnlineClient::<PolkadotConfig>::from_url(chain_url)
            .await
            .map_err(|e| format!("Failed to connect to chain: {e}"))?;

        let keypair = if seed_phrase.starts_with("//") {
            // Dev account like //Alice
            match seed_phrase {
                "//Alice" => subxt_signer::sr25519::dev::alice(),
                "//Bob" => subxt_signer::sr25519::dev::bob(),
                "//Charlie" => subxt_signer::sr25519::dev::charlie(),
                "//Dave" => subxt_signer::sr25519::dev::dave(),
                "//Eve" => subxt_signer::sr25519::dev::eve(),
                "//Ferdie" => subxt_signer::sr25519::dev::ferdie(),
                _ => return Err(format!("Unknown dev account: {seed_phrase}")),
            }
        } else {
            // Mnemonic phrase - parse and create keypair
            let mnemonic = bip39::Mnemonic::parse(seed_phrase)
                .map_err(|e| format!("Invalid mnemonic: {e:?}"))?;
            subxt_signer::sr25519::Keypair::from_phrase(&mnemonic, None)
                .map_err(|e| format!("Failed to create keypair: {e:?}"))?
        };

        let public_key = keypair.public_key();
        let account_id: [u8; 32] = public_key.0;
        info!("Connected to chain, account: 0x{}", hex::encode(account_id));

        Ok(Self {
            client,
            signer: Some(Arc::new(keypair)),
            account_id,
            endpoint: chain_url.to_string(),
        })
    }

    /// Get the signer keypair.
    fn signer(&self) -> std::result::Result<&Keypair, String> {
        self.signer
            .as_ref()
            .map(|s| s.as_ref())
            .ok_or_else(|| "No signer configured".to_string())
    }

    /// Create an S3 bucket.
    pub async fn create_s3_bucket(
        &self,
        name: &str,
        min_providers: u32,
    ) -> std::result::Result<S3BucketId, String> {
        debug!(
            "Creating S3 bucket: {} (min_providers={})",
            name, min_providers
        );

        let tx = subxt::dynamic::tx(
            "S3Registry",
            "create_s3_bucket",
            vec![
                Value::from_bytes(name.as_bytes()),
                Value::u128(min_providers as u128),
            ],
        );

        let signer = self.signer()?;
        let progress = self
            .client
            .tx()
            .sign_and_submit_then_watch_default(&tx, signer)
            .await
            .map_err(|e| format!("Failed to submit tx: {e}"))?;

        let events = progress
            .wait_for_finalized_success()
            .await
            .map_err(|e| format!("Transaction failed: {e}"))?;

        // Try to extract bucket ID from event
        for event in events.iter().flatten() {
            if event.pallet_name() == "S3Registry" && event.variant_name() == "S3BucketCreated" {
                if let Ok(values) = event.field_values() {
                    if let Some(id) = values.at("s3_bucket_id").and_then(|v| v.as_u128()) {
                        return Ok(id as u64);
                    }
                }
            }
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
            "S3Registry",
            "delete_s3_bucket",
            vec![Value::u128(bucket_id as u128)],
        );

        let signer = self.signer()?;
        let progress = self
            .client
            .tx()
            .sign_and_submit_then_watch_default(&tx, signer)
            .await
            .map_err(|e| format!("Failed to submit tx: {e}"))?;

        progress
            .wait_for_finalized_success()
            .await
            .map_err(|e| format!("Transaction failed: {e}"))?;

        Ok(())
    }

    /// Get bucket ID by name.
    pub async fn get_bucket_id_by_name(
        &self,
        name: &str,
    ) -> std::result::Result<Option<S3BucketId>, S3ClientError> {
        let storage_query = subxt::dynamic::storage(
            "S3Registry",
            "BucketNameToId",
            vec![Value::from_bytes(name.as_bytes())],
        );

        let result = self
            .client
            .storage()
            .at_latest()
            .await
            .map_err(|e| S3ClientError::InternalError(e.to_string()))?
            .fetch(&storage_query)
            .await
            .map_err(|e| S3ClientError::InternalError(e.to_string()))?;

        Ok(result.and_then(|v| v.as_type::<u64>().ok()))
    }

    /// Get bucket info by ID.
    pub async fn get_bucket_info(
        &self,
        bucket_id: S3BucketId,
    ) -> std::result::Result<Option<BucketInfo>, String> {
        let storage_query = subxt::dynamic::storage(
            "S3Registry",
            "S3Buckets",
            vec![Value::u128(bucket_id as u128)],
        );

        let result = self
            .client
            .storage()
            .at_latest()
            .await
            .map_err(|e| e.to_string())?
            .fetch(&storage_query)
            .await
            .map_err(|e| e.to_string())?;

        match result {
            Some(value) => {
                let decoded = value.to_value().map_err(|e| e.to_string())?;

                let name = extract_bytes_field(&decoded, "name").unwrap_or_default();
                let layer0_bucket_id = extract_u64_field(&decoded, "layer0_bucket_id").unwrap_or(0);
                let created_at = extract_u64_field(&decoded, "created_at").unwrap_or(0) as u32;

                Ok(Some(BucketInfo {
                    s3_bucket_id: bucket_id,
                    name: String::from_utf8_lossy(&name).to_string(),
                    layer0_bucket_id,
                    created_at,
                }))
            }
            None => Ok(None),
        }
    }

    /// List user's buckets.
    pub async fn list_user_buckets(&self) -> std::result::Result<Vec<BucketInfo>, String> {
        let storage_query = subxt::dynamic::storage(
            "S3Registry",
            "UserBuckets",
            vec![Value::from_bytes(self.account_id)],
        );

        let result = self
            .client
            .storage()
            .at_latest()
            .await
            .map_err(|e| e.to_string())?
            .fetch(&storage_query)
            .await
            .map_err(|e| e.to_string())?;

        let bucket_ids: Vec<u64> = match result {
            Some(value) => {
                let decoded = value.to_value().map_err(|e| e.to_string())?;
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
}

// Helper functions for extracting values from scale_value::Value

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
fn extract_bytes_from_value<T: Clone>(value: &Value<T>) -> Option<Vec<u8>> {
    match &value.value {
        ValueDef::Composite(Composite::Unnamed(values)) => {
            let bytes: Vec<u8> = values
                .iter()
                .filter_map(|v| v.as_u128().map(|n| n as u8))
                .collect();
            if bytes.len() == values.len() && !bytes.is_empty() {
                return Some(bytes);
            }
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
