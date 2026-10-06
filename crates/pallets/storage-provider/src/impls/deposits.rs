// SPDX-License-Identifier: Apache-2.0

//! Storage-deposit footprints: one fixed size per record kind, priced by
//! [`Config::StorageDeposit`]. Fixed sizes mean a ticket never has to be
//! updated while its record exists.

use crate::{pallet::Member, *};
use frame_support::{pallet_prelude::*, traits::Footprint};
use sp_core::H256;
use storage_primitives::{BucketId, Visibility};

impl<T: Config> Pallet<T> {
    /// A provider record.
    pub fn provider_footprint() -> Footprint {
        Footprint::from_mel::<ProviderInfo<T>>()
    }

    /// A bucket member together with its `MemberBuckets` reverse-index entry.
    pub fn member_footprint() -> Footprint {
        Footprint::from_parts(
            1,
            Member::<T>::max_encoded_len() + BucketId::max_encoded_len(),
        )
    }

    /// An agreement record.
    pub fn agreement_footprint() -> Footprint {
        Footprint::from_mel::<StorageAgreement<T>>()
    }

    /// A bucket record as `create_bucket` writes it: one member, no primary
    /// providers, no snapshot. A primary slot seeded at creation is paid for
    /// by the agreement deposit.
    pub fn bucket_footprint() -> Footprint {
        let one_member = 1 + Member::<T>::max_encoded_len();
        let no_frozen_start_seq = 1;
        let empty_primary_providers = 1;
        let no_snapshot = 1;
        let size = one_member
            + Visibility::max_encoded_len()
            + no_frozen_start_seq
            + u32::max_encoded_len()
            + empty_primary_providers
            + no_snapshot
            + <[(u32, H256); 6]>::max_encoded_len()
            + u32::max_encoded_len()
            + T::AccountId::max_encoded_len()
            + TicketOf::<T>::max_encoded_len();
        Footprint::from_parts(1, size)
    }
}
