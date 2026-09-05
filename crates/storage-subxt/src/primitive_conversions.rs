// SPDX-License-Identifier: Apache-2.0

//! Conversions between the generated runtime types and `storage-primitives`.

use crate::api::runtime_types::storage_primitives::{
    Role as RuntimeRole, Visibility as RuntimeVisibility,
};

impl From<RuntimeRole> for storage_primitives::Role {
    fn from(role: RuntimeRole) -> Self {
        match role {
            RuntimeRole::Admin => Self::Admin,
            RuntimeRole::Writer => Self::Writer,
            RuntimeRole::Reader => Self::Reader,
        }
    }
}

impl From<RuntimeVisibility> for storage_primitives::Visibility {
    fn from(visibility: RuntimeVisibility) -> Self {
        match visibility {
            RuntimeVisibility::Public => Self::Public,
            RuntimeVisibility::Private => Self::Private,
        }
    }
}
