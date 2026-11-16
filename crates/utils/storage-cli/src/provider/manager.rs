// SPDX-License-Identifier: Apache-2.0

//! The provider management operations.

use sp_core::crypto::{AccountId32, Ss58AddressFormat, Ss58Codec};
use sp_core::H256;
use storage_client::discovery::ProviderInfo;
use storage_client::{
    ClientError, ProviderClient, ProviderSettings, ProviderSettingsUpdate, SettingsError, Signer,
};

/// Why a provider operation did not submit a transaction, or why the chain
/// rejected the one it submitted.
#[derive(Debug, thiserror::Error)]
pub enum ProviderError {
    /// An input value fails a check the chain would also reject it for.
    #[error("invalid {field}: {reason}")]
    Invalid {
        /// The input field.
        field: &'static str,
        /// What is wrong with it.
        reason: String,
    },

    /// The signing account is already a registered provider.
    #[error("account {account} is already registered as a provider")]
    AlreadyRegistered {
        /// The signing account.
        account: String,
    },

    /// The signing account is not a registered provider.
    #[error("account {account} is not a registered provider")]
    NotRegistered {
        /// The signing account.
        account: String,
    },

    /// The provider has announced its deregistration. The pallet rejects
    /// settings updates and a second announcement until it is cancelled or
    /// completed.
    #[error(
        "provider {account} has announced deregistration, completable at anchor block {deregister_at}"
    )]
    DeregisterAnnounced {
        /// The signing account.
        account: String,
        /// Anchor block at which `complete_deregister` becomes callable.
        deregister_at: u32,
    },

    /// The provider still has committed bytes. The pallet rejects a
    /// deregistration until every agreement has ended.
    #[error(
        "provider {account} still has {committed_bytes} committed bytes; its agreements must end before it can deregister"
    )]
    HasActiveAgreements {
        /// The signing account.
        account: String,
        /// Bytes committed across live agreements.
        committed_bytes: u64,
    },

    /// The merged settings fail one of the pallet's settings checks.
    #[error("invalid settings: {0}")]
    Settings(#[from] SettingsError),

    /// A chain read or write failed, or the chain rejected the transaction.
    #[error(transparent)]
    Chain(#[from] ClientError),
}

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
    ) -> Result<H256, ProviderError> {
        check_registration(&multiaddr, stake)?;
        if self.provider().await?.is_some() {
            return Err(ProviderError::AlreadyRegistered {
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
    ) -> Result<SettingsUpdateOutcome, ProviderError> {
        let provider = active_provider(&self.display_account, self.provider().await?)?;
        let min_stake_per_byte = self.client.min_stake_per_byte().await?;
        let settings = merged_settings(&provider, update, min_stake_per_byte)?;

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
    pub async fn deregister(&self) -> Result<H256, ProviderError> {
        let provider = active_provider(&self.display_account, self.provider().await?)?;
        check_deregister(&self.display_account, &provider)?;
        let tx = self.client.deregister().await?;
        Ok(tx.block_hash)
    }

    /// The signing account's provider record, or `None` when it is not
    /// registered.
    async fn provider(&self) -> Result<Option<ProviderInfo>, ProviderError> {
        Ok(self.client.get_provider_info(&self.account).await?)
    }
}

/// The input checks of a registration, made before any chain read.
fn check_registration(multiaddr: &str, stake: u128) -> Result<(), ProviderError> {
    if multiaddr.is_empty() {
        return Err(ProviderError::Invalid {
            field: "multiaddr",
            reason: "must not be empty".to_string(),
        });
    }
    if stake == 0 {
        return Err(ProviderError::Invalid {
            field: "stake",
            reason: "must be greater than zero".to_string(),
        });
    }
    Ok(())
}

/// The record of a registered provider with no announced deregistration.
fn active_provider(
    account: &str,
    provider: Option<ProviderInfo>,
) -> Result<ProviderInfo, ProviderError> {
    let provider = provider.ok_or_else(|| ProviderError::NotRegistered {
        account: account.to_string(),
    })?;
    match provider.deregister_at {
        Some(deregister_at) => Err(ProviderError::DeregisterAnnounced {
            account: account.to_string(),
            deregister_at,
        }),
        None => Ok(provider),
    }
}

/// `update` applied to the provider's settings, checked like the pallet
/// checks them.
fn merged_settings(
    provider: &ProviderInfo,
    update: &ProviderSettingsUpdate,
    min_stake_per_byte: u128,
) -> Result<ProviderSettings, ProviderError> {
    let settings = update.apply(&provider.settings());
    settings.validate(provider.committed_bytes, provider.stake, min_stake_per_byte)?;
    Ok(settings)
}

/// A provider can announce deregistration only with no committed bytes.
fn check_deregister(account: &str, provider: &ProviderInfo) -> Result<(), ProviderError> {
    if provider.committed_bytes > 0 {
        return Err(ProviderError::HasActiveAgreements {
            account: account.to_string(),
            committed_bytes: provider.committed_bytes,
        });
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// `MinStakePerByte` of the tests. A stake of 1_000_000 backs capacities
    /// up to 10_000 bytes.
    const MIN_STAKE_PER_BYTE: u128 = 100;

    fn provider() -> ProviderInfo {
        ProviderInfo {
            multiaddr: "/ip4/127.0.0.1/tcp/3333".to_string(),
            public_key: vec![1; 32],
            stake: 1_000,
            min_duration: 10,
            max_duration: 100,
            price_per_byte: 5,
            accepting_primary: true,
            accepting_extensions: true,
            ..Default::default()
        }
    }

    #[test]
    fn registration_accepts_valid_input() {
        check_registration("/ip4/127.0.0.1/tcp/3333", 1_000).unwrap();
        check_registration("/ip4/127.0.0.1/tcp/3333", 1).unwrap();
    }

    #[test]
    fn registration_rejects_invalid_input() {
        let cases = [
            ("", 1_000, "multiaddr"),
            ("/ip4/127.0.0.1/tcp/3333", 0, "stake"),
        ];
        for (multiaddr, stake, expected_field) in cases {
            let err = check_registration(multiaddr, stake).unwrap_err();
            assert!(
                matches!(err, ProviderError::Invalid { field, .. } if field == expected_field),
                "{err}"
            );
        }
    }

    #[test]
    fn active_provider_requires_a_registered_provider() {
        let record = active_provider("5Abc", Some(provider())).unwrap();
        assert_eq!(record.stake, 1_000);

        let err = active_provider("5Abc", None).unwrap_err();
        assert!(
            matches!(&err, ProviderError::NotRegistered { account } if account == "5Abc"),
            "{err}"
        );
    }

    #[test]
    fn active_provider_rejects_an_announced_deregistration() {
        let err = active_provider(
            "5Abc",
            Some(ProviderInfo {
                deregister_at: Some(500),
                ..provider()
            }),
        )
        .unwrap_err();
        assert!(
            matches!(
                err,
                ProviderError::DeregisterAnnounced {
                    deregister_at: 500,
                    ..
                }
            ),
            "{err}"
        );
    }

    #[test]
    fn merged_settings_applies_only_the_set_fields() {
        let update = ProviderSettingsUpdate {
            price_per_byte: Some(9),
            replica_sync_price: Some(Some(3)),
            ..Default::default()
        };
        let merged = merged_settings(&provider(), &update, MIN_STAKE_PER_BYTE).unwrap();
        assert_eq!(
            merged,
            ProviderSettings {
                price_per_byte: 9,
                replica_sync_price: Some(3),
                ..provider().settings()
            }
        );

        let unchanged = merged_settings(
            &provider(),
            &ProviderSettingsUpdate::default(),
            MIN_STAKE_PER_BYTE,
        )
        .unwrap();
        assert_eq!(unchanged, provider().settings());
    }

    /// The merged settings go through [`ProviderSettings::validate`] with the
    /// provider's committed bytes and stake and the chain's
    /// `MinStakePerByte`.
    #[test]
    fn merged_settings_rejects_what_the_pallet_rejects() {
        let record = ProviderInfo {
            committed_bytes: 4_096,
            stake: 1_000_000,
            ..provider()
        };
        let cases = [
            (
                ProviderSettingsUpdate {
                    min_duration: Some(101),
                    ..Default::default()
                },
                SettingsError::MinDurationExceedsMaxDuration {
                    min_duration: 101,
                    max_duration: 100,
                },
            ),
            (
                ProviderSettingsUpdate {
                    max_capacity: Some(1_024),
                    ..Default::default()
                },
                SettingsError::CapacityBelowCommitted {
                    max_capacity: 1_024,
                    committed_bytes: 4_096,
                },
            ),
            (
                ProviderSettingsUpdate {
                    max_capacity: Some(8_192),
                    min_agreement_bytes: Some(10_000),
                    ..Default::default()
                },
                SettingsError::MinAgreementBytesExceedsMaxCapacity {
                    min_agreement_bytes: 10_000,
                    max_capacity: 8_192,
                },
            ),
            (
                ProviderSettingsUpdate {
                    max_capacity: Some(10_001),
                    ..Default::default()
                },
                SettingsError::InsufficientStakeForCapacity {
                    stake: 1_000_000,
                    max_capacity: 10_001,
                    min_stake_per_byte: MIN_STAKE_PER_BYTE,
                    required_stake: 1_000_100,
                },
            ),
        ];
        for (update, expected) in cases {
            let err = merged_settings(&record, &update, MIN_STAKE_PER_BYTE).unwrap_err();
            assert!(
                matches!(&err, ProviderError::Settings(e) if *e == expected),
                "{err}"
            );
        }

        // The largest capacity the stake backs is accepted; unlimited
        // capacity skips the capacity checks.
        for update in [
            ProviderSettingsUpdate {
                max_capacity: Some(10_000),
                ..Default::default()
            },
            ProviderSettingsUpdate {
                max_capacity: Some(0),
                min_agreement_bytes: Some(10_000),
                ..Default::default()
            },
        ] {
            merged_settings(&record, &update, MIN_STAKE_PER_BYTE).unwrap();
        }
    }

    #[test]
    fn deregister_requires_no_committed_bytes() {
        check_deregister("5Abc", &provider()).unwrap();

        let err = check_deregister(
            "5Abc",
            &ProviderInfo {
                committed_bytes: 1,
                ..provider()
            },
        )
        .unwrap_err();
        assert!(
            matches!(
                err,
                ProviderError::HasActiveAgreements {
                    committed_bytes: 1,
                    ..
                }
            ),
            "{err}"
        );
    }
}
