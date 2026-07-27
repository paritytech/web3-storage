// SPDX-License-Identifier: Apache-2.0

//! Authentication error type, independent of any HTTP framework. The provider
//! node maps these onto its HTTP error responses.

#[derive(Debug, thiserror::Error)]
pub enum AuthError {
    #[error("Authentication required")]
    AuthRequired,

    #[error("Request timestamp expired")]
    TimestampExpired,

    #[error("Insufficient role")]
    InsufficientRole,

    #[error("Membership lookup failed: {0}")]
    MembershipLookup(String),
}
