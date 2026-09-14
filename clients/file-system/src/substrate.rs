// SPDX-License-Identifier: Apache-2.0

//! Substrate blockchain integration for Drive Registry.
//!
//! This module provides blockchain interaction over the generated
//! `storage_subxt` bindings.

use crate::FsClientError;
use file_system_primitives::DriveId;
use sp_runtime::AccountId32;
use std::str::FromStr;
use storage_client::Signer;
use subxt::{OnlineClient, PolkadotConfig};

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
    use storage_client::convert;
    use storage_client::AgreementTermsOf;
    use storage_subxt::api;
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
        visibility: storage_client::Visibility,
    ) -> impl Payload {
        api::tx().drive_registry().create_drive(
            name,
            convert::to_subxt_account(&provider),
            convert::agreement_terms(terms),
            convert::multisig(sig),
            convert::visibility(visibility),
        )
    }

    /// Delete drive extrinsic.
    pub fn delete_drive(drive_id: DriveId) -> impl Payload {
        api::tx().drive_registry().delete_drive(drive_id)
    }
}

/// Drive Registry storage reads.
pub mod storage {
    use super::*;
    use storage_subxt::api;
    use storage_subxt::api::runtime_types::file_system_primitives::DriveInfo;

    /// Read a drive's on-chain record, or `None` if the drive does not exist.
    pub async fn drive_info<C>(
        at: &subxt::client::ClientAtBlock<PolkadotConfig, C>,
        drive_id: DriveId,
    ) -> Result<Option<DriveInfo<subxt::utils::AccountId32, u32>>, FsClientError>
    where
        C: subxt::client::OnlineClientAtBlockT<PolkadotConfig>,
    {
        let Some(value) = at
            .storage()
            .try_fetch(api::storage().drive_registry().drives(), (drive_id,))
            .await
            .map_err(|e| FsClientError::Blockchain(format!("Storage fetch failed: {e}")))?
        else {
            return Ok(None);
        };

        value
            .decode()
            .map(Some)
            .map_err(|e| FsClientError::Blockchain(format!("Invalid drive info encoding: {e}")))
    }
}
