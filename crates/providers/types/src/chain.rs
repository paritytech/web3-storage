// SPDX-License-Identifier: Apache-2.0

//! The error every chain call reports, shared by the provider crates and
//! the node.

use std::fmt;

/// Why a call against the chain failed.
///
/// This is the error type of `provider_replica::ReplicaSyncChainClient`
/// and of `provider-node`'s own subxt calls, so a chain failure keeps the
/// same shape whether it crosses a provider-crate trait or stays inside
/// the node. Naming no transport type keeps it usable from crates that do
/// not compile subxt.
#[derive(Debug, thiserror::Error)]
pub enum ChainClientError {
    /// A read against chain state (RPC call, storage fetch/iter, runtime API
    /// call) failed.
    #[error("Chain query failed ({what}): {reason}")]
    Query { what: &'static str, reason: String },

    /// A value read from the chain did not have the expected shape.
    #[error("Failed to decode chain state ({what}): {reason}")]
    Decode { what: &'static str, reason: String },

    /// The configured provider account could not be parsed.
    #[error("Invalid account {account}: {reason}")]
    InvalidAccount { account: String, reason: String },

    /// An extrinsic could not be submitted or its watch died before a
    /// verdict was seen; the transaction may or may not have landed, so this
    /// is safe to retry.
    #[error("Failed to submit {what}: {reason}")]
    TxSubmit { what: &'static str, reason: String },

    /// The chain rejected the extrinsic itself; resubmitting would fail
    /// identically.
    #[error("{what} rejected: {reason}")]
    TxRejected { what: &'static str, reason: String },
}

impl ChainClientError {
    /// A chain-state read failed. `what` names the read (e.g. `"current
    /// block"`); `e` is the underlying transport/RPC error, captured via its
    /// `Display` so this crate never names the chain client's own error type.
    pub fn query(what: &'static str, e: impl fmt::Display) -> Self {
        ChainClientError::Query {
            what,
            reason: e.to_string(),
        }
    }

    /// A value read from the chain did not decode into the expected shape.
    pub fn decode(what: &'static str, e: impl fmt::Display) -> Self {
        ChainClientError::Decode {
            what,
            reason: e.to_string(),
        }
    }

    /// The configured provider account could not be parsed.
    pub fn invalid_account(account: &str, e: impl fmt::Display) -> Self {
        ChainClientError::InvalidAccount {
            account: account.to_string(),
            reason: e.to_string(),
        }
    }

    /// An extrinsic submission failed in a way that may be safe to retry.
    pub fn tx_submit(what: &'static str, e: impl fmt::Display) -> Self {
        ChainClientError::TxSubmit {
            what,
            reason: e.to_string(),
        }
    }

    /// The chain rejected an extrinsic outright.
    pub fn tx_rejected(what: &'static str, e: impl fmt::Display) -> Self {
        ChainClientError::TxRejected {
            what,
            reason: e.to_string(),
        }
    }
}
