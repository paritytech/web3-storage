// SPDX-License-Identifier: Apache-2.0

//! Admin Client - For bucket administrators managing buckets and agreements.
//!
//! This client provides operations for:
//! - Establish storage agreement
//! - Managing bucket members and permissions
//! - Extending / topping up / terminating agreements
//! - Freezing buckets
//! - Deleting old data

use crate::base::{BaseClient, ClientConfig, ClientError, ClientResult};
use crate::event_subscription::{EventParser, StorageEvent, StorageProviderEventParser};
use crate::provider_node_request_scheme::AgreementTermsOf;
use crate::substrate::{extrinsics, storage, SubstrateClient};
use rt::pallet_storage_provider::pallet::Bucket;
use storage_primitives::{BucketId, Commitment, EndAction, Role};
use storage_subxt::api::runtime_types as rt;
use storage_subxt::api::runtime_types::pallet_storage_provider::runtime_api as rt_api;
use storage_subxt::api::runtime_types::sp_runtime::MultiSignature;
use storage_subxt::subxt::utils::AccountId32;
use storage_subxt::subxt::utils::H256;
use storage_subxt::subxt_signer;

/// Client for bucket administrators.
pub struct AdminClient {
    base: BaseClient,
    admin_account: String, // Substrate account ID
}

impl AdminClient {
    /// Create a new admin client.
    pub fn new(config: ClientConfig, admin_account: String) -> ClientResult<Self> {
        Ok(Self {
            base: BaseClient::new(config)?,
            admin_account,
        })
    }

    /// Create with default configuration.
    pub fn with_defaults(admin_account: String) -> ClientResult<Self> {
        Self::new(ClientConfig::default(), admin_account)
    }

    /// Connect to the blockchain. Must be called before any on-chain operations.
    pub async fn connect(&mut self) -> ClientResult<()> {
        self.base.connect_chain().await
    }

    /// Set a development signer (alice, bob, charlie, dave, eve, ferdie).
    /// Must be called after connect().
    pub fn set_dev_signer(&mut self, name: &str) -> ClientResult<()> {
        self.base.set_dev_signer(name)
    }

    /// Set a custom keypair signer loaded from a keyfile or seed.
    /// Must be called after connect().
    pub fn set_signer(&mut self, signer: subxt_signer::sr25519::Keypair) -> ClientResult<()> {
        self.base.set_signer(signer)
    }

    // ═════════════════════════════════════════════════════════════════════════
    // Bucket Management
    // ═════════════════════════════════════════════════════════════════════════

    /// Redeem provider-signed terms to open a bucket + primary agreement
    /// atomically.
    ///
    /// `terms` and `sig` come from the provider — typically via
    /// [`ProviderClient::negotiate_terms`](crate::provider::ProviderClient::negotiate_terms),
    /// but any source that produces a valid signature works.
    ///
    /// # Example
    /// ```no_run
    /// # use storage_client::{AdminClient, NegotiateRequest, ProviderClient};
    /// # async fn example() -> Result<(), Box<dyn std::error::Error>> {
    /// let client = AdminClient::with_defaults("5GrwvaEF...".to_string())?;
    /// let signed = ProviderClient::negotiate_terms(
    ///     "http://provider.example:3333",
    ///     &NegotiateRequest {
    ///         owner: "5GrwvaEF...".parse()?,
    ///         max_bytes: 1_000_000,
    ///         duration: 100,
    ///         price_per_byte: 1_000_000,
    ///         replica_params: None,
    ///         bucket_id: None,
    ///     },
    /// ).await?;
    /// let bucket_id = client.establish_storage_agreement(
    ///     "5FHneW46...".to_string(),
    ///     signed.terms,
    ///     signed.signature,
    /// ).await?;
    /// # Ok(())
    /// # }
    /// ```
    pub async fn establish_storage_agreement(
        &self,
        provider: String,
        terms: AgreementTermsOf,
        sig: MultiSignature,
    ) -> ClientResult<BucketId> {
        let chain = self.base.chain()?;
        let signer = chain.signer()?;
        let provider_account = SubstrateClient::parse_account(&provider)?;

        tracing::info!(
            "Establishing storage agreement with provider {} for owner {} (max_bytes={}, duration={}, nonce={})",
            provider,
            self.admin_account,
            terms.max_bytes,
            terms.duration,
            terms.nonce,
        );

        let tx = extrinsics::establish_storage_agreement(provider_account, &terms, sig);

        let tx_progress = chain
            .api()
            .tx()
            .sign_and_submit_then_watch_default(&tx, signer)
            .await
            .map_err(|e| ClientError::Chain(format!("Failed to submit tx: {e}")))?;

        let tx_in_block = tx_progress
            .wait_for_finalized()
            .await
            .map_err(|e| ClientError::Chain(format!("Transaction failed: {e}")))?;
        let raw_block_hash = tx_in_block.block_hash();
        let events = tx_in_block
            .wait_for_success()
            .await
            .map_err(|e| ClientError::Chain(format!("Transaction failed: {e}")))?;

        let block_hash = H256::from_slice(raw_block_hash.as_bytes());
        let block_number = chain
            .api()
            .blocks()
            .at(raw_block_hash)
            .await
            .map(|b| b.number())
            .unwrap_or(0);

        let parsed =
            StorageProviderEventParser::from_extrinsic_events(&events, block_hash, block_number);

        for event in parsed {
            if let StorageEvent::BucketCreated { bucket_id, .. } = event {
                tracing::info!(
                    "Storage agreement established; bucket {} created with provider {}",
                    bucket_id,
                    provider,
                );
                return Ok(bucket_id);
            }
        }

        Err(ClientError::Chain(
            "BucketCreated event not found in transaction".to_string(),
        ))
    }

