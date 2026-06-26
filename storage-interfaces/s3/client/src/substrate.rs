// SPDX-License-Identifier: Apache-2.0

//! Substrate/chain integration for S3 client.

use crate::{BucketInfo, S3ClientError};
use s3_primitives::{ListObjectsParams, ListObjectsResponse, S3BucketId};
use std::sync::Arc;
use storage_client::runtime_convert as rc;
use storage_client::EventParser;
use storage_subxt::api as runtime;
use storage_subxt::subxt;
use storage_subxt::subxt::utils::AccountId32;
use storage_subxt::subxt::utils::H256;
use storage_subxt::subxt_signer;
use subxt::{OnlineClient, PolkadotConfig};
use subxt_signer::sr25519::Keypair;
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

    /// Sign, submit, and wait for a transaction to finalize successfully.
    ///
    /// Retries on stale-nonce (error 1010) which can happen when submitting
    /// multiple transactions in quick succession — the RPC node's cached nonce
    /// may not yet reflect the previous tx's inclusion.
    async fn submit_and_finalize<P: subxt::tx::Payload>(
        &self,
        tx: P,
    ) -> std::result::Result<subxt::blocks::ExtrinsicEvents<PolkadotConfig>, String> {
        let signer = self.signer()?;

        let mut last_err = String::new();
        for attempt in 0..3u32 {
            match self
                .client
                .tx()
                .sign_and_submit_then_watch_default(&tx, signer)
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
        sig: storage_subxt::api::runtime_types::sp_runtime::MultiSignature,
    ) -> std::result::Result<S3BucketId, String> {
        debug!("Creating S3 bucket: {}", name);

        let tx = runtime::tx().s3_registry().create_s3_bucket(
            name.as_bytes().to_vec(),
            provider,
            rc::to_agreement_terms(terms),
            sig,
        );

        let events = self.submit_and_finalize(tx).await?;

        // Try to extract bucket ID from the S3BucketCreated event. Block hash / number
        // aren't accessible from ExtrinsicEvents alone and this caller doesn't need them,
        // so pass placeholders.
        for parsed in S3EventParser::from_extrinsic_events(&events, H256::zero(), 0) {
            if let S3Event::S3BucketCreated { s3_bucket_id, .. } = parsed {
                return Ok(s3_bucket_id);
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

        let tx = runtime::tx().s3_registry().delete_s3_bucket(bucket_id);
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

        let tx = runtime::tx().s3_registry().put_object_metadata(
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
    pub async fn delete_object_metadata(
        &self,
        bucket_id: S3BucketId,
        key: &str,
    ) -> std::result::Result<(), String> {
        debug!(
            "Deleting object metadata: bucket={}, key={}",
            bucket_id, key
        );

        let tx = runtime::tx()
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
    ) -> std::result::Result<(), String> {
        debug!(
            "Copying object metadata: {}:{} -> {}:{}",
            src_bucket_id, src_key, dst_bucket_id, dst_key
        );

        let tx = runtime::tx().s3_registry().copy_object_metadata(
            src_bucket_id,
            src_key.as_bytes().to_vec(),
            dst_bucket_id,
            dst_key.as_bytes().to_vec(),
        );

        self.submit_and_finalize(tx).await?;
        Ok(())
    }

    /// Get bucket ID by name.
    pub async fn get_bucket_id_by_name(
        &self,
        name: &str,
    ) -> std::result::Result<Option<S3BucketId>, S3ClientError> {
        let addr = runtime::storage()
            .s3_registry()
            .bucket_name_to_id(rc::to_bounded_bytes(name.as_bytes().to_vec()));

        let result = self
            .client
            .storage()
            .at_latest()
            .await
            .map_err(|e| S3ClientError::InternalError(e.to_string()))?
            .fetch(&addr)
            .await
            .map_err(|e| S3ClientError::InternalError(e.to_string()))?;

        Ok(result)
    }

    /// Get bucket info by ID.
    pub async fn get_bucket_info(
        &self,
        bucket_id: S3BucketId,
    ) -> std::result::Result<Option<BucketInfo>, String> {
        let addr = runtime::storage().s3_registry().s3_buckets(bucket_id);

        let result = self
            .client
            .storage()
            .at_latest()
            .await
            .map_err(|e| e.to_string())?
            .fetch(&addr)
            .await
            .map_err(|e| e.to_string())?;

        Ok(result.map(|info| BucketInfo {
            s3_bucket_id: info.s3_bucket_id,
            name: String::from_utf8_lossy(&info.name.0).to_string(),
            layer0_bucket_id: info.layer0_bucket_id,
            object_count: info.object_count,
            total_size: info.total_size,
            created_at: info.created_at,
        }))
    }

    /// Get object metadata.
    pub async fn get_object_metadata(
        &self,
        bucket_id: S3BucketId,
        key: &str,
    ) -> std::result::Result<Option<ChainObjectMetadata>, String> {
        let addr = runtime::storage()
            .s3_registry()
            .objects(bucket_id, rc::to_bounded_bytes(key.as_bytes().to_vec()));

        let result = self
            .client
            .storage()
            .at_latest()
            .await
            .map_err(|e| e.to_string())?
            .fetch(&addr)
            .await
            .map_err(|e| e.to_string())?;

        Ok(result.map(|meta| ChainObjectMetadata {
            cid: meta.cid,
            size: meta.size,
            last_modified: meta.last_modified,
            content_type: meta.content_type.0,
            etag: meta.etag.0,
            user_metadata: meta
                .user_metadata
                .0
                .into_iter()
                .map(|e| MetadataEntry {
                    key: e.key.0,
                    value: e.value.0,
                })
                .collect(),
        }))
    }

    /// List user's buckets.
    pub async fn list_user_buckets(&self) -> std::result::Result<Vec<BucketInfo>, String> {
        let addr = runtime::storage()
            .s3_registry()
            .user_buckets(AccountId32::from(self.account_id));

        let result = self
            .client
            .storage()
            .at_latest()
            .await
            .map_err(|e| e.to_string())?
            .fetch(&addr)
            .await
            .map_err(|e| e.to_string())?;

        let bucket_ids: Vec<u64> = result.map(|bvec| bvec.0).unwrap_or_default();

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

// ============================================================================
// Event Parser
// ============================================================================

/// Events emitted by the [`S3Registry`](PALLET_NAME) pallet, decoded into strongly-typed
/// form.
#[derive(Clone, Debug)]
#[allow(dead_code)] // Variant fields are public API; not every consumer reads every field.
pub enum S3Event {
    /// A new S3-compatible bucket was created.
    S3BucketCreated {
        s3_bucket_id: S3BucketId,
        name: Vec<u8>,
        layer0_bucket_id: u64,
        owner: AccountId32,
        block_hash: H256,
        block_number: u32,
    },

    /// An S3 bucket was deleted.
    S3BucketDeleted {
        s3_bucket_id: S3BucketId,
        block_hash: H256,
        block_number: u32,
    },

    /// Object metadata was stored.
    ObjectPut {
        s3_bucket_id: S3BucketId,
        key: Vec<u8>,
        cid: H256,
        size: u64,
        block_hash: H256,
        block_number: u32,
    },

    /// An object's metadata was removed.
    ObjectDeleted {
        s3_bucket_id: S3BucketId,
        key: Vec<u8>,
        block_hash: H256,
        block_number: u32,
    },

    /// An object's metadata was copied to a new key (possibly across buckets).
    ObjectCopied {
        src_bucket_id: S3BucketId,
        src_key: Vec<u8>,
        dst_bucket_id: S3BucketId,
        dst_key: Vec<u8>,
        block_hash: H256,
        block_number: u32,
    },

    /// An event from the S3Registry pallet that this parser does not yet decode.
    Unknown {
        variant: String,
        block_hash: H256,
        block_number: u32,
    },
}

/// Parser for converting raw subxt events into typed [`S3Event`]s.
///
/// Mirrors `StorageProviderEventParser` from `storage-client`: stateless, with all decoding
/// done through associated functions. Use [`EventParser::from_extrinsic_events`] to scan a
/// finalized extrinsic's events at once.
pub struct S3EventParser;

impl EventParser<S3Event> for S3EventParser {
    /// Parse a single event into an [`S3Event`].
    ///
    /// Returns `None` when the event comes from a pallet other than [`PALLET_NAME`] or has
    /// unexpected field structure. Unknown variants within the right pallet surface as
    /// [`S3Event::Unknown`].
    fn parse_event_detail(
        event: &subxt::events::EventDetails<PolkadotConfig>,
        block_hash: H256,
        block_number: u32,
    ) -> Option<S3Event> {
        use runtime::s3_registry::events as ev;

        if event.pallet_name() != PALLET_NAME {
            return None;
        }

        if let Ok(Some(e)) = event.as_event::<ev::S3BucketCreated>() {
            return Some(S3Event::S3BucketCreated {
                s3_bucket_id: e.s3_bucket_id,
                name: e.name,
                layer0_bucket_id: e.layer0_bucket_id,
                owner: e.owner,
                block_hash,
                block_number,
            });
        }
        if let Ok(Some(e)) = event.as_event::<ev::S3BucketDeleted>() {
            return Some(S3Event::S3BucketDeleted {
                s3_bucket_id: e.s3_bucket_id,
                block_hash,
                block_number,
            });
        }
        if let Ok(Some(e)) = event.as_event::<ev::ObjectPut>() {
            return Some(S3Event::ObjectPut {
                s3_bucket_id: e.s3_bucket_id,
                key: e.key,
                cid: e.cid,
                size: e.size,
                block_hash,
                block_number,
            });
        }
        if let Ok(Some(e)) = event.as_event::<ev::ObjectDeleted>() {
            return Some(S3Event::ObjectDeleted {
                s3_bucket_id: e.s3_bucket_id,
                key: e.key,
                block_hash,
                block_number,
            });
        }
        if let Ok(Some(e)) = event.as_event::<ev::ObjectCopied>() {
            return Some(S3Event::ObjectCopied {
                src_bucket_id: e.src_bucket_id,
                src_key: e.src_key,
                dst_bucket_id: e.dst_bucket_id,
                dst_key: e.dst_key,
                block_hash,
                block_number,
            });
        }

        Some(S3Event::Unknown {
            variant: event.variant_name().to_string(),
            block_hash,
            block_number,
        })
    }
}
