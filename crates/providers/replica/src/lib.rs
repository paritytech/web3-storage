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
pub use provider_types::ChainClientError;
pub use sync::ReplicaSync;
pub use sync_roots::{SignedSyncRoots, SyncRoots, SyncRootsSigner};

use std::fmt;
use thiserror::Error;

#[derive(Error, Debug)]
pub enum Error {
    /// The storage engine failed. Carried whole rather than mirrored variant
    /// by variant, so a new engine error needs no change here.
    #[error(transparent)]
    Backend(#[from] provider_storage::Error),

    /// A value read from a primary provider did not have the expected shape.
    #[error("Failed to decode {what}: {reason}")]
    Decode { what: &'static str, reason: String },

    /// An HTTP request to a primary provider failed at the transport level
    /// (connection refused, timed out, DNS failure).
    #[error("Request to primary failed ({what}): {reason}")]
    PrimaryRequest { what: &'static str, reason: String },

    /// A primary provider answered but with a non-success HTTP status.
    #[error("Primary returned error for {what}: status {status}")]
    PrimaryUnavailable { what: &'static str, status: u16 },

    /// A call through [`ReplicaSyncChainClient`] failed. Its variants are
    /// defined with that trait; this crate only reports them.
    #[error(transparent)]
    ChainClient(#[from] ChainClientError),

    /// A coordinator control or status channel was dropped.
    #[error("Coordinator channel closed")]
    ChannelClosed,
}

impl Error {
    /// A value read from a primary provider did not decode into the expected
    /// shape.
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
    fn chain_client_variant_wraps_the_trait_error_transparently() {
        let err: Error = ChainClientError::query("current block", "timed out").into();
        assert!(matches!(err, Error::ChainClient(_)));
        assert_eq!(
            err.to_string(),
            "Chain query failed (current block): timed out"
        );
    }

    /// Every [`ChainClientError`] constructor, including the two whose only
    /// production callers live in `provider-node`'s subxt client.
    #[test]
    fn chain_client_constructors_produce_expected_messages() {
        assert_eq!(
            ChainClientError::query("current block", "timed out").to_string(),
            "Chain query failed (current block): timed out"
        );
        assert_eq!(
            ChainClientError::decode("bucket", "unexpected shape").to_string(),
            "Failed to decode chain state (bucket): unexpected shape"
        );
        assert_eq!(
            ChainClientError::invalid_account("0xzz", "odd length hex string").to_string(),
            "Invalid account 0xzz: odd length hex string"
        );
        assert_eq!(
            ChainClientError::tx_submit("confirm_replica_sync", "watch dropped").to_string(),
            "Failed to submit confirm_replica_sync: watch dropped"
        );
        assert_eq!(
            ChainClientError::tx_rejected("confirm_replica_sync", "SyncTooFrequent").to_string(),
            "confirm_replica_sync rejected: SyncTooFrequent"
        );
    }

    #[test]
    fn constructors_produce_expected_messages() {
        assert_eq!(
            Error::decode("node data", "invalid base64").to_string(),
            "Failed to decode node data: invalid base64"
        );
        assert_eq!(
            Error::primary_request("mmr peaks", "connection refused").to_string(),
            "Request to primary failed (mmr peaks): connection refused"
        );
    }

    #[test]
    fn remaining_variants_produce_expected_messages() {
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
    }
}
