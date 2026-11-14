// SPDX-License-Identifier: Apache-2.0

//! Chain access of the `provider` subcommand: [`ProviderChain`] over the
//! SDK's [`SubstrateClient`].

use provider_helpers::{
    ChainError, ProviderChain, ProviderRecord, ProviderRegistration, ProviderSettings, TxReceipt,
};
use sp_core::crypto::{AccountId32, Ss58AddressFormat, Ss58Codec};
use std::time::Duration;
use storage_client::substrate::{extrinsics, SubstrateClient};
use storage_subxt::api;
use storage_subxt::api::runtime_types::pallet_storage_provider::pallet::ProviderInfo as RuntimeProviderInfo;
use subxt::error::TransactionEventsError;

/// The generic Substrate SS58 prefix, 42: how an account is displayed when
/// the chain's own `System::SS58Prefix` cannot be read.
pub const GENERIC_SS58_PREFIX: u16 = 42;

/// Upper bound on submitting one transaction and seeing it final. Finality
/// normally takes a few relay-chain blocks (well under a minute); a chain
/// whose relay stops finalizing would otherwise leave the command hanging
/// with no output. The same bound the provider node uses per submit pass.
const SUBMIT_TIMEOUT: Duration = Duration::from_secs(120);

/// The chain's `System::SS58Prefix` constant: the prefix every address the
/// subcommand prints uses, so they match the chain's own tools.
pub async fn chain_ss58_prefix(client: &SubstrateClient) -> Result<u16, ChainError> {
    let address = api::constants().system().ss58_prefix();
    client
        .at_current_block()
        .await
        .map_err(|e| read_error("SS58Prefix", e))?
        .constants()
        .entry(address)
        .map_err(|e| read_error("SS58Prefix", e))
}

/// Reads and writes about one signing account, over one connection. Every
/// transaction is signed by the client's signer and waited for until it is
/// final.
pub struct HelperChain {
    client: SubstrateClient,
    /// The signing account's raw 32 bytes.
    account: [u8; 32],
    /// SS58 prefix the signing account is displayed with.
    ss58_prefix: u16,
}

impl HelperChain {
    /// Chain access signed by `client`'s signer; fails when it has none.
    pub fn new(client: SubstrateClient, ss58_prefix: u16) -> Result<Self, ChainError> {
        let account = client
            .signer()
            .map_err(|e| read_error("signer", e))?
            .keypair()
            .public_key()
            .0;
        Ok(Self {
            client,
            account,
            ss58_prefix,
        })
    }

    /// Sign `tx`, submit it and wait for it to be final and successful,
    /// within [`SUBMIT_TIMEOUT`].
    async fn submit(
        &self,
        tx: &impl subxt::tx::Payload,
        what: &'static str,
    ) -> Result<TxReceipt, ChainError> {
        tokio::time::timeout(SUBMIT_TIMEOUT, self.submit_once(tx, what))
            .await
            .unwrap_or_else(|_| {
                Err(ChainError::Submit {
                    what,
                    reason: format!("not finalized within {}s", SUBMIT_TIMEOUT.as_secs()),
                })
            })
    }

    /// The unbounded submit-and-watch pass [`Self::submit`] puts a deadline on.
    async fn submit_once(
        &self,
        tx: &impl subxt::tx::Payload,
        what: &'static str,
    ) -> Result<TxReceipt, ChainError> {
        let submit_error = |reason: &dyn std::fmt::Display| ChainError::Submit {
            what,
            reason: reason.to_string(),
        };
        let signer = self.client.signer().map_err(|e| submit_error(&e))?;
        let in_block = self
            .client
            .at_current_block()
            .await
            .map_err(|e| submit_error(&e))?
            .transactions()
            .sign_and_submit_then_watch_default(tx, signer)
            .await
            .map_err(|e| submit_error(&e))?
            .wait_for_finalized()
            .await
            .map_err(|e| submit_error(&e))?;
        // The block is known before the dispatch outcome, so a rejection
        // names the block it was recorded in.
        let block_hash = array_bytes::bytes2hex("0x", in_block.block_hash().0);
        match in_block.wait_for_success().await {
            Ok(_) => Ok(TxReceipt {
                block_hash: Some(block_hash),
            }),
            Err(TransactionEventsError::ExtrinsicFailed(e)) => Err(ChainError::Rejected {
                what,
                reason: format!("{e} (in block {block_hash})"),
            }),
            Err(e) => Err(submit_error(&e)),
        }
    }
}

