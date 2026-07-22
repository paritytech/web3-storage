// SPDX-License-Identifier: Apache-2.0

//! Substrate blockchain integration for Drive Registry.
//!
//! This module provides blockchain interaction using subxt with dynamic dispatch.

use crate::FsClientError;
use file_system_primitives::DriveId;
use sp_core::H256;
use sp_runtime::AccountId32;
use std::str::FromStr;
use storage_client::{scale_decode, EventParser, Signer};
use storage_primitives::Role;
use subxt::ext::scale_value::{At, Composite, Value};
use subxt::{OnlineClient, PolkadotConfig};

/// Pallet name in the runtime configuration.
pub const PALLET_NAME: &str = "DriveRegistry";

/// Substrate client for blockchain interactions.
#[derive(Clone)]
pub struct SubstrateClient {
    api: OnlineClient<PolkadotConfig>,
    signer: Signer,
    endpoint: String,
}

impl SubstrateClient {
    /// Connect to a substrate node.
    pub async fn connect(ws_url: &str, signer: Signer) -> Result<Self, FsClientError> {
        let api = OnlineClient::<PolkadotConfig>::from_url(ws_url)
            .await
            .map_err(|e| FsClientError::Blockchain(format!("Connection failed: {e}")))?;

        Ok(Self {
            api,
            signer,
            endpoint: ws_url.to_string(),
        })
    }

    /// Get the API client.
    pub fn api(&self) -> &OnlineClient<PolkadotConfig> {
        &self.api
    }

    /// The signer.
    pub fn signer(&self) -> &Signer {
        &self.signer
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
    use storage_client::substrate::extrinsics::{dynamic_agreement_terms, dynamic_multi_signature};
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
        sig: &sp_runtime::MultiSignature,
    ) -> impl Payload {
        subxt::dynamic::tx(
            "DriveRegistry",
            "create_drive",
            vec![
                name.map(|n| subxt::dynamic::Value::from_bytes(&n))
                    .map(|v| subxt::dynamic::Value::unnamed_variant("Some", vec![v]))
                    .unwrap_or_else(|| subxt::dynamic::Value::unnamed_variant("None", vec![])),
                subxt::dynamic::Value::from_bytes(provider.as_ref() as &[u8]),
                dynamic_agreement_terms(terms),
                dynamic_multi_signature(sig),
            ],
        )
    }

    /// Delete drive extrinsic.
    #[allow(dead_code)]
    pub fn delete_drive(drive_id: DriveId) -> impl Payload {
        subxt::dynamic::tx(
            "DriveRegistry",
            "delete_drive",
            vec![subxt::dynamic::Value::u128(drive_id as u128)],
        )
    }
}

/// Storage queries for reading chain state.
///
/// Since subxt 0.50 a storage address no longer carries the keys, so each
/// map query returns the address together with the key tuple to pass to
/// `try_fetch`/`fetch`.
#[allow(dead_code)]
pub mod storage {
    use super::*;
    use subxt::storage::DynamicAddress;

    /// Query drive info.
    pub fn drive_info(drive_id: DriveId) -> (DynamicAddress<(Value,)>, (Value,)) {
        (
            subxt::dynamic::storage::<(Value,), Value>("DriveRegistry", "Drives"),
            (subxt::dynamic::Value::u128(drive_id as u128),),
        )
    }

    /// Query user drives list.
    pub fn user_drives(account: &AccountId32) -> (DynamicAddress<(Value,)>, (Value,)) {
        (
            subxt::dynamic::storage::<(Value,), Value>("DriveRegistry", "UserDrives"),
            (subxt::dynamic::Value::from_bytes(account.as_ref() as &[u8]),),
        )
    }

    /// Query bucket to drive mapping.
    pub fn bucket_to_drive(bucket_id: u64) -> (DynamicAddress<(Value,)>, (Value,)) {
        (
            subxt::dynamic::storage::<(Value,), Value>("DriveRegistry", "BucketToDrive"),
            (subxt::dynamic::Value::u128(bucket_id as u128),),
        )
    }

    /// Query next drive ID.
    pub fn next_drive_id() -> DynamicAddress<()> {
        subxt::dynamic::storage::<(), Value>("DriveRegistry", "NextDriveId")
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
        event: &subxt::events::Event<'_, PolkadotConfig>,
        block_hash: H256,
        block_number: u32,
    ) -> Option<FileSystemEvent> {
        if event.pallet_name() != PALLET_NAME {
            return None;
        }

        let fields = match event.decode_fields_unchecked_as::<Composite<()>>() {
            Ok(f) => f,
            Err(e) => {
                tracing::trace!("Failed to decode fields for {}: {e}", event.event_name());
                return None;
            }
        };

        match event.event_name() {
            "DriveCreated" => Some(FileSystemEvent::DriveCreated {
                drive_id: scale_decode::field_u64(&fields, "drive_id")?,
                owner: scale_decode::field_account(&fields, "owner")?,
                bucket_id: scale_decode::field_u64(&fields, "bucket_id")?,
                block_hash,
                block_number,
            }),
            "DriveDeleted" => Some(FileSystemEvent::DriveDeleted {
                drive_id: scale_decode::field_u64(&fields, "drive_id")?,
                owner: scale_decode::field_account(&fields, "owner")?,
                bucket_id: scale_decode::field_u64(&fields, "bucket_id")?,
                refunded: scale_decode::field_u128(&fields, "refunded")?,
                block_hash,
                block_number,
            }),
            "DriveShared" => Some(FileSystemEvent::DriveShared {
                drive_id: scale_decode::field_u64(&fields, "drive_id")?,
                member: scale_decode::field_account(&fields, "member")?,
                role: field_role(&fields, "role")?,
                block_hash,
                block_number,
            }),
            "DriveUnshared" => Some(FileSystemEvent::DriveUnshared {
                drive_id: scale_decode::field_u64(&fields, "drive_id")?,
                member: scale_decode::field_account(&fields, "member")?,
                block_hash,
                block_number,
            }),
            other => Some(FileSystemEvent::Unknown {
                variant: other.to_string(),
                block_hash,
                block_number,
            }),
        }
    }
}

/// Decode a `storage_primitives::Role`-shaped variant field.
fn field_role(fields: &Composite<()>, name: &str) -> Option<Role> {
    match scale_decode::variant_name(fields.at(name)?)?.as_str() {
        "Admin" => Some(Role::Admin),
        "Writer" => Some(Role::Writer),
        "Reader" => Some(Role::Reader),
        _ => None,
    }
}
