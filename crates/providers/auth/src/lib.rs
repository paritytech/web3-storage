// SPDX-License-Identifier: Apache-2.0

//! Provider HTTP authentication — shared by the client SDK (which builds the
//! signed `Authorization` header) and the provider node (which verifies it and
//! enforces bucket roles).
//!
//! * [`http_auth`] — the signed `Authorization` header format.
//! * [`membership`] — bucket members, the role ladder, and a TTL cache.
//! * [`verify`] — [`Authenticator`]: signature verification and role enforcement.

pub mod error;
pub mod http_auth;
pub mod membership;
pub mod verify;

pub use error::{AuthError, MembershipError};
pub use http_auth::{auth_message, build_auth_header};
pub use membership::{Member, MembershipResolver, RequiredRole, StaticMembershipResolver};
pub use verify::Authenticator;
