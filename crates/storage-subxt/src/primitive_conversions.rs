// SPDX-License-Identifier: Apache-2.0

//! Conversions between the generated runtime types and `storage-primitives`.

use crate::api::runtime_types::storage_primitives::{
    Commitment as RuntimeCommitment, ProviderRole as RuntimeProviderRole,
    ReplicaSyncRecord as RuntimeReplicaSyncRecord, Role as RuntimeRole,
    Visibility as RuntimeVisibility,
};
use storage_primitives::{RuntimeBalance, RuntimeBlockNumber};

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

impl From<RuntimeCommitment> for storage_primitives::Commitment {
    fn from(commitment: RuntimeCommitment) -> Self {
        Self {
            mmr_root: commitment.mmr_root.0.into(),
            start_seq: commitment.start_seq,
            leaf_count: commitment.leaf_count,
        }
    }
}

impl From<RuntimeReplicaSyncRecord<RuntimeBlockNumber>>
    for storage_primitives::ReplicaSyncRecord<RuntimeBlockNumber>
{
    fn from(record: RuntimeReplicaSyncRecord<RuntimeBlockNumber>) -> Self {
        Self {
            commitment: record.commitment.into(),
            block: record.block,
        }
    }
}

impl From<RuntimeProviderRole<RuntimeBalance, RuntimeBlockNumber>>
    for storage_primitives::RuntimeProviderRole
{
    fn from(role: RuntimeProviderRole<RuntimeBalance, RuntimeBlockNumber>) -> Self {
        match role {
            RuntimeProviderRole::Primary => Self::Primary,
            RuntimeProviderRole::Replica {
                sync_balance,
                sync_price,
                min_sync_interval,
                last_sync,
            } => Self::Replica {
                sync_balance,
                sync_price,
                min_sync_interval,
                last_sync: last_sync.map(Into::into),
            },
        }
    }
}
