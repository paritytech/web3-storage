// SPDX-License-Identifier: Apache-2.0

//! Substrate blockchain integration for Drive Registry.
//!
//! This module provides blockchain interaction using subxt with typed dispatch.

use crate::FsClientError;
use file_system_primitives::DriveId;
use sp_core::H256;
use std::str::FromStr;
use std::sync::Arc;
use storage_client::runtime_convert as rc;
use storage_client::EventParser;
use storage_primitives::Role;
use storage_subxt::api as runtime;
use storage_subxt::subxt;
use storage_subxt::subxt::utils::AccountId32;
use storage_subxt::subxt_signer;
use subxt::{OnlineClient, PolkadotConfig};
use subxt_signer::sr25519::Keypair;

/// Pallet name in the runtime configuration.
pub const PALLET_NAME: &str = "DriveRegistry";

/// Substrate client for blockchain interactions.
#[derive(Clone)]
pub struct SubstrateClient {
    api: OnlineClient<PolkadotConfig>,
    signer: Option<Arc<Keypair>>,
    endpoint: String,
}

impl SubstrateClient {
    /// Connect to a substrate node.
    pub async fn connect(ws_url: &str) -> Result<Self, FsClientError> {
        let api = OnlineClient::<PolkadotConfig>::from_url(ws_url)
            .await
            .map_err(|e| FsClientError::Blockchain(format!("Connection failed: {e}")))?;

        Ok(Self {
            api,
            signer: None,
            endpoint: ws_url.to_string(),
        })
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
                return Err(FsClientError::Config(format!(
                    "Unknown dev account: {name}"
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
            .ok_or(FsClientError::NoSigner)
    }

    /// Get the signer keypair (cloned) if available.
    ///
    /// This is useful when you need to pass the keypair to another component.
    pub fn signer_keypair(&self) -> Result<Keypair, FsClientError> {
        self.signer
            .as_ref()
            .map(|s| (**s).clone())
            .ok_or(FsClientError::NoSigner)
    }

    /// Get the WebSocket endpoint URL.
    pub fn endpoint(&self) -> &str {
        &self.endpoint
    }

    /// Parse an SS58 account ID string into AccountId32.
    pub fn parse_account(account: &str) -> Result<AccountId32, FsClientError> {
        AccountId32::from_str(account)
            .map_err(|e| FsClientError::Config(format!("Invalid account ID: {e}")))
    }
}

/// Drive Registry extrinsics.
pub mod extrinsics {
    use super::*;
    use storage_client::AgreementTermsOf;
    use subxt::tx::Payload;

    /// Build a `DriveRegistry::create_drive` extrinsic.
    ///
    /// `terms` + `sig` are the provider-signed agreement bundle returned by
    /// `ProviderClient::negotiate_terms`. Layer 0 verifies the signature
    /// inside `establish_storage_agreement_internal`; bucket creation +
    /// primary-agreement opening happen atomically alongside drive
    /// registration.
    pub fn create_drive(
        name: Option<Vec<u8>>,
        provider: AccountId32,
        terms: &AgreementTermsOf,
        sig: storage_subxt::api::runtime_types::sp_runtime::MultiSignature,
    ) -> impl Payload {
        runtime::tx().drive_registry().create_drive(
            name,
            provider,
            rc::to_agreement_terms(terms),
            sig,
        )
    }

    /// Delete drive extrinsic.
    #[allow(dead_code)]
    pub fn delete_drive(drive_id: DriveId) -> impl Payload {
        runtime::tx().drive_registry().delete_drive(drive_id)
    }
}

/// Storage queries for reading chain state.
#[allow(dead_code)]
pub mod storage {
    use super::*;
    use subxt::storage::Address;

    /// Query drive info.
    pub fn drive_info(drive_id: DriveId) -> impl Address {
        runtime::storage().drive_registry().drives(drive_id)
    }

    /// Query user drives list.
    pub fn user_drives(account: &AccountId32) -> impl Address {
        runtime::storage()
            .drive_registry()
            .user_drives(account.clone())
    }

    /// Query bucket to drive mapping.
    pub fn bucket_to_drive(bucket_id: u64) -> impl Address {
        runtime::storage()
            .drive_registry()
            .bucket_to_drive(bucket_id)
    }

    /// Query next drive ID.
    pub fn next_drive_id() -> impl Address {
        runtime::storage().drive_registry().next_drive_id()
    }
}

// ============================================================================
// Event Parser
// ============================================================================

/// Events emitted by the [`DriveRegistry`](PALLET_NAME) pallet, decoded into
/// strongly-typed form.
#[derive(Clone, Debug)]
#[allow(dead_code)] // Variant fields are public API; not every consumer reads every field.
pub enum FileSystemEvent {
    /// A new drive was created.
    DriveCreated {
        drive_id: DriveId,
        owner: AccountId32,
        bucket_id: u64,
        block_hash: H256,
        block_number: u32,
    },

    /// A drive was deleted; remaining agreement balance was refunded.
    DriveDeleted {
        drive_id: DriveId,
        owner: AccountId32,
        bucket_id: u64,
        refunded: u128,
        block_hash: H256,
        block_number: u32,
    },

    /// A drive was shared with a member.
    DriveShared {
        drive_id: DriveId,
        member: AccountId32,
        role: Role,
        block_hash: H256,
        block_number: u32,
    },

    /// A member was removed from a shared drive.
    DriveUnshared {
        drive_id: DriveId,
        member: AccountId32,
        block_hash: H256,
        block_number: u32,
    },

    /// An event from the DriveRegistry pallet that this parser does not yet decode.
    Unknown {
        variant: String,
        block_hash: H256,
        block_number: u32,
    },
}

/// Parser for converting raw subxt events into typed [`FileSystemEvent`]s.
///
/// Mirrors `StorageProviderEventParser` from `storage-client`: stateless, with all
/// decoding done through associated functions. Use [`EventParser::from_extrinsic_events`]
/// to scan a finalized extrinsic's events at once.
pub struct FileSystemEventParser;

impl EventParser<FileSystemEvent> for FileSystemEventParser {
    /// Parse a single event into a [`FileSystemEvent`].
    ///
    /// Returns `None` when the event comes from a pallet other than [`PALLET_NAME`]
    /// or has unexpected field structure. Unknown variants within the right pallet
    /// surface as [`FileSystemEvent::Unknown`].
    fn parse_event_detail(
        event: &subxt::events::EventDetails<PolkadotConfig>,
        block_hash: H256,
        block_number: u32,
    ) -> Option<FileSystemEvent> {
        use runtime::drive_registry::events as ev;
        use storage_subxt::api::runtime_types as rt;

        if event.pallet_name() != PALLET_NAME {
            return None;
        }

        if let Ok(Some(e)) = event.as_event::<ev::DriveCreated>() {
            return Some(FileSystemEvent::DriveCreated {
                drive_id: e.drive_id,
                owner: e.owner,
                bucket_id: e.bucket_id,
                block_hash,
                block_number,
            });
        }
        if let Ok(Some(e)) = event.as_event::<ev::DriveDeleted>() {
            return Some(FileSystemEvent::DriveDeleted {
                drive_id: e.drive_id,
                owner: e.owner,
                bucket_id: e.bucket_id,
                refunded: e.refunded,
                block_hash,
                block_number,
            });
        }
        if let Ok(Some(e)) = event.as_event::<ev::DriveShared>() {
            let role = match e.role {
                rt::storage_primitives::Role::Admin => Role::Admin,
                rt::storage_primitives::Role::Writer => Role::Writer,
                rt::storage_primitives::Role::Reader => Role::Reader,
            };
            return Some(FileSystemEvent::DriveShared {
                drive_id: e.drive_id,
                member: e.member,
                role,
                block_hash,
                block_number,
            });
        }
        if let Ok(Some(e)) = event.as_event::<ev::DriveUnshared>() {
            return Some(FileSystemEvent::DriveUnshared {
                drive_id: e.drive_id,
                member: e.member,
                block_hash,
                block_number,
            });
        }

        Some(FileSystemEvent::Unknown {
            variant: event.variant_name().to_string(),
            block_hash,
            block_number,
        })
    }
}
