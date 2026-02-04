//! Admin Client - For bucket administrators managing buckets and agreements.
//!
//! This client provides operations for:
//! - Creating and configuring buckets
//! - Managing bucket members and permissions
//! - Requesting storage agreements from providers
//! - Terminating agreements
//! - Freezing buckets
//! - Deleting old data

use crate::base::{BaseClient, ClientConfig, ClientError, ClientResult};
use crate::substrate::{extrinsics, SubstrateClient};
use sp_core::H256;
use sp_runtime::AccountId32;
use storage_primitives::{BucketId, EndAction, Role};

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

    // ═════════════════════════════════════════════════════════════════════════
    // Bucket Management
    // ═════════════════════════════════════════════════════════════════════════

    /// Create a new storage bucket.
    ///
    /// # Parameters
    /// - `min_providers`: Minimum number of provider signatures required for checkpoints
    ///
    /// # Returns
    /// The bucket ID of the newly created bucket.
    ///
    /// # Example
    /// ```no_run
    /// # use storage_client::AdminClient;
    /// # async fn example() -> Result<(), Box<dyn std::error::Error>> {
    /// let client = AdminClient::with_defaults("5GrwvaEF...".to_string())?;
    /// let bucket_id = client.create_bucket(2).await?;
    /// println!("Created bucket {}", bucket_id);
    /// # Ok(())
    /// # }
    /// ```
    pub async fn create_bucket(&self, min_providers: u32) -> ClientResult<BucketId> {
        let chain = self.base.chain()?;
        let signer = chain.signer()?;

        tracing::info!(
            "Creating bucket with min_providers={} for admin {}",
            min_providers,
            self.admin_account
        );

        // Create and submit the extrinsic
        let tx = extrinsics::create_bucket(min_providers);

        let tx_progress = chain
            .api()
            .tx()
            .sign_and_submit_then_watch_default(&tx, signer)
            .await
            .map_err(|e| ClientError::Chain(format!("Failed to submit tx: {}", e)))?;

        // Wait for finalization and extract bucket ID from events
        let events = tx_progress
            .wait_for_finalized_success()
            .await
            .map_err(|e| ClientError::Chain(format!("Transaction failed: {}", e)))?;

        // Extract bucket ID from BucketCreated event
        // For now, return a placeholder - in production, parse the event
        tracing::info!("Bucket created successfully");
        Ok(1) // TODO: Extract from event
    }

    /// Add a member to a bucket with a specific role.
    pub async fn add_member(
        &self,
        bucket_id: BucketId,
        member: String,
        role: Role,
    ) -> ClientResult<()> {
        // TODO: Submit extrinsic
        tracing::info!(
            "Would add member {} to bucket {} with role {:?}",
            member,
            bucket_id,
            role
        );
        Ok(())
    }

    /// Remove a member from a bucket.
    pub async fn remove_member(&self, bucket_id: BucketId, member: String) -> ClientResult<()> {
        // TODO: Submit extrinsic
        tracing::info!("Would remove member {} from bucket {}", member, bucket_id);
        Ok(())
    }

    /// Update a member's role in a bucket.
    pub async fn update_member_role(
        &self,
        bucket_id: BucketId,
        member: String,
        new_role: Role,
    ) -> ClientResult<()> {
        // TODO: Submit extrinsic
        tracing::info!(
            "Would update member {} in bucket {} to role {:?}",
            member,
            bucket_id,
            new_role
        );
        Ok(())
    }

    /// Freeze a bucket at a specific sequence number.
    ///
    /// After freezing, no data before `frozen_start_seq` can be deleted,
    /// and anyone can extend agreements (permissionless persistence).
    pub async fn freeze_bucket(
        &self,
        bucket_id: BucketId,
        frozen_start_seq: u64,
    ) -> ClientResult<()> {
        // TODO: Submit extrinsic
        tracing::info!(
            "Would freeze bucket {} at seq {}",
            bucket_id,
            frozen_start_seq
        );
        Ok(())
    }

    // ═════════════════════════════════════════════════════════════════════════
    // Agreement Management
    // ═════════════════════════════════════════════════════════════════════════

    /// Request a storage agreement from a provider.
    ///
    /// # Parameters
    /// - `provider`: Provider's account ID
    /// - `max_bytes`: Maximum storage capacity to reserve
    /// - `duration`: Agreement duration in blocks
    /// - `payment`: Total payment to lock (will be released to provider on success)
    ///
    /// # Example
    /// ```no_run
    /// # use storage_client::AdminClient;
    /// # async fn example() -> Result<(), Box<dyn std::error::Error>> {
    /// let client = AdminClient::with_defaults("5GrwvaEF...".to_string())?;
    /// let provider = "5FHneW46...".to_string();
    ///
    /// client.request_agreement(
    ///     1,                      // bucket_id
    ///     provider,
    ///     10 * 1024 * 1024 * 1024, // 10 GB
    ///     100_000,                // duration blocks (~2 weeks)
    ///     1_000_000_000_000,      // payment
    ///     None                    // primary (not replica)
    /// ).await?;
    /// # Ok(())
    /// # }
    /// ```
    pub async fn request_agreement(
        &self,
        bucket_id: BucketId,
        provider: String,
        max_bytes: u64,
        duration: u32,
        payment: u128,
        replica_params: Option<ReplicaParams>,
    ) -> ClientResult<()> {
        let chain = self.base.chain()?;
        let signer = chain.signer()?;

        tracing::info!(
            "Requesting agreement from {} for bucket {}: {} bytes, {} blocks, {} payment",
            provider,
            bucket_id,
            max_bytes,
            duration,
            payment
        );

        // Parse provider account
        let provider_account = SubstrateClient::parse_account(&provider)?;

        // Extract replica_for if present
        let replica_for = replica_params
            .as_ref()
            .and_then(|p| p.primary_provider.as_ref())
            .map(|p| SubstrateClient::parse_account(p))
            .transpose()?;

        // Create and submit the extrinsic
        let tx = extrinsics::request_agreement(
            bucket_id,
            provider_account,
            max_bytes,
            duration,
            payment,
            replica_for,
        );

        let tx_progress = chain
            .api()
            .tx()
            .sign_and_submit_then_watch_default(&tx, signer)
            .await
            .map_err(|e| ClientError::Chain(format!("Failed to submit tx: {}", e)))?;

        tx_progress
            .wait_for_finalized_success()
            .await
            .map_err(|e| ClientError::Chain(format!("Transaction failed: {}", e)))?;

        tracing::info!("Agreement request submitted successfully");
        Ok(())
    }

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
        // TODO: Submit extrinsic
        tracing::info!(
            "Would extend agreement with {} for bucket {} by {} blocks",
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
        // TODO: Submit extrinsic
        tracing::info!(
            "Would top up agreement with {} for bucket {} by {} bytes",
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
        // TODO: Submit extrinsic
        tracing::info!(
            "Would terminate agreement with {} for bucket {} with action {:?}",
            provider,
            bucket_id,
            action
        );
        Ok(())
    }

    /// Block extensions for a specific agreement.
    ///
    /// Use this to prevent unwanted third-party extensions of your agreement.
    pub async fn block_extensions(
        &self,
        bucket_id: BucketId,
        provider: String,
    ) -> ClientResult<()> {
        // TODO: Submit extrinsic
        tracing::info!(
            "Would block extensions for agreement with {} on bucket {}",
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
    pub async fn delete_before(
        &self,
        bucket_id: BucketId,
        new_start_seq: u64,
        signature: Vec<u8>,
    ) -> ClientResult<()> {
        // TODO: Submit extrinsic
        tracing::info!(
            "Would delete data before seq {} in bucket {}",
            new_start_seq,
            bucket_id
        );
        Ok(())
    }

    /// Submit a checkpoint with provider signatures.
    ///
    /// This creates a canonical snapshot of the bucket state.
    pub async fn submit_checkpoint(
        &self,
        bucket_id: BucketId,
        mmr_root: H256,
        start_seq: u64,
        leaf_count: u64,
        signatures: Vec<(String, Vec<u8>)>, // (provider, signature)
    ) -> ClientResult<()> {
        // TODO: Submit extrinsic
        tracing::info!(
            "Would submit checkpoint for bucket {} with {} signatures",
            bucket_id,
            signatures.len()
        );
        Ok(())
    }

    // ═════════════════════════════════════════════════════════════════════════
    // Monitoring
    // ═════════════════════════════════════════════════════════════════════════

    /// Get bucket information.
    pub async fn get_bucket_info(&self, bucket_id: BucketId) -> ClientResult<BucketInfo> {
        // TODO: Query via Runtime API
        Ok(BucketInfo {
            bucket_id,
            members: vec![],
            frozen_start_seq: None,
            min_providers: 0,
            snapshot: None,
        })
    }

    /// List all agreements for a bucket.
    pub async fn list_bucket_agreements(
        &self,
        bucket_id: BucketId,
    ) -> ClientResult<Vec<AgreementInfo>> {
        // TODO: Query via Runtime API
        Ok(vec![])
    }

    /// Get your buckets.
    pub async fn list_my_buckets(&self) -> ClientResult<Vec<BucketId>> {
        // TODO: Query chain storage
        Ok(vec![])
    }
}

// Types

#[derive(Debug, Clone)]
pub struct ReplicaParams {
    /// The primary provider this replica syncs from
    pub primary_provider: Option<String>,
    /// Initial sync balance to fund per-sync payments
    pub sync_balance: u128,
    /// Minimum blocks between sync confirmations
    pub min_sync_interval: u32,
}

#[derive(Debug, Clone)]
pub struct BucketInfo {
    pub bucket_id: BucketId,
    pub members: Vec<MemberInfo>,
    pub frozen_start_seq: Option<u64>,
    pub min_providers: u32,
    pub snapshot: Option<SnapshotInfo>,
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