    /// Add a member to a bucket with a specific role.
    pub async fn add_member(
        &self,
        bucket_id: BucketId,
        member: String,
        role: Role,
    ) -> ClientResult<()> {
        let chain = self.base.chain()?;
        let signer = chain.signer()?;
        let member_account = SubstrateClient::parse_account(&member)?;

        let tx = extrinsics::set_member(bucket_id, member_account, role);
        chain
            .api()
            .tx()
            .sign_and_submit_then_watch_default(&tx, signer)
            .await
            .map_err(|e| ClientError::Chain(format!("Failed to submit tx: {e}")))?
            .wait_for_finalized_success()
            .await
            .map_err(|e| ClientError::Chain(format!("Transaction failed: {e}")))?;

        tracing::info!(
            "Added member {} to bucket {} with role {:?}",
            member,
            bucket_id,
            role
        );
        Ok(())
    }

    /// Remove a member from a bucket.
    pub async fn remove_member(&self, bucket_id: BucketId, member: String) -> ClientResult<()> {
        let chain = self.base.chain()?;
        let signer = chain.signer()?;
        let member_account = SubstrateClient::parse_account(&member)?;

        let tx = extrinsics::remove_bucket_member(bucket_id, member_account);
        chain
            .api()
            .tx()
            .sign_and_submit_then_watch_default(&tx, signer)
            .await
            .map_err(|e| ClientError::Chain(format!("Failed to submit tx: {e}")))?
            .wait_for_finalized_success()
            .await
            .map_err(|e| ClientError::Chain(format!("Transaction failed: {e}")))?;

        tracing::info!("Removed member {} from bucket {}", member, bucket_id);
        Ok(())
    }

    /// Update a member's role in a bucket.
    pub async fn update_member_role(
        &self,
        bucket_id: BucketId,
        member: String,
        new_role: Role,
    ) -> ClientResult<()> {
        let chain = self.base.chain()?;
        let signer = chain.signer()?;
        let member_account = SubstrateClient::parse_account(&member)?;

        let tx = extrinsics::set_member(bucket_id, member_account, new_role);
        chain
            .api()
            .tx()
            .sign_and_submit_then_watch_default(&tx, signer)
            .await
            .map_err(|e| ClientError::Chain(format!("Failed to submit tx: {e}")))?
            .wait_for_finalized_success()
            .await
            .map_err(|e| ClientError::Chain(format!("Transaction failed: {e}")))?;

        tracing::info!(
            "Updated member {} in bucket {} to role {:?}",
            member,
            bucket_id,
            new_role
        );
        Ok(())
    }

    /// Freeze a bucket using the current snapshot's start sequence.
    ///
    /// After freezing, no data before the snapshot's `start_seq` can be deleted,
    /// and anyone can extend agreements (permissionless persistence).
    /// The `frozen_start_seq` parameter is informational — the chain derives it
    /// from the bucket's latest checkpoint.
    pub async fn freeze_bucket(
        &self,
        bucket_id: BucketId,
        _frozen_start_seq: u64,
    ) -> ClientResult<()> {
        let chain = self.base.chain()?;
        let signer = chain.signer()?;

        let tx = extrinsics::freeze_bucket(bucket_id);
        chain
            .api()
            .tx()
            .sign_and_submit_then_watch_default(&tx, signer)
            .await
            .map_err(|e| ClientError::Chain(format!("Failed to submit tx: {e}")))?
            .wait_for_finalized_success()
            .await
            .map_err(|e| ClientError::Chain(format!("Transaction failed: {e}")))?;

        tracing::info!("Froze bucket {}", bucket_id);
        Ok(())
    }

