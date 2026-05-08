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
use crate::event_subscription::{EventParser, StorageEvent, StorageProviderEventParser};
use crate::substrate::{extrinsics, storage, SubstrateClient};
use sp_core::H256;
use storage_primitives::{BucketId, EndAction, Role};
use subxt::ext::scale_value::{Composite, ValueDef, Variant};

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
            .map_err(|e| ClientError::Chain(format!("Failed to submit tx: {e}")))?;

        // Wait for finalization, capture the block hash, then check success.
        let tx_in_block = tx_progress
            .wait_for_finalized()
            .await
            .map_err(|e| ClientError::Chain(format!("Transaction failed: {e}")))?;

        let raw_block_hash = tx_in_block.block_hash();

        let events = tx_in_block
            .wait_for_success()
            .await
            .map_err(|e| ClientError::Chain(format!("Transaction failed: {e}")))?;

        tracing::info!("Bucket created successfully");

        // Convert subxt's H256 (primitive_types 0.12) to sp_core::H256 (0.13) via raw bytes.
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

        tracing::info!("Parsed events: {:?}", parsed);

        for event in parsed {
            tracing::info!("Received event: {event:?}");
            if let StorageEvent::BucketCreated { bucket_id, .. } = event {
                tracing::info!("Bucket created with ID: {bucket_id}");
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

        // Use different extrinsic based on whether it's a primary or replica agreement
        if let Some(params) = replica_params {
            // Replica agreement
            let tx = extrinsics::request_agreement(
                bucket_id,
                provider_account,
                max_bytes,
                duration,
                payment,
                params.sync_balance,
                params.min_sync_interval,
            );

            let tx_progress = chain
                .api()
                .tx()
                .sign_and_submit_then_watch_default(&tx, signer)
                .await
                .map_err(|e| ClientError::Chain(format!("Failed to submit tx: {e}")))?;

            tx_progress
                .wait_for_finalized_success()
                .await
                .map_err(|e| ClientError::Chain(format!("Transaction failed: {e}")))?;
        } else {
            // Primary agreement
            let tx = extrinsics::request_primary_agreement(
                bucket_id,
                provider_account,
                max_bytes,
                duration,
                payment,
            );

            let tx_progress = chain
                .api()
                .tx()
                .sign_and_submit_then_watch_default(&tx, signer)
                .await
                .map_err(|e| ClientError::Chain(format!("Failed to submit tx: {e}")))?;

            tx_progress
                .wait_for_finalized_success()
                .await
                .map_err(|e| ClientError::Chain(format!("Transaction failed: {e}")))?;
        }

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
        mmr_root: H256,
        start_seq: u64,
        leaf_count: u64,
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

        let tx = extrinsics::checkpoint(bucket_id, mmr_root, start_seq, leaf_count, parsed_sigs);

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
            hex::encode(mmr_root.as_bytes())
        );
        Ok(())
    }

    // ═════════════════════════════════════════════════════════════════════════
    // Monitoring
    // ═════════════════════════════════════════════════════════════════════════

    /// Get bucket information.
    pub async fn get_bucket_info(&self, bucket_id: BucketId) -> ClientResult<BucketInfo> {
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

        let value = thunk
            .to_value()
            .map_err(|e| ClientError::Chain(format!("Failed to decode bucket: {e}")))?;

        let min_providers = named_field(&value, "min_providers")
            .and_then(|v| v.as_u128())
            .unwrap_or(0) as u32;

        let frozen_start_seq =
            named_field(&value, "frozen_start_seq").and_then(|v| match &v.value {
                ValueDef::Variant(Variant { name, .. }) if name == "None" => None,
                ValueDef::Variant(Variant {
                    name,
                    values: Composite::Unnamed(items),
                }) if name == "Some" => items.first().and_then(|v| v.as_u128()).map(|n| n as u64),
                _ => None,
            });

        let members = named_field(&value, "members")
            .and_then(|v| match &v.value {
                // BoundedVec is a newtype: Unnamed([inner_vec]) where inner_vec is Unnamed([entries...]).
                // Peel one level to reach the actual Vec contents.
                ValueDef::Composite(Composite::Unnamed(outer)) => {
                    outer.first().and_then(|inner| match &inner.value {
                        ValueDef::Composite(Composite::Unnamed(entries)) => Some(entries),
                        _ => None,
                    })
                }
                _ => None,
            })
            .map(|items| {
                items
                    .iter()
                    .filter_map(|item| {
                        let account_bytes =
                            named_field(item, "account").and_then(decode_account_bytes)?;
                        let account = format!("0x{}", hex::encode(&account_bytes));
                        let role = named_field(item, "role").and_then(|r| match &r.value {
                            ValueDef::Variant(Variant { name, .. }) => match name.as_str() {
                                "Admin" => Some(Role::Admin),
                                "Writer" => Some(Role::Writer),
                                "Reader" => Some(Role::Reader),
                                _ => None,
                            },
                            _ => None,
                        })?;
                        Some(MemberInfo { account, role })
                    })
                    .collect()
            })
            .unwrap_or_default();

        let snapshot = named_field(&value, "snapshot").and_then(|v| match &v.value {
            ValueDef::Variant(Variant { name, .. }) if name == "None" => None,
            ValueDef::Variant(Variant {
                name,
                values: Composite::Unnamed(items),
            }) if name == "Some" => {
                let snap = items.first()?;
                let mmr_root_bytes = named_field(snap, "mmr_root")
                    .and_then(decode_account_bytes)
                    .filter(|b| b.len() == 32)?;
                let mut mmr_root = [0u8; 32];
                mmr_root.copy_from_slice(&mmr_root_bytes);
                let start_seq = named_field(snap, "start_seq")
                    .and_then(|v| v.as_u128())
                    .unwrap_or(0) as u64;
                let leaf_count = named_field(snap, "leaf_count")
                    .and_then(|v| v.as_u128())
                    .unwrap_or(0) as u64;
                let checkpoint_block = named_field(snap, "checkpoint_block")
                    .and_then(|v| v.as_u128())
                    .unwrap_or(0) as u32;
                Some(SnapshotInfo {
                    mmr_root: H256::from(mmr_root),
                    start_seq,
                    leaf_count,
                    checkpoint_block,
                })
            }
            _ => None,
        });

        Ok(BucketInfo {
            bucket_id,
            members,
            frozen_start_seq,
            min_providers,
            snapshot,
        })
    }

    /// List all agreements for a bucket.
    pub async fn list_bucket_agreements(
        &self,
        bucket_id: BucketId,
    ) -> ClientResult<Vec<AgreementInfo>> {
        let chain = self.base.chain()?;

        let storage = chain
            .api()
            .storage()
            .at_latest()
            .await
            .map_err(|e| ClientError::Chain(format!("Failed to get storage: {e}")))?;

        let mut iter = storage
            .iter(storage::agreements_for_bucket(bucket_id))
            .await
            .map_err(|e| ClientError::Chain(format!("Failed to iterate agreements: {e}")))?;

        let mut agreements = Vec::new();

        while let Some(result) = iter.next().await {
            let kv =
                result.map_err(|e| ClientError::Chain(format!("Storage iteration error: {e}")))?;

            // Key layout: [pallet_hash=16][storage_hash=16][blake2_128(bucket_id)=16][bucket_id=8]
            //             [blake2_128(provider)=16][provider=32]; provider at [72..104]
            let key = &kv.key_bytes;
            let provider = if key.len() >= 104 {
                format!("0x{}", hex::encode(&key[72..104]))
            } else {
                String::new()
            };

            let value = match kv.value.to_value() {
                Ok(v) => v,
                Err(e) => {
                    tracing::warn!("Failed to decode agreement: {e}");
                    continue;
                }
            };

            let max_bytes = named_field(&value, "max_bytes")
                .and_then(|v| v.as_u128())
                .unwrap_or(0) as u64;

            let payment_locked = named_field(&value, "payment_locked")
                .and_then(|v| v.as_u128())
                .unwrap_or(0);

            let expires_at = named_field(&value, "expires_at")
                .and_then(|v| v.as_u128())
                .unwrap_or(0) as u32;

            let is_primary = named_field(&value, "role")
                .map(|r| {
                    matches!(&r.value, ValueDef::Variant(Variant { name, .. }) if name == "Primary")
                })
                .unwrap_or(false);

            agreements.push(AgreementInfo {
                provider,
                max_bytes,
                payment_locked,
                expires_at,
                is_primary,
            });
        }

        Ok(agreements)
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

        let value = thunk
            .to_value()
            .map_err(|e| ClientError::Chain(format!("Failed to decode member buckets: {e}")))?;

        // BoundedVec is a newtype: Unnamed([inner_vec]) where inner_vec is Unnamed([entries...]).
        let bucket_ids = match &value.value {
            ValueDef::Composite(Composite::Unnamed(outer)) => outer
                .first()
                .and_then(|inner| match &inner.value {
                    ValueDef::Composite(Composite::Unnamed(entries)) => Some(entries),
                    _ => None,
                })
                .map(|entries| {
                    entries
                        .iter()
                        .filter_map(|v| v.as_u128().map(|n| n as BucketId))
                        .collect()
                })
                .unwrap_or_default(),
            _ => vec![],
        };

        Ok(bucket_ids)
    }
}

