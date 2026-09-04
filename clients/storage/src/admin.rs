// SPDX-License-Identifier: Apache-2.0

//! Admin Client - For bucket administrators managing buckets and agreements.
//!
//! This client provides operations for:
//! - Establish storage agreement
//! - Managing bucket members and permissions
//! - Extending / topping up / terminating agreements
//! - Freezing buckets
//! - Deleting old data

use crate::agreement::SignedTerms;
use crate::base::{BaseClient, ClientConfig, ClientError, ClientResult};
use crate::convert;
use crate::substrate::{extrinsics, SubstrateClient};
use crate::Signer;
use sp_core::H256;
use sp_runtime::AccountId32;
use storage_primitives::{BucketId, Commitment, EndAction, Role};
use storage_subxt::api;
use storage_subxt::api::runtime_types::storage_primitives::ProviderRole;
use storage_subxt::api::storage_provider::events::BucketCreated;

/// Client for bucket administrators.
pub struct AdminClient {
    base: BaseClient,
    signer: Signer,
}

impl AdminClient {
    /// Create a new admin client. `signer` submits every extrinsic and
    /// identifies the admin account.
    pub fn new(config: ClientConfig, signer: Signer) -> ClientResult<Self> {
        Ok(Self {
            base: BaseClient::new(config)?,
            signer,
        })
    }

    /// The admin account: the signer's public key.
    fn admin_account(&self) -> AccountId32 {
        AccountId32::new(self.signer.keypair().public_key().0)
    }

    /// Create with default configuration.
    pub fn with_defaults(signer: Signer) -> ClientResult<Self> {
        Self::new(ClientConfig::default(), signer)
    }

