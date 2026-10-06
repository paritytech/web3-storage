// SPDX-License-Identifier: Apache-2.0

//! Storage-deposit footprints: one fixed size per record kind, priced by
//! [`Config::StorageDeposit`]. Fixed sizes mean a ticket never has to be
//! updated while its record exists.

use crate::{pallet::Member, *};
use frame_support::{pallet_prelude::*, traits::Footprint};
use storage_primitives::BucketId;

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
}
