// SPDX-License-Identifier: Apache-2.0

//! Authentication error types, independent of any HTTP framework. The provider
//! node maps these onto its HTTP error responses.

use storage_primitives::BucketId;

/// This node cannot answer for a bucket right now, whoever notices it: the
/// resolver or the authenticator. Split by what the caller should do. Never
/// cached, so the next request asks again.
#[derive(Debug, thiserror::Error)]
pub enum MembershipError {
    /// The chain could not be read; retry.
    #[error("chain unavailable: {0}")]
    Unavailable(String),
    /// The on-chain value did not decode; a bug, not retryable.
    #[error("could not read membership for bucket {bucket_id}: {reason}")]
    Decode { bucket_id: BucketId, reason: String },
    /// The request refers to a block newer than `read_block`, the block this
    /// node read the bucket at: the bucket id is one the chain allocates next,
    /// or the client's context block is newer than the read. Retry after a
    /// block.
    #[error("bucket {bucket_id} was read at block {read_block}; the block the request refers to has not reached this node")]
    BlockNotKnown {
        bucket_id: BucketId,
        read_block: u32,
    },
}

impl MembershipError {
    /// `Unavailable` from any error the chain read failed with.
    pub fn unavailable(error: impl std::fmt::Display) -> Self {
        Self::Unavailable(error.to_string())
    }
}

/// The verdict on a request.
#[derive(Debug, thiserror::Error)]
pub enum AuthError {
    #[error("Authentication required")]
    AuthRequired,

    #[error("Request timestamp expired")]
    TimestampExpired,

    #[error("Insufficient role")]
    InsufficientRole,

    /// The `X-Web3Storage-Context` header is not `<block_number>:<0x block hash>`.
    #[error("X-Web3Storage-Context is not <block_number>:<block_hash>")]
    ContextBlockInvalid,

    #[error("Membership lookup failed: {0}")]
    MembershipLookup(#[from] MembershipError),
}
