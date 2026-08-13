// SPDX-License-Identifier: Apache-2.0

//! Authentication error types, independent of any HTTP framework. The provider
//! node maps these onto its HTTP error responses.

use storage_primitives::BucketId;

/// Split by what the caller should do: `Unavailable` is worth retrying,
/// `Decode` is a bug.
#[derive(Debug, thiserror::Error)]
pub enum MembershipError {
    #[error("chain unavailable: {0}")]
    Unavailable(String),

    #[error("could not read membership for bucket {bucket_id}: {reason}")]
    Decode { bucket_id: BucketId, reason: String },
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
