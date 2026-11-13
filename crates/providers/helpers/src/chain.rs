// SPDX-License-Identifier: Apache-2.0

//! The chain boundary of the helper operations.

use crate::types::{ProviderRecord, ProviderRegistration, ProviderSettings, TxReceipt};

/// A failed chain read or write, reported by a [`ProviderChain`]
/// implementation. `what` names the read or the transaction; `reason` is
/// the underlying error's message, so this crate never names a chain
/// client's error type.
#[derive(Debug, thiserror::Error)]
pub enum ChainError {
    /// A read of chain state failed: connecting, fetching, or decoding.
    #[error("chain read failed ({what}): {reason}")]
    Read {
        /// The read that failed.
        what: &'static str,
        /// The underlying error.
        reason: String,
    },
    /// A transaction could not be submitted, or its outcome was not seen.
    #[error("failed to submit {what}: {reason}")]
    Submit {
        /// The transaction.
        what: &'static str,
        /// The underlying error.
        reason: String,
    },
    /// The chain rejected the transaction.
    #[error("{what} rejected: {reason}")]
    Rejected {
        /// The transaction.
        what: &'static str,
        /// The chain's reason.
        reason: String,
    },
}

/// Chain access for the provider management operations.
///
/// An implementation owns one signing account: every write is signed by it
/// and every read is about it. It encodes the plain types in
/// [`crate::types`] for its chain and waits for each transaction to be
/// final before returning a [`TxReceipt`].
#[async_trait::async_trait]
pub trait ProviderChain: Send + Sync {
    /// The signing account in the chain's display encoding (SS58 on
    /// Substrate chains). Used in messages only.
    fn account(&self) -> String;

    /// The signing account's provider record, or `None` when it is not
    /// registered.
    async fn fetch_provider(&self) -> Result<Option<ProviderRecord>, ChainError>;

    /// The chain's minimum stake per byte of declared capacity
    /// (`MinStakePerByte`), in the chain's smallest unit.
    async fn min_stake_per_byte(&self) -> Result<u128, ChainError>;

    /// Submit a registration for the signing account.
    async fn register_provider(
        &self,
        registration: &ProviderRegistration,
    ) -> Result<TxReceipt, ChainError>;

    /// Replace the signing account's provider settings.
    async fn update_provider_settings(
        &self,
        settings: &ProviderSettings,
    ) -> Result<TxReceipt, ChainError>;

    /// Announce the signing account's deregistration.
    async fn deregister_provider(&self) -> Result<TxReceipt, ChainError>;
}
