// SPDX-License-Identifier: Apache-2.0

//! Chain access of the `provider` subcommand: [`ProviderChain`] over the
//! SDK's [`ProviderClient`].

use super::{
    ChainError, ProviderChain, ProviderRecord, ProviderRegistration, ProviderSettings, TxReceipt,
};
use sp_core::crypto::{AccountId32, Ss58AddressFormat, Ss58Codec};
use std::future::Future;
use std::time::Duration;
use storage_client::discovery::ProviderInfo;
use storage_client::{
    ClientError, ClientResult, FinalizedTx, ProviderClient,
    ProviderSettings as SdkProviderSettings, Signer,
};

/// The generic Substrate SS58 prefix, 42: how an account is displayed when
/// the chain's own `System::SS58Prefix` cannot be read.
pub const GENERIC_SS58_PREFIX: u16 = 42;

/// Upper bound on submitting one transaction and seeing it final. Finality
/// normally takes a few relay-chain blocks (well under a minute); a chain
/// whose relay stops finalizing would otherwise leave the command hanging
/// with no output. The same bound the provider node uses per submit pass.
const SUBMIT_TIMEOUT: Duration = Duration::from_secs(120);

/// Reads and writes about one signing account, over one connection. Every
/// transaction is signed by the client's signer and waited for until it is
/// final.
pub struct SubstrateProviderChain {
    client: ProviderClient,
    /// The signing account.
    account: AccountId32,
    /// SS58 prefix the signing account is displayed with.
    ss58_prefix: u16,
}

impl SubstrateProviderChain {
    /// Chain access over the connected `client`, which signs with `signer`.
    pub fn new(client: ProviderClient, signer: &Signer, ss58_prefix: u16) -> Self {
        Self {
            client,
            account: signer.account_id(),
            ss58_prefix,
        }
    }

    /// Run one write within [`SUBMIT_TIMEOUT`] and report its outcome in the
    /// provider operations' vocabulary.
    async fn submit(
        &self,
        what: &'static str,
        write: impl Future<Output = ClientResult<FinalizedTx>>,
    ) -> Result<TxReceipt, ChainError> {
        match tokio::time::timeout(SUBMIT_TIMEOUT, write).await {
            Ok(Ok(tx)) => Ok(TxReceipt {
                block_hash: Some(array_bytes::bytes2hex("0x", tx.block_hash.0)),
            }),
            Ok(Err(e)) => Err(write_error(what, e)),
            Err(_) => Err(ChainError::Submit {
                what,
                reason: format!("not finalized within {}s", SUBMIT_TIMEOUT.as_secs()),
            }),
        }
    }
}

#[async_trait::async_trait]
impl ProviderChain for SubstrateProviderChain {
    fn account(&self) -> String {
        self.account
            .to_ss58check_with_version(Ss58AddressFormat::custom(self.ss58_prefix))
    }

    /// The signing account's provider record.
    async fn fetch_provider(&self) -> Result<Option<ProviderRecord>, ChainError> {
        self.client
            .get_provider_info(&self.account)
            .await
            .map(|info| info.map(provider_record))
            .map_err(|e| read_error("provider", e))
    }

    /// The pallet's `MinStakePerByte` constant from the runtime metadata.
    async fn min_stake_per_byte(&self) -> Result<u128, ChainError> {
        self.client
            .min_stake_per_byte()
            .await
            .map_err(|e| read_error("MinStakePerByte", e))
    }

    async fn register_provider(
        &self,
        registration: &ProviderRegistration,
    ) -> Result<TxReceipt, ChainError> {
        self.submit(
            "register_provider",
            self.client.register(
                registration.multiaddr.clone(),
                registration.public_key.clone(),
                registration.stake,
            ),
        )
        .await
    }

    async fn update_provider_settings(
        &self,
        settings: &ProviderSettings,
    ) -> Result<TxReceipt, ChainError> {
        // The runtime counts blocks in `u32`; the CLI already limits
        // durations to that range, and a larger value from another caller is
        // reported here instead of truncated.
        let block_number = |field: &str, value: u64| {
            u32::try_from(value).map_err(|_| ChainError::Rejected {
                what: "update_provider_settings",
                reason: format!("{field} {value} exceeds the chain's block number range (u32)"),
            })
        };
        let settings = SdkProviderSettings {
            min_duration: block_number("min_duration", settings.min_duration)?,
            max_duration: block_number("max_duration", settings.max_duration)?,
            price_per_byte: settings.price_per_byte,
            accepting_primary: settings.accepting_primary,
            replica_sync_price: settings.replica_sync_price,
            accepting_extensions: settings.accepting_extensions,
            max_capacity: settings.max_capacity,
            min_agreement_bytes: settings.min_agreement_bytes,
        };
        self.submit(
            "update_provider_settings",
            self.client.update_settings(settings),
        )
        .await
    }

    async fn deregister_provider(&self) -> Result<TxReceipt, ChainError> {
        self.submit("deregister_provider", self.client.deregister())
            .await
    }
}

/// A failed chain read in the provider operations' vocabulary.
fn read_error(what: &'static str, reason: impl std::fmt::Display) -> ChainError {
    ChainError::Read {
        what,
        reason: reason.to_string(),
    }
}

/// A failed write in the provider operations' vocabulary: a dispatch the
/// chain rejected names the block it was recorded in; anything else is a
/// submission failure.
fn write_error(what: &'static str, error: ClientError) -> ChainError {
    match error {
        ClientError::TxRejected { block_hash, reason } => ChainError::Rejected {
            what,
            reason: format!(
                "{reason} (in block {})",
                array_bytes::bytes2hex("0x", block_hash.0)
            ),
        },
        other => ChainError::Submit {
            what,
            reason: other.to_string(),
        },
    }
}

/// The provider operations' view of a provider record.
fn provider_record(info: ProviderInfo) -> ProviderRecord {
    ProviderRecord {
        multiaddr: info.multiaddr,
        public_key: info.public_key,
        stake: info.stake,
        committed_bytes: info.committed_bytes,
        settings: ProviderSettings {
            min_duration: u64::from(info.min_duration),
            max_duration: u64::from(info.max_duration),
            price_per_byte: info.price_per_byte,
            accepting_primary: info.accepting_primary,
            replica_sync_price: info.replica_sync_price,
            accepting_extensions: info.accepting_extensions,
            max_capacity: info.max_capacity,
            min_agreement_bytes: info.min_agreement_bytes,
        },
        deregister_at: info.deregister_at.map(u64::from),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A rejected dispatch keeps the chain's reason and names the block; any
    /// other client error is a submission failure.
    #[test]
    fn write_errors_map_to_chain_errors() {
        let block_hash = sp_core::H256::repeat_byte(0xab);
        assert_eq!(
            write_error(
                "register_provider",
                ClientError::TxRejected {
                    block_hash,
                    reason: "Pallet error: StorageProvider::ProviderAlreadyRegistered".to_string(),
                }
            )
            .to_string(),
            format!(
                "register_provider rejected: Pallet error: StorageProvider::ProviderAlreadyRegistered (in block {})",
                array_bytes::bytes2hex("0x", block_hash.0)
            )
        );
        assert_eq!(
            write_error(
                "deregister_provider",
                ClientError::Chain("Failed to submit tx: offline".to_string())
            )
            .to_string(),
            "failed to submit deregister_provider: Chain error: Failed to submit tx: offline"
        );
    }
}