    // ═════════════════════════════════════════════════════════════════════════
    // Agreement Management
    // ═════════════════════════════════════════════════════════════════════════

    /// Extend an existing agreement with a provider.
    ///
    /// Anyone can extend if price hasn't increased (permissionless persistence).
    /// Only owner can extend if price increased.
    pub async fn extend_agreement(
        &self,
        bucket_id: BucketId,
        provider: String,
        additional_duration: u32,
        max_payment: u128,
    ) -> ClientResult<()> {
        let chain = self.base.chain()?;
        let signer = chain.signer()?;
        let provider_account = SubstrateClient::parse_account(&provider)?;

        let tx = extrinsics::extend_agreement(
            bucket_id,
            provider_account,
            additional_duration,
            max_payment,
        );
        chain
            .api()
            .tx()
            .sign_and_submit_then_watch_default(&tx, signer)
            .await
            .map_err(|e| ClientError::Chain(format!("Failed to submit tx: {e}")))?
            .wait_for_finalized_success()
            .await
            .map_err(|e| ClientError::Chain(format!("Transaction failed: {e}")))?;

        tracing::info!(
            "Extended agreement with {} for bucket {} by {} blocks",
            provider,
            bucket_id,
            additional_duration
        );
        Ok(())
    }

    /// Increase storage quota for an existing agreement.
    pub async fn top_up_agreement(
        &self,
        bucket_id: BucketId,
        provider: String,
        additional_bytes: u64,
        max_payment: u128,
    ) -> ClientResult<()> {
        let chain = self.base.chain()?;
        let signer = chain.signer()?;
        let provider_account = SubstrateClient::parse_account(&provider)?;

        let tx = extrinsics::top_up_agreement(
            bucket_id,
            provider_account,
            additional_bytes,
            max_payment,
        );
        chain
            .api()
            .tx()
            .sign_and_submit_then_watch_default(&tx, signer)
            .await
            .map_err(|e| ClientError::Chain(format!("Failed to submit tx: {e}")))?
            .wait_for_finalized_success()
            .await
            .map_err(|e| ClientError::Chain(format!("Transaction failed: {e}")))?;

        tracing::info!(
            "Topped up agreement with {} for bucket {} by {} bytes",
            provider,
            bucket_id,
            additional_bytes
        );
        Ok(())
    }

    /// Terminate an agreement early (admin only for primaries).
    ///
    /// You can choose to pay the provider in full or burn a percentage.
    pub async fn terminate_agreement(
        &self,
        bucket_id: BucketId,
        provider: String,
        action: EndAction,
    ) -> ClientResult<()> {
        let chain = self.base.chain()?;
        let signer = chain.signer()?;
        let provider_account = SubstrateClient::parse_account(&provider)?;

        let tx = extrinsics::end_agreement(bucket_id, provider_account, action);
        chain
            .api()
            .tx()
            .sign_and_submit_then_watch_default(&tx, signer)
            .await
            .map_err(|e| ClientError::Chain(format!("Failed to submit tx: {e}")))?
            .wait_for_finalized_success()
            .await
            .map_err(|e| ClientError::Chain(format!("Transaction failed: {e}")))?;

        tracing::info!(
            "Terminated agreement with {} for bucket {} with action {:?}",
            provider,
            bucket_id,
            action
        );
        Ok(())
    }

    /// Block extensions for a specific agreement (provider-side call).
    ///
    /// Note: the pallet requires the caller to be the provider of the agreement.
    /// This method is intended for cases where the admin is also the provider.
    pub async fn block_extensions(
        &self,
        bucket_id: BucketId,
        provider: String,
    ) -> ClientResult<()> {
        let chain = self.base.chain()?;
        let signer = chain.signer()?;

        let _ = provider; // The signer must be the provider; `provider` param is for logging.
        let tx = extrinsics::set_extensions_blocked(bucket_id, true);
        chain
            .api()
            .tx()
            .sign_and_submit_then_watch_default(&tx, signer)
            .await
            .map_err(|e| ClientError::Chain(format!("Failed to submit tx: {e}")))?
            .wait_for_finalized_success()
            .await
            .map_err(|e| ClientError::Chain(format!("Transaction failed: {e}")))?;

        tracing::info!(
            "Blocked extensions for agreement with {} on bucket {}",
            provider,
            bucket_id
        );
        Ok(())
    }

    // ═════════════════════════════════════════════════════════════════════════
    // Data Management
    // ═════════════════════════════════════════════════════════════════════════

