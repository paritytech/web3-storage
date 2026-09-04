// SPDX-License-Identifier: Apache-2.0

//! Tests for the storage provider pallet.

use crate::{mock::*, *};
use frame_support::{assert_err, assert_noop, assert_ok};
use storage_primitives::{ProviderRole, Role};

/// Helper function to create a test public key (32 bytes).
fn test_public_key() -> frame_support::BoundedVec<u8, frame_support::traits::ConstU32<64>> {
    vec![1u8; 32].try_into().unwrap()
}

mod agreement;
mod auto_matching;
mod bucket;
mod challenge;
mod checkpoint;
mod end_agreement;
mod error_paths;
mod extend_topup;
mod genesis;
mod member_buckets;
mod misc;
mod provider;
mod replica;
mod runtime_api;
mod try_state;
mod visibility;
