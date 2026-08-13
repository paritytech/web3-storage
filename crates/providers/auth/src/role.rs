// SPDX-License-Identifier: Apache-2.0

//! The privilege ladder: what an endpoint demands, and which granted role
//! clears it.
//!
//! Kept apart from [`crate::verify`] so the policy is one small, directly
//! testable thing rather than a `match` buried in the request path — and so
//! widening it is a visible edit to this file.

use storage_primitives::Role;

/// Required role for an endpoint.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RequiredRole {
    Reader,
    Writer,
    Admin,
}

impl RequiredRole {
    /// Whether a member holding `granted` clears this bar.
    ///
    /// Matched over the `(required, granted)` pair rather than on `self` alone:
    /// a new [`Role`] variant then fails to compile here until it is given an
    /// explicit answer, instead of silently inheriting one.
    pub fn is_satisfied_by(self, granted: Role) -> bool {
        match (self, granted) {
            // Any member of the bucket can read.
            (RequiredRole::Reader, Role::Reader | Role::Writer | Role::Admin) => true,
            (RequiredRole::Writer, Role::Writer | Role::Admin) => true,
            (RequiredRole::Writer, Role::Reader) => false,
            (RequiredRole::Admin, Role::Admin) => true,
            (RequiredRole::Admin, Role::Reader | Role::Writer) => false,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn privilege_ladder_is_exhaustive() {
        // Every (required, granted) pair, so the ladder cannot be widened
        // without a test failing.
        for (required, granted, expected) in [
            (RequiredRole::Reader, Role::Reader, true),
            (RequiredRole::Reader, Role::Writer, true),
            (RequiredRole::Reader, Role::Admin, true),
            (RequiredRole::Writer, Role::Reader, false),
            (RequiredRole::Writer, Role::Writer, true),
            (RequiredRole::Writer, Role::Admin, true),
            (RequiredRole::Admin, Role::Reader, false),
            (RequiredRole::Admin, Role::Writer, false),
            (RequiredRole::Admin, Role::Admin, true),
        ] {
            assert_eq!(
                required.is_satisfied_by(granted),
                expected,
                "{granted:?} vs required {required:?}"
            );
        }
    }
}
