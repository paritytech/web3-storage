// SPDX-License-Identifier: Apache-2.0

//! Provider HTTP authentication — shared by the client SDK (which builds the
//! signed `Authorization` header) and the provider node (which verifies it and
//! enforces bucket roles).
//!
//! * [`http_auth`] — the signed `Authorization` header format.
//! * [`membership`] — bucket members and visibility, the role ladder, and a TTL cache.
//! * [`verify`] — [`Authenticator`]: signature verification and role enforcement.
//!
//! Inputs: the `Authorization` header, HTTP method and bucket id of a request, plus a
//! [`MembershipResolver`] that reads bucket members and a [`MembershipInvalidations`] feed that
//! reports membership changes. The provider node implements both over chain state and block events.
//!
//! Outputs: `Ok(())` when the caller holds the required role, otherwise an [`AuthError`].
//!
//! How it works:
//! 1. Drop the cached entries that the invalidation feed reports, then look up the bucket's
//!    members and visibility. A cache entry older than the TTL is refetched through the
//!    [`MembershipResolver`]. If the refetch fails, the old entry is served up to the stale bound.
//! 2. A `Reader` request on a public bucket, or on a bucket this node holds as a replica, passes
//!    without a signature.
//! 3. Otherwise verify the `Authorization` header: an sr25519 signature over
//!    `web3storage:<METHOD>:<bucket_id>:<timestamp>`, with the timestamp inside the skew window.
//! 4. The node's own provider account passes any `Reader` check.
//! 5. Map the signer to its bucket role and compare it with the [`RequiredRole`].

pub mod error;
pub mod http_auth;
pub mod membership;
#[cfg(test)]
mod test_support;
pub mod verify;

pub use error::{AuthError, MembershipError};
pub use http_auth::{auth_message, build_auth_header};
pub use membership::{
    BucketAccess, Invalidation, Member, MembershipInvalidations, MembershipResolver, RequiredRole,
    StaticMembershipResolver,
};
pub use verify::Authenticator;
