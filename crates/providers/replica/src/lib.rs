// SPDX-License-Identifier: Apache-2.0

//! Replica synchronization for provider nodes: the HTTP protocol replicas use
//! to pull data from primaries, and the background coordinator that drives it.

pub mod coordinator;
pub mod sync;
pub mod sync_roots;

pub use coordinator::{
    ReplicaSyncChainClient, ReplicaSyncCoordinator, ReplicaSyncCoordinatorConfig,
    ReplicaSyncCoordinatorHandle, SyncCommand, SyncCoordinatorStatus, SyncDuty, SyncResult,
};
pub use sync::ReplicaSync;
pub use sync_roots::{SignedSyncRoots, SyncRoots, SyncRootsSigner};

use std::fmt;
use storage_primitives::BucketId;
use thiserror::Error;

#[derive(Error, Debug)]
pub enum Error {
    /// The storage engine failed. Carried whole rather than mirrored variant
    /// by variant, so a new engine error needs no change here.
    #[error(transparent)]
    Backend(#[from] provider_storage::Error),

    #[error("Invalid hash: expected {expected}, got {actual}")]
    InvalidHash { expected: String, actual: String },

    /// This variant exists only for `provider-node`'s `From<Error> for
    /// Error` catch-all, which maps every node error it has no dedicated
    /// arm for onto this one via `Display`. Nothing in this crate should
    /// construct it directly.
    #[error("Node error: {0}")]
    Node(String),

    /// The chain connection itself is unavailable or failed to build.
    #[error(transparent)]
    Chain(#[from] provider_chain::Error),

    /// A read against chain state (RPC call, storage fetch/iter, runtime API
    /// call) failed.
    #[error("Chain query failed ({what}): {reason}")]
    ChainQuery { what: &'static str, reason: String },

    /// A value read from the chain or from a primary provider did not have
    /// the expected shape.
    #[error("Failed to decode {what}: {reason}")]
    Decode { what: &'static str, reason: String },

    /// An HTTP request to a primary provider failed at the transport level
    /// (connection refused, timed out, DNS failure).
    #[error("Request to primary failed ({what}): {reason}")]
    PrimaryRequest { what: &'static str, reason: String },

    /// A primary provider answered but with a non-success HTTP status.
    #[error("Primary returned error for {what}: status {status}")]
    PrimaryUnavailable { what: &'static str, status: u16 },

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

    /// A value read from the chain or from a primary provider did not
    /// decode into the expected shape.
    pub fn decode(what: &'static str, e: impl fmt::Display) -> Self {
        Error::Decode {
            what,
            reason: e.to_string(),
        }
    }

    /// A request to a primary provider failed at the transport level.
    pub fn primary_request(what: &'static str, e: impl fmt::Display) -> Self {
        Error::PrimaryRequest {
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn backend_variant_wraps_provider_storage_error_transparently() {
        let err: Error = provider_storage::Error::BucketNotFound(7).into();
        assert!(matches!(err, Error::Backend(_)));
        assert_eq!(err.to_string(), "Bucket not found: 7");
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
        assert_eq!(
            Error::primary_request("mmr peaks", "connection refused").to_string(),
            "Request to primary failed (mmr peaks): connection refused"
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
        assert_eq!(
            Error::PrimaryUnavailable {
                what: "mmr peaks",
                status: 404,
            }
            .to_string(),
            "Primary returned error for mmr peaks: status 404"
        );
        assert_eq!(
            Error::Node("cannot sign sync roots: bad key".to_string()).to_string(),
            "Node error: cannot sign sync roots: bad key"
        );
    }
}
