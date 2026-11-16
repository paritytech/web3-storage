// SPDX-License-Identifier: Apache-2.0

//! The provider management operations.

use sp_core::crypto::{AccountId32, Ss58AddressFormat, Ss58Codec};
use sp_core::H256;
use storage_client::discovery::{ProviderInfo, ProviderInfoError};
use storage_client::{ProviderClient, ProviderSettings, ProviderSettingsUpdate, Signer};

/// Outcome of [`ProviderManager::update_settings`].
#[derive(Debug)]
pub struct SettingsUpdateOutcome {
    /// The settings now on chain: the current ones with the update applied.
    pub settings: ProviderSettings,
    /// The finalized block that includes the update, or `None` when the
    /// update changed nothing and no transaction was submitted.
    pub block_hash: Option<H256>,
}

/// Provider management operations over a connected [`ProviderClient`].
pub struct ProviderManager {
    client: ProviderClient,
    account: AccountId32,
    display_account: String,
}

impl ProviderManager {
    /// Operations over the connected `client`, signed by `signer`. The
    /// account is displayed with `ss58_prefix`.
    pub fn new(client: ProviderClient, signer: &Signer, ss58_prefix: u16) -> Self {
        let account = signer.account_id();
        let display_account =
            account.to_ss58check_with_version(Ss58AddressFormat::custom(ss58_prefix));
        Self {
            client,
            account,
            display_account,
        }
    }

    /// The signing account in the chain's SS58 encoding.
    pub fn account(&self) -> &str {
        &self.display_account
    }

    /// Register the signing account as a provider.
    ///
    /// Rejects an empty multiaddr, a zero stake, and an account that is
    /// already registered. The chain still enforces its minimum stake, its
    /// multiaddr length limit and the public key length.
    pub async fn register(
        &self,
        multiaddr: String,
        public_key: Vec<u8>,
        stake: u128,
    ) -> Result<H256, ProviderInfoError> {
        if self
            .client
            .get_provider_info(&self.account)
            .await?
            .is_some()
        {
            return Err(ProviderInfoError::AlreadyRegistered {
                account: self.display_account.clone(),
            });
        }
        let tx = self.client.register(multiaddr, public_key, stake).await?;
        Ok(tx.block_hash)
    }

    /// Apply `update` to the signing account's current settings and submit
    /// the result.
    ///
    /// Rejects an unregistered account, a provider with an announced
    /// deregistration, and merged settings the chain would reject (see
    /// [`ProviderSettings::validate`]). Submits nothing when the merged settings
    /// equal the current ones.
    pub async fn update_settings(
        &self,
        update: &ProviderSettingsUpdate,
    ) -> Result<SettingsUpdateOutcome, ProviderInfoError> {
        let provider = ProviderInfo::active_provider(
            &self.display_account,
            self.client.get_provider_info(&self.account).await?,
        )?;
        let min_stake_per_byte = self.client.min_stake_per_byte().await?;
        let settings = update.apply(&provider.settings());
        settings.validate(
            &provider.multiaddr,
            provider.committed_bytes,
            provider.stake,
            min_stake_per_byte,
        )?;

        if settings == provider.settings() {
            return Ok(SettingsUpdateOutcome {
                settings,
                block_hash: None,
            });
        }

        let tx = self.client.update_settings(settings.clone()).await?;
        Ok(SettingsUpdateOutcome {
            settings,
            block_hash: Some(tx.block_hash),
        })
    }

    /// Announce the signing account's deregistration.
    ///
    /// Rejects an unregistered account, a provider with an announced
    /// deregistration, and a provider with committed bytes. The pallet's
    /// `complete_deregister` releases the stake after the announcement
    /// period.
    pub async fn deregister(&self) -> Result<H256, ProviderInfoError> {
        let provider = ProviderInfo::active_provider(
            &self.display_account,
            self.client.get_provider_info(&self.account).await?,
        )?;
        provider.check_deregister(&self.display_account)?;
        let tx = self.client.deregister().await?;
        Ok(tx.block_hash)
    }
}
