// SPDX-License-Identifier: Apache-2.0

//! On-chain drive metadata.

use codec::{Decode, Encode, MaxEncodedLen};
use scale_info::TypeInfo;
use sp_runtime::{traits::Get, BoundedVec};

/// Drive information stored on-chain (user's virtual drive)
///
/// File/directory metadata is managed off-chain by the provider node (fs_index).
/// Only drive lifecycle (create/delete) and storage parameters live on-chain.
#[derive(Clone, Encode, Decode, Eq, PartialEq, Debug, TypeInfo, MaxEncodedLen)]
#[scale_info(skip_type_params(MaxNameLength, Balance))]
#[codec(mel_bound())]
pub struct DriveInfo<
    AccountId: Encode + Decode + MaxEncodedLen,
    BlockNumber: Encode + Decode + MaxEncodedLen,
    MaxNameLength: Get<u32>,
> {
    /// Owner of the drive
    pub owner: AccountId,
    /// Layer 0 bucket ID this drive uses
    pub bucket_id: u64,
    /// Block number when drive was created
    pub created_at: BlockNumber,
    /// Optional human-readable name (bounded)
    pub name: Option<BoundedVec<u8, MaxNameLength>>,
    /// Maximum storage capacity in bytes
    pub max_capacity: u64,
    /// Storage period in blocks
    pub storage_period: BlockNumber,
    /// Expiry block number (created_at + storage_period)
    pub expires_at: BlockNumber,
}
