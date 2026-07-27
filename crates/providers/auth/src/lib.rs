// SPDX-License-Identifier: Apache-2.0

//! Provider HTTP authentication — shared by the client SDK (which builds the
//! signed `Authorization` header) and the provider node (which verifies it and
//! enforces bucket roles).
//!
//! * [`http_auth`] — the signed `Authorization` header format for bucket-scoped
//!   provider HTTP requests (client builds, provider verifies).
//! * [`membership`] — bucket membership resolution with TTL caching, backed by
//!   chain queries via subxt.
//! * [`verify`] — sr25519 request signature verification and role-based access
//!   control enforcement.

pub mod error;
pub mod http_auth;
pub mod membership;
pub mod verify;

pub use error::AuthError;
pub use http_auth::{auth_message, build_auth_header};
pub use membership::{
    ChainMembershipResolver, MembershipCache, MembershipResolver, StaticMembershipResolver,
};
pub use verify::{require_role, verify_signature, RequiredRole};
