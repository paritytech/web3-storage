// SPDX-License-Identifier: Apache-2.0

//! Conversions from the generated mirrors of `storage_primitives` enums to
//! the real types, so consumers of the typed bindings can `.into()` instead
//! of re-enumerating variants at every use site.

use super::api::runtime_types::storage_primitives as generated;

impl From<generated::Role> for storage_primitives::Role {
    fn from(role: generated::Role) -> Self {
        match role {
            generated::Role::Admin => Self::Admin,
            generated::Role::Writer => Self::Writer,
            generated::Role::Reader => Self::Reader,
        }
    }
}

impl From<generated::Visibility> for storage_primitives::Visibility {
    fn from(visibility: generated::Visibility) -> Self {
        match visibility {
            generated::Visibility::Public => Self::Public,
            generated::Visibility::Private => Self::Private,
        }
    }
}
