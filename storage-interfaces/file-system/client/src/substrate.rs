//! Substrate blockchain integration for Drive Registry.
//!
//! This module provides blockchain interaction using subxt with dynamic dispatch.

use crate::FsClientError;
use file_system_primitives::{Cid, CommitStrategy, DriveId};
use sp_runtime::AccountId32;
use std::str::FromStr;
use std::sync::Arc;
use subxt::{OnlineClient, PolkadotConfig};
use subxt_signer::sr25519::Keypair;

/// Substrate client for blockchain interactions.
#[derive(Clone)]
pub struct SubstrateClient {
    api: OnlineClient<PolkadotConfig>,
    signer: Option<Arc<Keypair>>,
}

impl SubstrateClient {
    /// Connect to a substrate node.
    pub async fn connect(ws_url: &str) -> Result<Self, FsClientError> {
        let api = OnlineClient::<PolkadotConfig>::from_url(ws_url)
            .await
            .map_err(|e| FsClientError::Blockchain(format!("Connection failed: {}", e)))?;

        Ok(Self { api, signer: None })
    }

    /// Set the signer for this client.
    pub fn with_signer(mut self, signer: Keypair) -> Self {
        self.signer = Some(Arc::new(signer));
        self
    }

    /// Create a client with a development keypair (for testing).
    pub fn with_dev_signer(mut self, name: &str) -> Result<Self, FsClientError> {
        use subxt_signer::sr25519::dev;

        let keypair = match name {
            "alice" => dev::alice(),
            "bob" => dev::bob(),
            "charlie" => dev::charlie(),
            "dave" => dev::dave(),
            "eve" => dev::eve(),
            "ferdie" => dev::ferdie(),
            _ => {
                return Err(FsClientError::InvalidPath(format!(
                    "Unknown dev account: {}",
                    name
                )))
            }
        };
        self.signer = Some(Arc::new(keypair));
        Ok(self)
    }

    /// Get the API client.
    pub fn api(&self) -> &OnlineClient<PolkadotConfig> {
        &self.api
    }

    /// Get the signer if available.
    pub fn signer(&self) -> Result<&Keypair, FsClientError> {
        self.signer
            .as_ref()
            .map(|s| s.as_ref())
            .ok_or_else(|| FsClientError::InvalidPath("No signer configured".to_string()))
    }

    /// Parse an SS58 account ID string into AccountId32.
    pub fn parse_account(account: &str) -> Result<AccountId32, FsClientError> {
        AccountId32::from_str(account)
            .map_err(|e| FsClientError::InvalidPath(format!("Invalid account ID: {}", e)))
    }
}

/// Drive Registry extrinsics.
pub mod extrinsics {
    use super::*;
    use subxt::tx::Payload;

    /// Create a drive extrinsic.
    #[allow(clippy::too_many_arguments)]
    pub fn create_drive(
        name: Option<Vec<u8>>,
        max_capacity: u64,
        storage_period: u64,
        payment: u128,
        min_providers: Option<u8>,
        commit_strategy: CommitStrategy,
    ) -> impl Payload {
        // Encode CommitStrategy
        let strategy_value = match commit_strategy {
            CommitStrategy::Immediate => {
                subxt::dynamic::Value::unnamed_variant("Immediate", vec![])
            }
            CommitStrategy::Batched { interval } => {
                subxt::dynamic::Value::unnamed_variant(
                    "Batched",
                    vec![subxt::dynamic::Value::named_composite(vec![(
                        "interval",
                        subxt::dynamic::Value::u128(interval as u128),
                    )])],
                )
            }
            CommitStrategy::Manual => {
                subxt::dynamic::Value::unnamed_variant("Manual", vec![])
            }
        };

        subxt::dynamic::tx(
            "DriveRegistry",
            "create_drive",
            vec![
                // name: Option<Vec<u8>>
                name.map(|n| subxt::dynamic::Value::from_bytes(&n))
                    .map(|v| subxt::dynamic::Value::unnamed_variant("Some", vec![v]))
                    .unwrap_or_else(|| subxt::dynamic::Value::unnamed_variant("None", vec![])),
                // max_capacity: u64
                subxt::dynamic::Value::u128(max_capacity as u128),
                // storage_period: BlockNumber (u64)
                subxt::dynamic::Value::u128(storage_period as u128),
                // payment: Balance (u128)
                subxt::dynamic::Value::u128(payment),
                // min_providers: Option<u8>
                min_providers
                    .map(|p| {
                        subxt::dynamic::Value::unnamed_variant(
                            "Some",
                            vec![subxt::dynamic::Value::u128(p as u128)],
                        )
                    })
                    .unwrap_or_else(|| subxt::dynamic::Value::unnamed_variant("None", vec![])),
                // commit_strategy: CommitStrategy
                strategy_value,
            ],
        )
    }

    /// Update root CID extrinsic.
    pub fn update_root_cid(drive_id: DriveId, new_root_cid: Cid) -> impl Payload {
        subxt::dynamic::tx(
            "DriveRegistry",
            "update_root_cid",
            vec![
                subxt::dynamic::Value::u128(drive_id as u128),
                subxt::dynamic::Value::from_bytes(new_root_cid.as_bytes()),
            ],
        )
    }

    /// Clear drive extrinsic.
    pub fn clear_drive(drive_id: DriveId) -> impl Payload {
        subxt::dynamic::tx(
            "DriveRegistry",
            "clear_drive",
            vec![subxt::dynamic::Value::u128(drive_id as u128)],
        )
    }

    /// Delete drive extrinsic.
    pub fn delete_drive(drive_id: DriveId) -> impl Payload {
        subxt::dynamic::tx(
            "DriveRegistry",
            "delete_drive",
            vec![subxt::dynamic::Value::u128(drive_id as u128)],
        )
    }

    /// Update drive name extrinsic.
    pub fn update_drive_name(drive_id: DriveId, name: Option<Vec<u8>>) -> impl Payload {
        subxt::dynamic::tx(
            "DriveRegistry",
            "update_drive_name",
            vec![
                subxt::dynamic::Value::u128(drive_id as u128),
                name.map(|n| subxt::dynamic::Value::from_bytes(&n))
                    .map(|v| subxt::dynamic::Value::unnamed_variant("Some", vec![v]))
                    .unwrap_or_else(|| subxt::dynamic::Value::unnamed_variant("None", vec![])),
            ],
        )
    }
}

/// Storage queries for reading chain state.
pub mod storage {
    use super::*;
    use subxt::storage::Address;

    /// Query drive info.
    pub fn drive_info(drive_id: DriveId) -> impl Address {
        subxt::dynamic::storage(
            "DriveRegistry",
            "Drives",
            vec![subxt::dynamic::Value::u128(drive_id as u128)],
        )
    }

    /// Query user drives list.
    pub fn user_drives(account: &AccountId32) -> impl Address {
        subxt::dynamic::storage(
            "DriveRegistry",
            "UserDrives",
            vec![subxt::dynamic::Value::from_bytes(account.as_ref() as &[u8])],
        )
    }

    /// Query bucket to drive mapping.
    pub fn bucket_to_drive(bucket_id: u64) -> impl Address {
        subxt::dynamic::storage(
            "DriveRegistry",
            "BucketToDrive",
            vec![subxt::dynamic::Value::u128(bucket_id as u128)],
        )
    }

    /// Query next drive ID.
    pub fn next_drive_id() -> impl Address {
        subxt::dynamic::storage("DriveRegistry", "NextDriveId", vec![])
    }
}