    /// Connect to the blockchain and install the signer. Must be called before
    /// any on-chain operations.
    pub async fn connect(&mut self) -> ClientResult<()> {
        self.base.connect_chain().await?;
        self.base.set_signer(self.signer.clone())
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
    /// # use storage_client::{AdminClient, NegotiateRequest, ProviderClient, Signer};
    /// # async fn example() -> Result<(), Box<dyn std::error::Error>> {
    /// let client = AdminClient::with_defaults(Signer::from_seed("//Alice")?)?;
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
    ///     signed,
    ///     storage_primitives::Visibility::Private,
    /// ).await?;
    /// # Ok(())
    /// # }
    /// ```
    pub async fn establish_storage_agreement(
        &self,
        provider: String,
        signed_terms: SignedTerms,
        visibility: storage_primitives::Visibility,
    ) -> ClientResult<BucketId> {
        let SignedTerms { terms, signature } = signed_terms;
        let chain = self.base.chain()?;
        let signer = chain.signer()?;
        let provider_account = SubstrateClient::parse_account(&provider)?;

        tracing::info!(
            "Establishing storage agreement with provider {} for owner {} (max_bytes={}, duration={}, nonce={})",
            provider,
            self.admin_account(),
            terms.max_bytes,
            terms.duration,
            terms.nonce,
        );

        let tx = extrinsics::establish_storage_agreement(
            provider_account,
            &terms,
            &signature,
            visibility,
        );

        let tx_progress = chain
            .api()
            .at_current_block()
            .await
            .map_err(|e| ClientError::Chain(format!("Failed to submit tx: {e}")))?
            .transactions()
            .sign_and_submit_then_watch_default(&tx, signer)
            .await
            .map_err(|e| ClientError::Chain(format!("Failed to submit tx: {e}")))?;

        let events = tx_progress
            .wait_for_finalized_success()
            .await
            .map_err(|e| ClientError::Chain(format!("Transaction failed: {e}")))?;

        let created = events
            .find_first::<BucketCreated>()
            .ok_or_else(|| {
                ClientError::Chain("BucketCreated event not found in transaction".to_string())
            })?
            .map_err(|e| {
                ClientError::Chain(format!("Failed to decode BucketCreated event: {e}"))
            })?;

        tracing::info!(
            "Storage agreement established; bucket {} created with provider {}",
            created.bucket_id,
            provider,
        );
        Ok(created.bucket_id)
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
            .at_current_block()
            .await
            .map_err(|e| ClientError::Chain(format!("Failed to submit tx: {e}")))?
            .transactions()
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
            .at_current_block()
            .await
            .map_err(|e| ClientError::Chain(format!("Failed to submit tx: {e}")))?
            .transactions()
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
            .at_current_block()
            .await
            .map_err(|e| ClientError::Chain(format!("Failed to submit tx: {e}")))?
            .transactions()
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
            .at_current_block()
            .await
            .map_err(|e| ClientError::Chain(format!("Failed to submit tx: {e}")))?
            .transactions()
            .sign_and_submit_then_watch_default(&tx, signer)
            .await
            .map_err(|e| ClientError::Chain(format!("Failed to submit tx: {e}")))?
            .wait_for_finalized_success()
            .await
            .map_err(|e| ClientError::Chain(format!("Transaction failed: {e}")))?;

        tracing::info!("Froze bucket {}", bucket_id);
        Ok(())
    }

    /// Set bucket read visibility (admin only). Flips `Public` ⇄ `Private`
    /// unconditionally in both directions.
    pub async fn set_bucket_visibility(
        &self,
        bucket_id: BucketId,
        visibility: storage_primitives::Visibility,
    ) -> ClientResult<()> {
        let chain = self.base.chain()?;
        let signer = chain.signer()?;

        let tx = extrinsics::set_bucket_visibility(bucket_id, visibility);
        chain
            .api()
            .at_current_block()
            .await
            .map_err(|e| ClientError::Chain(format!("Failed to submit tx: {e}")))?
            .transactions()
            .sign_and_submit_then_watch_default(&tx, signer)
            .await
            .map_err(|e| ClientError::Chain(format!("Failed to submit tx: {e}")))?
            .wait_for_finalized_success()
            .await
            .map_err(|e| ClientError::Chain(format!("Transaction failed: {e}")))?;

        tracing::info!("Set bucket {} visibility to {:?}", bucket_id, visibility);
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
            .at_current_block()
            .await
            .map_err(|e| ClientError::Chain(format!("Failed to submit tx: {e}")))?
            .transactions()
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
            .at_current_block()
            .await
            .map_err(|e| ClientError::Chain(format!("Failed to submit tx: {e}")))?
            .transactions()
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
            .at_current_block()
            .await
            .map_err(|e| ClientError::Chain(format!("Failed to submit tx: {e}")))?
            .transactions()
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
            .at_current_block()
            .await
            .map_err(|e| ClientError::Chain(format!("Failed to submit tx: {e}")))?
            .transactions()
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
        signatures: Vec<(String, Vec<u8>)>, // (provider SS58, signature bytes)
    ) -> ClientResult<()> {
        let chain = self.base.chain()?;
        let signer = chain.signer()?;

        // Parse provider accounts
        let parsed_sigs: Vec<(sp_runtime::AccountId32, Vec<u8>)> = signatures
            .into_iter()
            .map(|(account_str, sig)| {
                let account = SubstrateClient::parse_account(&account_str)?;
                Ok((account, sig))
            })
            .collect::<ClientResult<Vec<_>>>()?;

        let tx = extrinsics::checkpoint(bucket_id, commitment, parsed_sigs)?;

        let tx_progress = chain
            .api()
            .at_current_block()
            .await
            .map_err(|e| ClientError::Chain(format!("Failed to submit checkpoint tx: {e}")))?
            .transactions()
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
    pub async fn get_bucket_info(&self, bucket_id: BucketId) -> ClientResult<BucketInfo> {
        let chain = self.base.chain()?;

        let at = chain.at_current_block().await?;
        let value = at
            .storage()
            .try_fetch(api::storage().storage_provider().buckets(), (bucket_id,))
            .await
            .map_err(|e| ClientError::Chain(format!("Failed to fetch bucket: {e}")))?
            .ok_or_else(|| ClientError::Chain(format!("Bucket {bucket_id} not found")))?;

        let bucket = value
            .decode()
            .map_err(|e| ClientError::Chain(format!("Failed to decode bucket: {e}")))?;

        let members = bucket
            .members
            .0
            .iter()
            .map(|m| MemberInfo {
                account: convert::account_hex(&m.account),
                role: convert::to_sp_role(&m.role),
            })
            .collect();

        let snapshot = bucket.snapshot.map(|s| SnapshotInfo {
            mmr_root: s.commitment.mmr_root,
            start_seq: s.commitment.start_seq,
            leaf_count: s.commitment.leaf_count,
            checkpoint_block: s.checkpoint_block,
        });

        Ok(BucketInfo {
            bucket_id,
            members,
            frozen_start_seq: bucket.frozen_start_seq,
            min_providers: bucket.min_providers,
            snapshot,
            visibility: bucket.visibility.into(),
        })
    }

    /// List all agreements for a bucket.
    pub async fn list_bucket_agreements(
        &self,
        bucket_id: BucketId,
    ) -> ClientResult<Vec<AgreementInfo>> {
        let chain = self.base.chain()?;

        let at = chain.at_current_block().await?;

        let agreements = at
            .runtime_apis()
            .call(
                api::runtime_apis()
                    .storage_provider_api()
                    .bucket_agreements(bucket_id),
            )
            .await
            .map_err(|e| {
                ClientError::Chain(format!("bucket_agreements runtime API failed: {e}"))
            })?;

        Ok(agreements
            .into_iter()
            .filter_map(|a| {
                let provider = convert::account_from_runtime_api(&a.provider, "provider")?;
                Some(AgreementInfo {
                    provider: convert::account_hex(&provider),
                    max_bytes: a.max_bytes,
                    payment_locked: a.payment_locked,
                    expires_at: a.expires_at,
                    is_primary: matches!(a.role, ProviderRole::Primary),
                })
            })
            .collect())
    }

    /// Get the buckets this admin account is a member of.
    pub async fn list_my_buckets(&self) -> ClientResult<Vec<BucketId>> {
        let chain = self.base.chain()?;

        let at = chain.at_current_block().await?;
        let value = at
            .storage()
            .try_fetch(
                api::storage().storage_provider().member_buckets(),
                (convert::to_subxt_account(&self.admin_account()),),
            )
            .await
            .map_err(|e| ClientError::Chain(format!("Failed to fetch member buckets: {e}")))?;

        let Some(value) = value else {
            return Ok(vec![]);
        };

        let bucket_ids = value
            .decode()
            .map_err(|e| ClientError::Chain(format!("Failed to decode member buckets: {e}")))?;

        Ok(convert::unbounded(bucket_ids))
    }
}

// Types

#[derive(Debug, Clone)]
pub struct BucketInfo {
    pub bucket_id: BucketId,
    pub members: Vec<MemberInfo>,
    pub frozen_start_seq: Option<u64>,
    pub min_providers: u32,
    pub snapshot: Option<SnapshotInfo>,
    pub visibility: storage_primitives::Visibility,
}

#[derive(Debug, Clone)]
pub struct MemberInfo {
    pub account: String,
    pub role: Role,
}

#[derive(Debug, Clone)]
pub struct SnapshotInfo {
    pub mmr_root: H256,
    pub start_seq: u64,
    pub leaf_count: u64,
    pub checkpoint_block: u32,
}

#[derive(Debug, Clone)]
pub struct AgreementInfo {
    pub provider: String,
    pub max_bytes: u64,
    pub payment_locked: u128,
    pub expires_at: u32,
    pub is_primary: bool,
}