    /// Delete old data before a sequence number.
    ///
    /// This reduces storage costs by removing data you no longer need.
    /// Cannot delete data before `frozen_start_seq` if bucket is frozen.
    ///
    /// Note: deletion is enforced off-chain. The admin signs a deletion payload and
    /// provides it to providers out-of-band. Providers include the admin signature
    /// in challenge responses to prove legitimate deletion. There is no direct
    /// on-chain extrinsic for this operation.
    pub async fn delete_before(
        &self,
        _bucket_id: BucketId,
        _new_start_seq: u64,
        _signature: Vec<u8>,
    ) -> ClientResult<()> {
        Err(ClientError::Chain(
            "delete_before is enforced off-chain: sign a deletion payload and provide it to \
             providers directly. There is no on-chain extrinsic for this operation."
                .to_string(),
        ))
    }

    /// Submit a checkpoint with provider signatures.
    ///
    /// This creates a canonical on-chain snapshot of the bucket state,
    /// enabling `challenge_checkpoint` to work against it.
    pub async fn submit_checkpoint(
        &self,
        bucket_id: BucketId,
        commitment: Commitment,
        nonce: u64, // nonce the providers signed over (echoed from their commitment)
        signatures: Vec<(String, Vec<u8>)>, // (provider SS58, signature bytes)
    ) -> ClientResult<()> {
        // TODO: replace `nonce`, `signatures` with API call

        let chain = self.base.chain()?;
        let signer = chain.signer()?;

        // Parse provider accounts
        let parsed_sigs: Vec<(AccountId32, Vec<u8>)> = signatures
            .into_iter()
            .map(|(account_str, sig)| {
                let account = SubstrateClient::parse_account(&account_str)?;
                Ok((account, sig))
            })
            .collect::<ClientResult<Vec<_>>>()?;

        let tx = extrinsics::checkpoint(bucket_id, commitment, nonce, parsed_sigs);

        let tx_progress = chain
            .api()
            .tx()
            .sign_and_submit_then_watch_default(&tx, signer)
            .await
            .map_err(|e| ClientError::Chain(format!("Failed to submit checkpoint tx: {e}")))?;

        tx_progress
            .wait_for_finalized_success()
            .await
            .map_err(|e| ClientError::Chain(format!("Checkpoint transaction failed: {e}")))?;

        tracing::info!(
            "Checkpoint submitted for bucket {} with MMR root 0x{}",
            bucket_id,
            hex::encode(commitment.mmr_root.as_bytes())
        );
        Ok(())
    }

    // ═════════════════════════════════════════════════════════════════════════
    // Monitoring
    // ═════════════════════════════════════════════════════════════════════════

    /// Get bucket information.
    pub async fn get_bucket_info(&self, bucket_id: BucketId) -> ClientResult<Bucket> {
        let chain = self.base.chain()?;

        let thunk = chain
            .api()
            .storage()
            .at_latest()
            .await
            .map_err(|e| ClientError::Chain(format!("Failed to get storage: {e}")))?
            .fetch(&storage::bucket_info(bucket_id))
            .await
            .map_err(|e| ClientError::Chain(format!("Failed to fetch bucket: {e}")))?
            .ok_or_else(|| ClientError::Chain(format!("Bucket {bucket_id} not found")))?;

        Ok(thunk)
    }

    /// List all agreements for a bucket.
    pub async fn list_bucket_agreements(
        &self,
        bucket_id: BucketId,
    ) -> ClientResult<Vec<rt_api::AgreementResponse>> {
        let chain = self.base.chain()?;

        chain
            .api()
            .runtime_api()
            .at_latest()
            .await
            .map_err(|e| ClientError::Chain(format!("runtime api: {e}")))?
            .call(
                storage_subxt::api::apis()
                    .storage_provider_api()
                    .bucket_agreements(bucket_id),
            )
            .await
            .map_err(|e| ClientError::Chain(format!("bucket_agreements: {e}")))
    }

    /// Get the buckets this admin account is a member of.
    pub async fn list_my_buckets(&self) -> ClientResult<Vec<BucketId>> {
        let chain = self.base.chain()?;
        let admin_account = SubstrateClient::parse_account(&self.admin_account)?;

        let thunk = chain
            .api()
            .storage()
            .at_latest()
            .await
            .map_err(|e| ClientError::Chain(format!("Failed to get storage: {e}")))?
            .fetch(&storage::member_buckets(&admin_account))
            .await
            .map_err(|e| ClientError::Chain(format!("Failed to fetch member buckets: {e}")))?;

        let Some(thunk) = thunk else {
            return Ok(vec![]);
        };

        Ok(thunk.0)
    }
}
