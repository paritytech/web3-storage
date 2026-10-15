// SPDX-License-Identifier: Apache-2.0

//! Chain access for the S3 client: Layer 0 bucket creation and bucket reads.

use crate::{BucketInfo, BucketMember, Result, S3ClientError};
use sp_runtime::AccountId32;
use storage_client::convert;
use storage_client::Signer;
use storage_primitives::BucketId;
use storage_subxt::api;
use storage_subxt::api::storage_provider::events::BucketCreated;
use subxt::{OnlineClient, PolkadotConfig};
use tracing::{debug, info, warn};

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

    /// The signer's account.
    pub fn account(&self) -> AccountId32 {
        AccountId32::new(self.account_id)
    }

    /// Submit `StorageProvider::create_bucket_with_primary` and return the
    /// new bucket id from the `BucketCreated` event.
    ///
    /// `terms` and `sig` are the provider-signed agreement returned by
    /// [`storage_client::ProviderClient::negotiate_terms`]. The chain creates
    /// the bucket and opens the primary agreement in one call.
    pub async fn create_bucket_with_primary(
        &self,
        provider: AccountId32,
        terms: &storage_client::AgreementTermsOf,
        sig: &sp_runtime::MultiSignature,
        visibility: storage_client::Visibility,
    ) -> Result<BucketId> {
        let tx = storage_client::substrate::extrinsics::create_bucket_with_primary(
            provider, terms, sig, visibility,
        );
        let events = self.submit_and_finalize(tx).await?;

        let created = events
            .find_first::<BucketCreated>()
            .ok_or_else(|| {
                S3ClientError::ChainError("BucketCreated event not found in transaction".into())
            })?
            .map_err(|e| {
                S3ClientError::ChainError(format!("Failed to decode BucketCreated event: {e}"))
            })?;
        Ok(created.bucket_id)
    }

    /// Read a Layer 0 bucket, or `None` if it does not exist.
    pub async fn get_bucket_info(&self, bucket_id: BucketId) -> Result<Option<BucketInfo>> {
        let at = self
            .client
            .at_current_block()
            .await
            .map_err(|e| S3ClientError::ChainError(e.to_string()))?;
        let result = at
            .storage()
            .try_fetch(api::storage().storage_provider().buckets(), (bucket_id,))
            .await
            .map_err(|e| S3ClientError::ChainError(e.to_string()))?;

        match result {
            Some(value) => {
                let bucket = value
                    .decode()
                    .map_err(|e| S3ClientError::ChainError(e.to_string()))?;
                Ok(Some(to_bucket_info(bucket_id, bucket)))
            }
            None => Ok(None),
        }
    }

    /// Every bucket `account` is a member of (`StorageProvider::MemberBuckets`),
    /// owned or shared.
    pub async fn list_member_buckets(&self, account: &AccountId32) -> Result<Vec<BucketInfo>> {
        let at = self
            .client
            .at_current_block()
            .await
            .map_err(|e| S3ClientError::ChainError(e.to_string()))?;
        let result = at
            .storage()
            .try_fetch(
                api::storage().storage_provider().member_buckets(),
                (convert::to_subxt_account(account),),
            )
            .await
            .map_err(|e| S3ClientError::ChainError(e.to_string()))?;

        let bucket_ids: Vec<BucketId> = match result {
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
            .entry(api::storage().storage_provider().buckets())
            .map_err(|e| S3ClientError::ChainError(e.to_string()))?;

        let fetched = futures::future::join_all(
            bucket_ids
                .iter()
                .map(|id| async { (*id, entry.try_fetch((*id,)).await) }),
        )
        .await;

        let mut buckets = Vec::new();
        for (id, result) in fetched {
            let value = match result {
                Ok(Some(value)) => value,
                Ok(None) => continue,
                Err(e) => {
                    warn!("Failed to fetch bucket {id}: {e}");
                    continue;
                }
            };
            match value.decode() {
                Ok(bucket) => buckets.push(to_bucket_info(id, bucket)),
                Err(e) => warn!("Failed to decode bucket {id}: {e}"),
            }
        }

        Ok(buckets)
    }
}

/// Convert the decoded `StorageProvider::Buckets` value to [`BucketInfo`].
fn to_bucket_info(
    bucket_id: BucketId,
    bucket: api::runtime_types::pallet_storage_provider::pallet::Bucket,
) -> BucketInfo {
    BucketInfo {
        bucket_id,
        members: bucket
            .members
            .0
            .iter()
            .map(|m| BucketMember {
                account: AccountId32::new(m.account.0),
                role: convert::to_sp_role(&m.role),
            })
            .collect(),
        primary_providers: bucket
            .primary_providers
            .0
            .iter()
            .map(|p| AccountId32::new(p.0))
            .collect(),
        visibility: bucket.visibility.into(),
        frozen: bucket.frozen_start_seq.is_some(),
    }
}
