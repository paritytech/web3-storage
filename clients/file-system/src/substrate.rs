// SPDX-License-Identifier: Apache-2.0

//! Chain access for the file system client, over the generated
//! `storage_subxt` bindings.

use crate::FsClientError;
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

    /// Sign `call`, submit it, and wait until it is finalized. Returns the
    /// events of the extrinsic, or an error if it failed.
    pub async fn submit<Call: subxt::tx::Payload>(
        &self,
        call: &Call,
    ) -> Result<subxt::extrinsics::ExtrinsicEvents<PolkadotConfig>, FsClientError> {
        self.api
            .at_current_block()
            .await
            .map_err(|e| FsClientError::Blockchain(format!("Failed to submit tx: {e}")))?
            .transactions()
            .sign_and_submit_then_watch_default(call, &self.signer)
            .await
            .map_err(|e| FsClientError::Blockchain(format!("Failed to submit tx: {e}")))?
            .wait_for_finalized_success()
            .await
            .map_err(|e| FsClientError::Blockchain(format!("Extrinsic reverted: {e}")))
    }

    /// Parse an SS58 account ID string into AccountId32.
    pub fn parse_account(account: &str) -> Result<AccountId32, FsClientError> {
        AccountId32::from_str(account)
            .map_err(|e| FsClientError::Config(format!("Invalid account ID: {e}")))
    }
}
