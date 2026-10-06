// SPDX-License-Identifier: Apache-2.0

use super::*;
use frame_support::{pallet_prelude::MaxEncodedLen, traits::Footprint};

/// A member's deposit pays for two writes, the entry in the bucket and the
/// reverse-index entry, so the footprint must count both.
#[test]
fn member_footprint_covers_the_reverse_index_entry() {
    let expected = Footprint::from_parts(
        1,
        Member::<Test>::max_encoded_len() + storage_primitives::BucketId::max_encoded_len(),
    );
    assert_eq!(StorageProvider::member_footprint(), expected);
    assert_eq!(
        StorageProvider::provider_footprint(),
        Footprint::from_mel::<ProviderInfo<Test>>()
    );
    assert_eq!(
        StorageProvider::agreement_footprint(),
        Footprint::from_mel::<StorageAgreement<Test>>()
    );
}
