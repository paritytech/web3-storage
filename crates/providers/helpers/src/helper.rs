// SPDX-License-Identifier: Apache-2.0

//! The provider management operations.

use crate::chain::{ChainError, ProviderChain};
use crate::types::{
    ProviderRecord, ProviderRegistration, ProviderSettings, ProviderSettingsUpdate, SettingsError,
    TxReceipt,
};

/// Public key lengths the pallet accepts: 32 bytes for sr25519 and
/// ed25519, 33 for compressed ecdsa.
const ACCEPTED_PUBLIC_KEY_LENGTHS: [usize; 2] = [32, 33];

/// Why a helper operation did not submit a transaction, or why the chain
/// rejected the one it submitted.
#[derive(Debug, thiserror::Error)]
pub enum HelperError {
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
        deregister_at: u64,
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

    /// A chain read or write failed.
    #[error(transparent)]
    Chain(#[from] ChainError),
}

/// Outcome of [`ProviderHelper::update_settings`].
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SettingsUpdateOutcome {
    /// The settings now on chain: the current ones with the update applied.
    pub settings: ProviderSettings,
    /// The transaction receipt, or `None` when the update changed nothing
    /// and no transaction was submitted.
    pub receipt: Option<TxReceipt>,
}

/// Provider management operations over a [`ProviderChain`].
pub struct ProviderHelper<C> {
    chain: C,
}

impl<C: ProviderChain> ProviderHelper<C> {
    /// Operations signed by `chain`'s account.
    pub fn new(chain: C) -> Self {
        Self { chain }
    }

    /// The signing account in the chain's display encoding.
    pub fn account(&self) -> String {
        self.chain.account()
    }

    /// The signing account's provider record, or `None` when it is not
    /// registered.
    pub async fn provider(&self) -> Result<Option<ProviderRecord>, HelperError> {
        Ok(self.chain.fetch_provider().await?)
    }

    /// Register the signing account as a provider.
    ///
    /// Rejects an empty multiaddr, a public key of a length no scheme uses,
    /// a zero stake, and an account that is already registered. The chain
    /// still enforces its minimum stake and its multiaddr length limit.
    pub async fn register(
        &self,
        registration: &ProviderRegistration,
    ) -> Result<TxReceipt, HelperError> {
        if registration.multiaddr.is_empty() {
            return Err(HelperError::Invalid {
                field: "multiaddr",
                reason: "must not be empty".to_string(),
            });
        }
        if !ACCEPTED_PUBLIC_KEY_LENGTHS.contains(&registration.public_key.len()) {
            return Err(HelperError::Invalid {
                field: "public_key",
                reason: format!(
                    "{} bytes; expected 32 (sr25519, ed25519) or 33 (ecdsa)",
                    registration.public_key.len()
                ),
            });
        }
        if registration.stake == 0 {
            return Err(HelperError::Invalid {
                field: "stake",
                reason: "must be greater than zero".to_string(),
            });
        }
        if self.chain.fetch_provider().await?.is_some() {
            return Err(HelperError::AlreadyRegistered {
                account: self.chain.account(),
            });
        }
        Ok(self.chain.register_provider(registration).await?)
    }

    /// Apply `update` to the signing account's current settings and submit
    /// the result.
    ///
    /// Rejects an unregistered account, a provider with an announced
    /// deregistration, and merged settings the chain would reject (see
    /// [`ProviderSettings::validate`]). Submits nothing when the merged
    /// settings equal the current ones.
    pub async fn update_settings(
        &self,
        update: &ProviderSettingsUpdate,
    ) -> Result<SettingsUpdateOutcome, HelperError> {
        let provider = self.registered_provider().await?;
        self.ensure_no_deregister_announced(&provider)?;

        let settings = update.apply(&provider.settings);
        let min_stake_per_byte = self.chain.min_stake_per_byte().await?;
        settings.validate(provider.committed_bytes, provider.stake, min_stake_per_byte)?;

        if settings == provider.settings {
            return Ok(SettingsUpdateOutcome {
                settings,
                receipt: None,
            });
        }

        let receipt = self.chain.update_provider_settings(&settings).await?;
        Ok(SettingsUpdateOutcome {
            settings,
            receipt: Some(receipt),
        })
    }

