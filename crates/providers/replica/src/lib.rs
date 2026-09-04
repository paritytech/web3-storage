// SPDX-License-Identifier: Apache-2.0

//! Replica synchronization for provider nodes: the HTTP protocol replicas use
//! to pull data from primaries, and the background coordinator that drives it.

pub mod coordinator;
pub mod sync;

pub use coordinator::{
    ReplicaSyncChainClient, ReplicaSyncCoordinator, ReplicaSyncCoordinatorConfig,
    ReplicaSyncCoordinatorHandle, SyncCommand, SyncCoordinatorStatus, SyncDuty, SyncResult,
};
pub use sync::ReplicaSync;

use std::fmt;
use storage_primitives::BucketId;
use thiserror::Error;

#[derive(Error, Debug)]
pub enum Error {
    #[error("Node not found: {0}")]
    NodeNotFound(String),

    #[error("Children missing: {0:?}")]
    ChildrenMissing(Vec<String>),

    #[error("Quota exceeded: used {used}, max {max}")]
    QuotaExceeded { used: u64, max: u64 },

    #[error("Bucket not found: {0}")]
    BucketNotFound(u64),

    #[error("Root not found: {0}")]
    RootNotFound(String),

    #[error("Invalid hash: expected {expected}, got {actual}")]
    InvalidHash { expected: String, actual: String },

    #[error("Storage error: {0}")]
    Storage(String),

    #[error("Serialization error: {0}")]
    Serialization(String),

    #[error("Internal error: {0}")]
    Internal(String),

    /// The chain connection itself is unavailable or failed to build.
    #[error(transparent)]
    Chain(#[from] provider_chain::Error),

    /// A read against chain state (RPC call, storage fetch/iter, runtime API
    /// call) failed.
    #[error("Chain query failed ({what}): {reason}")]
    ChainQuery { what: &'static str, reason: String },

    /// A value came back from the chain but did not have the expected shape.
    #[error("Failed to decode {what}: {reason}")]
    Decode { what: &'static str, reason: String },

    /// An extrinsic could not be submitted or its watch died before a
    /// verdict was seen; the transaction may or may not have landed, so this
    /// is safe to retry.
    #[error("Failed to submit {what}: {reason}")]
    TxSubmit { what: &'static str, reason: String },

    /// The chain rejected the extrinsic itself; resubmitting would fail
    /// identically.
    #[error("{what} rejected: {reason}")]
    TxRejected { what: &'static str, reason: String },

    /// The configured provider account could not be parsed.
    #[error("Invalid account {account}: {reason}")]
    InvalidAccount { account: String, reason: String },

    /// The agreement at this bucket is a primary agreement, not a replica
    /// one; not a failure, but a filter result the caller must be able to
    /// tell apart from a real decode error.
    #[error("Bucket {0} does not hold a replica agreement")]
    NotReplicaAgreement(BucketId),

    /// A coordinator control or status channel was dropped.
    #[error("Coordinator channel closed")]
    ChannelClosed,
}

impl Error {
    /// A chain-state read failed. `what` names the read (e.g. `"current
    /// block"`); `e` is the underlying transport/RPC error, captured via its
    /// `Display` so callers never need to name the chain client's own error
    /// type.
    pub fn chain_query(what: &'static str, e: impl fmt::Display) -> Self {
        Error::ChainQuery {
            what,
            reason: e.to_string(),
        }
    }

    /// A value read from the chain did not decode into the expected shape.
    pub fn decode(what: &'static str, e: impl fmt::Display) -> Self {
        Error::Decode {
            what,
            reason: e.to_string(),
        }
    }

    /// An extrinsic submission failed in a way that may be safe to retry.
    pub fn tx_submit(what: &'static str, e: impl fmt::Display) -> Self {
        Error::TxSubmit {
            what,
            reason: e.to_string(),
        }
    }

    /// The chain rejected an extrinsic outright.
    pub fn tx_rejected(what: &'static str, e: impl fmt::Display) -> Self {
        Error::TxRejected {
            what,
            reason: e.to_string(),
        }
    }
}

/// Map storage-engine errors onto this crate's error space one-to-one, the
/// same mapping provider-node's own `Error` uses.
impl From<provider_storage::Error> for Error {
    fn from(e: provider_storage::Error) -> Self {
        use provider_storage::Error as StorageError;
        match e {
            StorageError::NodeNotFound(hash) => Error::NodeNotFound(hash),
            StorageError::ChildrenMissing(children) => Error::ChildrenMissing(children),
            StorageError::QuotaExceeded { used, max } => Error::QuotaExceeded { used, max },
            StorageError::BucketNotFound(id) => Error::BucketNotFound(id),
            StorageError::RootNotFound(root) => Error::RootNotFound(root),
            StorageError::InvalidHash { expected, actual } => {
                Error::InvalidHash { expected, actual }
            }
            StorageError::Storage(msg) => Error::Storage(msg),
            StorageError::Serialization(msg) => Error::Serialization(msg),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn from_provider_storage_error_maps_one_to_one() {
        use provider_storage::Error as StorageError;

        let cases: Vec<(StorageError, &str)> = vec![
            (StorageError::NodeNotFound("h".into()), "Node not found: h"),
            (
                StorageError::ChildrenMissing(vec!["a".into(), "b".into()]),
                "Children missing: [\"a\", \"b\"]",
            ),
            (
                StorageError::QuotaExceeded { used: 1, max: 2 },
                "Quota exceeded: used 1, max 2",
            ),
            (StorageError::BucketNotFound(7), "Bucket not found: 7"),
            (StorageError::RootNotFound("r".into()), "Root not found: r"),
            (
                StorageError::InvalidHash {
                    expected: "e".into(),
                    actual: "a".into(),
                },
                "Invalid hash: expected e, got a",
            ),
            (StorageError::Storage("s".into()), "Storage error: s"),
            (
                StorageError::Serialization("z".into()),
                "Serialization error: z",
            ),
        ];

        for (storage_err, expected_message) in cases {
            let mapped: Error = storage_err.into();
            assert_eq!(mapped.to_string(), expected_message);
        }
    }

    #[test]
    fn chain_variant_wraps_provider_chain_error_transparently() {
        let err: Error = provider_chain::Error::NotConnected.into();
        assert_eq!(err.to_string(), "Chain connection not established yet");
    }

    #[test]
    fn constructors_produce_expected_messages() {
        assert_eq!(
            Error::chain_query("current block", "timed out").to_string(),
            "Chain query failed (current block): timed out"
        );
        assert_eq!(
            Error::decode("bucket", "unexpected shape").to_string(),
            "Failed to decode bucket: unexpected shape"
        );
        assert_eq!(
            Error::tx_submit("confirm_replica_sync", "watch dropped").to_string(),
            "Failed to submit confirm_replica_sync: watch dropped"
        );
        assert_eq!(
            Error::tx_rejected("confirm_replica_sync", "SyncTooFrequent").to_string(),
            "confirm_replica_sync rejected: SyncTooFrequent"
        );
    }

    #[test]
    fn remaining_variants_produce_expected_messages() {
        assert_eq!(
            Error::InvalidAccount {
                account: "0xzz".into(),
                reason: "odd length hex string".into(),
            }
            .to_string(),
            "Invalid account 0xzz: odd length hex string"
        );
        assert_eq!(
            Error::NotReplicaAgreement(7).to_string(),
            "Bucket 7 does not hold a replica agreement"
        );
        assert_eq!(
            Error::ChannelClosed.to_string(),
            "Coordinator channel closed"
        );
    }
}