fn named_field<'a>(
    value: &'a subxt::ext::scale_value::Value<u32>,
    field: &str,
) -> Option<&'a subxt::ext::scale_value::Value<u32>> {
    match &value.value {
        ValueDef::Composite(Composite::Named(fields)) => {
            fields.iter().find(|(n, _)| n == field).map(|(_, v)| v)
        }
        _ => None,
    }
}

/// Decode raw bytes from a SCALE value, handling `AccountId32`'s newtype nesting.
///
/// `AccountId32` is decoded by subxt as `Composite::Unnamed([Composite::Unnamed([byte × 32])])`.
/// This function recurses through any level of `Composite::Unnamed` wrapping to collect
/// all `Primitive::U128` leaf values as bytes.
fn decode_account_bytes(value: &subxt::ext::scale_value::Value<u32>) -> Option<Vec<u8>> {
    let mut buf = Vec::new();
    collect_bytes_recursive(value, &mut buf);
    if buf.is_empty() {
        None
    } else {
        Some(buf)
    }
}

fn collect_bytes_recursive(value: &subxt::ext::scale_value::Value<u32>, buf: &mut Vec<u8>) {
    use subxt::ext::scale_value::Primitive;
    match &value.value {
        ValueDef::Primitive(Primitive::U128(n)) => buf.push(*n as u8),
        ValueDef::Composite(Composite::Unnamed(items)) => {
            for item in items {
                collect_bytes_recursive(item, buf);
            }
        }
        _ => {}
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