    /// Announce the signing account's deregistration.
    ///
    /// Rejects an unregistered account, a provider with an announced
    /// deregistration, and a provider with committed bytes. The pallet's
    /// `complete_deregister` releases the stake after the announcement
    /// period.
    pub async fn deregister(&self) -> Result<TxReceipt, HelperError> {
        let provider = self.registered_provider().await?;
        self.ensure_no_deregister_announced(&provider)?;
        if provider.committed_bytes > 0 {
            return Err(HelperError::HasActiveAgreements {
                account: self.chain.account(),
                committed_bytes: provider.committed_bytes,
            });
        }
        Ok(self.chain.deregister_provider().await?)
    }

    /// The signing account's record, or [`HelperError::NotRegistered`].
    async fn registered_provider(&self) -> Result<ProviderRecord, HelperError> {
        self.chain
            .fetch_provider()
            .await?
            .ok_or_else(|| HelperError::NotRegistered {
                account: self.chain.account(),
            })
    }

    fn ensure_no_deregister_announced(&self, provider: &ProviderRecord) -> Result<(), HelperError> {
        match provider.deregister_at {
            Some(deregister_at) => Err(HelperError::DeregisterAnnounced {
                account: self.chain.account(),
                deregister_at,
            }),
            None => Ok(()),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;

    /// What the mock chain recorded being submitted.
    #[derive(Debug, PartialEq, Eq)]
    enum Submitted {
        Register(ProviderRegistration),
        UpdateSettings(ProviderSettings),
        Deregister,
    }

    /// `MinStakePerByte` of the mock chain. A stake of 1_000_000 backs
    /// capacities up to 10_000 bytes.
    const MIN_STAKE_PER_BYTE: u128 = 100;

    struct MockChain {
        provider: Option<ProviderRecord>,
        submitted: Mutex<Vec<Submitted>>,
    }

    impl MockChain {
        fn new(provider: Option<ProviderRecord>) -> Self {
            Self {
                provider,
                submitted: Mutex::new(Vec::new()),
            }
        }
    }

    fn receipt() -> TxReceipt {
        TxReceipt {
            block_hash: Some("0x01".to_string()),
        }
    }

    #[async_trait::async_trait]
    impl ProviderChain for MockChain {
        fn account(&self) -> String {
            "alice".to_string()
        }

        async fn fetch_provider(&self) -> Result<Option<ProviderRecord>, ChainError> {
            Ok(self.provider.clone())
        }

        async fn min_stake_per_byte(&self) -> Result<u128, ChainError> {
            Ok(MIN_STAKE_PER_BYTE)
        }

        async fn register_provider(
            &self,
            registration: &ProviderRegistration,
        ) -> Result<TxReceipt, ChainError> {
            self.submitted
                .lock()
                .unwrap()
                .push(Submitted::Register(registration.clone()));
            Ok(receipt())
        }

        async fn update_provider_settings(
            &self,
            settings: &ProviderSettings,
        ) -> Result<TxReceipt, ChainError> {
            self.submitted
                .lock()
                .unwrap()
                .push(Submitted::UpdateSettings(settings.clone()));
            Ok(receipt())
        }

        async fn deregister_provider(&self) -> Result<TxReceipt, ChainError> {
            self.submitted.lock().unwrap().push(Submitted::Deregister);
            Ok(receipt())
        }
    }

    fn settings() -> ProviderSettings {
        ProviderSettings {
            min_duration: 10,
            max_duration: 100,
            price_per_byte: 5,
            accepting_primary: true,
            replica_sync_price: None,
            accepting_extensions: true,
            max_capacity: 0,
            min_agreement_bytes: 0,
        }
    }

    fn record() -> ProviderRecord {
        ProviderRecord {
            multiaddr: "/ip4/127.0.0.1/tcp/3333".to_string(),
            public_key: vec![1; 32],
            stake: 1_000,
            committed_bytes: 0,
            settings: settings(),
            deregister_at: None,
        }
    }

    fn registration() -> ProviderRegistration {
        ProviderRegistration {
            multiaddr: "/ip4/127.0.0.1/tcp/3333".to_string(),
            public_key: vec![1; 32],
            stake: 1_000,
        }
    }

    fn submitted(helper: &ProviderHelper<MockChain>) -> Vec<Submitted> {
        std::mem::take(&mut *helper.chain.submitted.lock().unwrap())
    }

    #[tokio::test]
    async fn register_submits_for_unregistered_account() {
        let helper = ProviderHelper::new(MockChain::new(None));
        let got = helper.register(&registration()).await.unwrap();
        assert_eq!(got, receipt());
        assert_eq!(
            submitted(&helper),
            vec![Submitted::Register(registration())]
        );
    }

    #[tokio::test]
    async fn register_rejects_registered_account() {
        let helper = ProviderHelper::new(MockChain::new(Some(record())));
        let err = helper.register(&registration()).await.unwrap_err();
        assert!(
            matches!(err, HelperError::AlreadyRegistered { .. }),
            "{err}"
        );
        assert!(submitted(&helper).is_empty());
    }

    #[tokio::test]
    async fn register_rejects_invalid_input_before_reading_chain() {
        let helper = ProviderHelper::new(MockChain::new(None));
        let cases = [
            (
                ProviderRegistration {
                    multiaddr: String::new(),
                    ..registration()
                },
                "multiaddr",
            ),
            (
                ProviderRegistration {
                    public_key: vec![1; 20],
                    ..registration()
                },
                "public_key",
            ),
            (
                ProviderRegistration {
                    public_key: vec![1; 64],
                    ..registration()
                },
                "public_key",
            ),
            (
                ProviderRegistration {
                    stake: 0,
                    ..registration()
                },
                "stake",
            ),
        ];
        for (input, expected_field) in cases {
            let err = helper.register(&input).await.unwrap_err();
            assert!(
                matches!(err, HelperError::Invalid { field, .. } if field == expected_field),
                "{err}"
            );
        }
        assert!(submitted(&helper).is_empty());
    }

    #[tokio::test]
    async fn update_settings_merges_and_submits() {
        let helper = ProviderHelper::new(MockChain::new(Some(record())));
        let update = ProviderSettingsUpdate {
            price_per_byte: Some(9),
            replica_sync_price: Some(Some(3)),
            ..Default::default()
        };
        let got = helper.update_settings(&update).await.unwrap();
        let expected = ProviderSettings {
            price_per_byte: 9,
            replica_sync_price: Some(3),
            ..settings()
        };
        assert_eq!(got.settings, expected);
        assert_eq!(got.receipt, Some(receipt()));
        assert_eq!(
            submitted(&helper),
            vec![Submitted::UpdateSettings(expected)]
        );
    }

    #[tokio::test]
    async fn update_settings_skips_no_op() {
        let helper = ProviderHelper::new(MockChain::new(Some(record())));
        let update = ProviderSettingsUpdate {
            price_per_byte: Some(settings().price_per_byte),
            ..Default::default()
        };
        let got = helper.update_settings(&update).await.unwrap();
        assert_eq!(got.settings, settings());
        assert_eq!(got.receipt, None);
        assert!(submitted(&helper).is_empty());
    }

    #[tokio::test]
    async fn update_settings_rejects_unregistered_and_deregistering() {
        let helper = ProviderHelper::new(MockChain::new(None));
        let err = helper
            .update_settings(&ProviderSettingsUpdate::default())
            .await
            .unwrap_err();
        assert!(matches!(err, HelperError::NotRegistered { .. }), "{err}");

        let helper = ProviderHelper::new(MockChain::new(Some(ProviderRecord {
            deregister_at: Some(500),
            ..record()
        })));
        let err = helper
            .update_settings(&ProviderSettingsUpdate::default())
            .await
            .unwrap_err();
        assert!(
            matches!(
                err,
                HelperError::DeregisterAnnounced {
                    deregister_at: 500,
                    ..
                }
            ),
            "{err}"
        );
        assert!(submitted(&helper).is_empty());
    }

    /// The merged settings go through [`ProviderSettings::validate`] with
    /// the provider's committed bytes and stake and the chain's
    /// `MinStakePerByte`; a failure submits nothing.
    #[tokio::test]
    async fn update_settings_rejects_what_the_pallet_rejects() {
        let helper = ProviderHelper::new(MockChain::new(Some(ProviderRecord {
            committed_bytes: 4_096,
            stake: 1_000_000,
            ..record()
        })));
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
            let err = helper.update_settings(&update).await.unwrap_err();
            assert!(
                matches!(&err, HelperError::Settings(e) if *e == expected),
                "{err}"
            );
        }
        assert!(submitted(&helper).is_empty());

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
            helper.update_settings(&update).await.unwrap();
        }
        assert_eq!(submitted(&helper).len(), 2);
    }

