// SPDX-License-Identifier: Apache-2.0

//! Authentication error types, independent of any HTTP framework. The provider
//! node maps these onto its HTTP error responses.

use storage_primitives::BucketId;

/// Why membership could not be resolved, split by what the caller should do.
#[derive(Debug, thiserror::Error)]
pub enum MembershipError {
    /// The chain could not be read; retry.
    #[error("chain unavailable: {0}")]
    Unavailable(String),
    /// The on-chain value did not decode; a bug, not retryable.
    #[error("could not read membership for bucket {bucket_id}: {reason}")]
    Decode { bucket_id: BucketId, reason: String },
    /// The bucket is absent at the newest block this node has, but its id is
    /// one the chain allocates next, so the block that created it may not
    /// have reached this node yet; retry after a block.
    #[error("bucket {bucket_id} is not yet seen by this node")]
    BlockNotKnown { bucket_id: BucketId },
}

impl MembershipError {
    /// `Unavailable` from any error the chain read failed with.
    pub fn unavailable(error: impl std::fmt::Display) -> Self {
        Self::Unavailable(error.to_string())
    }
}

#[derive(Debug, thiserror::Error)]
pub enum AuthError {
    #[error("Authentication required")]
    AuthRequired,

    #[error("Request timestamp expired")]
    TimestampExpired,

    #[error("Insufficient role")]
    InsufficientRole,

    #[error("Membership lookup failed: {0}")]
    MembershipLookup(#[from] MembershipError),
}