#[async_trait::async_trait]
impl ProviderChain for HelperChain {
    fn account(&self) -> String {
        AccountId32::from(self.account)
            .to_ss58check_with_version(Ss58AddressFormat::custom(self.ss58_prefix))
    }

    /// The signing account's `StorageProvider::Providers` entry.
    async fn fetch_provider(&self) -> Result<Option<ProviderRecord>, ChainError> {
        let providers = api::storage().storage_provider().providers().unvalidated();
        let account = subxt::utils::AccountId32(self.account);
        self.client
            .at_current_block()
            .await
            .map_err(|e| read_error("provider", e))?
            .storage()
            .try_fetch(providers, (account,))
            .await
            .map_err(|e| read_error("provider", e))?
            .map(|value| value.decode().map(provider_record))
            .transpose()
            .map_err(|e| read_error("provider", e))
    }

    /// The pallet's `MinStakePerByte` constant from the runtime metadata.
    async fn min_stake_per_byte(&self) -> Result<u128, ChainError> {
        let address = api::constants().storage_provider().min_stake_per_byte();
        self.client
            .at_current_block()
            .await
            .map_err(|e| read_error("MinStakePerByte", e))?
            .constants()
            .entry(address)
            .map_err(|e| read_error("MinStakePerByte", e))
    }

    async fn register_provider(
        &self,
        registration: &ProviderRegistration,
    ) -> Result<TxReceipt, ChainError> {
        let tx = extrinsics::register_provider(
            registration.multiaddr.as_bytes().to_vec(),
            registration.public_key.clone(),
            registration.stake,
        );
        self.submit(&tx, "register_provider").await
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
        let tx = extrinsics::update_provider_settings(
            block_number("min_duration", settings.min_duration)?,
            block_number("max_duration", settings.max_duration)?,
            settings.price_per_byte,
            settings.accepting_primary,
            settings.replica_sync_price,
            settings.accepting_extensions,
            settings.max_capacity,
            settings.min_agreement_bytes,
        );
        self.submit(&tx, "update_provider_settings").await
    }

    async fn deregister_provider(&self) -> Result<TxReceipt, ChainError> {
        let tx = extrinsics::deregister_provider();
        self.submit(&tx, "deregister_provider").await
    }
}

/// A failed chain read in the helper's vocabulary.
fn read_error(what: &'static str, reason: impl std::fmt::Display) -> ChainError {
    ChainError::Read {
        what,
        reason: reason.to_string(),
    }
}

/// The helper's view of a `Providers` entry.
fn provider_record(info: RuntimeProviderInfo) -> ProviderRecord {
    ProviderRecord {
        multiaddr: String::from_utf8_lossy(&info.multiaddr.0).into_owned(),
        public_key: info.public_key.0,
        stake: info.stake,
        committed_bytes: info.committed_bytes,
        settings: ProviderSettings {
            min_duration: u64::from(info.settings.min_duration),
            max_duration: u64::from(info.settings.max_duration),
            price_per_byte: info.settings.price_per_byte,
            accepting_primary: info.settings.accepting_primary,
            replica_sync_price: info.settings.replica_sync_price,
            accepting_extensions: info.settings.accepting_extensions,
            max_capacity: info.settings.max_capacity,
            min_agreement_bytes: info.settings.min_agreement_bytes,
        },
        deregister_at: info.deregister_at.map(u64::from),
    }
}