    #[tokio::test]
    async fn deregister_submits_for_idle_provider() {
        let helper = ProviderHelper::new(MockChain::new(Some(record())));
        assert_eq!(helper.deregister().await.unwrap(), receipt());
        assert_eq!(submitted(&helper), vec![Submitted::Deregister]);
    }

    #[tokio::test]
    async fn deregister_rejects_unregistered_announced_and_committed() {
        let helper = ProviderHelper::new(MockChain::new(None));
        let err = helper.deregister().await.unwrap_err();
        assert!(matches!(err, HelperError::NotRegistered { .. }), "{err}");

        let helper = ProviderHelper::new(MockChain::new(Some(ProviderRecord {
            deregister_at: Some(500),
            ..record()
        })));
        let err = helper.deregister().await.unwrap_err();
        assert!(
            matches!(err, HelperError::DeregisterAnnounced { .. }),
            "{err}"
        );

        let helper = ProviderHelper::new(MockChain::new(Some(ProviderRecord {
            committed_bytes: 1,
            ..record()
        })));
        let err = helper.deregister().await.unwrap_err();
        assert!(
            matches!(
                err,
                HelperError::HasActiveAgreements {
                    committed_bytes: 1,
                    ..
                }
            ),
            "{err}"
        );
        assert!(submitted(&helper).is_empty());
    }

    #[tokio::test]
    async fn chain_errors_propagate() {
        struct FailingChain;

        #[async_trait::async_trait]
        impl ProviderChain for FailingChain {
            fn account(&self) -> String {
                "alice".to_string()
            }
            async fn fetch_provider(&self) -> Result<Option<ProviderRecord>, ChainError> {
                Err(ChainError::Read {
                    what: "provider",
                    reason: "connection refused".to_string(),
                })
            }
            async fn min_stake_per_byte(&self) -> Result<u128, ChainError> {
                unreachable!()
            }
            async fn register_provider(
                &self,
                _: &ProviderRegistration,
            ) -> Result<TxReceipt, ChainError> {
                unreachable!()
            }
            async fn update_provider_settings(
                &self,
                _: &ProviderSettings,
            ) -> Result<TxReceipt, ChainError> {
                unreachable!()
            }
            async fn deregister_provider(&self) -> Result<TxReceipt, ChainError> {
                unreachable!()
            }
        }

        let helper = ProviderHelper::new(FailingChain);
        let err = helper.register(&registration()).await.unwrap_err();
        assert!(matches!(err, HelperError::Chain(_)), "{err}");
    }
}
