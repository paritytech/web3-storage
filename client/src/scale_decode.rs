// SPDX-License-Identifier: Apache-2.0

//! Generic SCALE value decoders for use with subxt's dynamic event and storage APIs.
//!
//! Subxt's event API returns field values as `Composite<u32>` (field index as context);
//! `.at(name)` on that composite yields `Option<&Value<u32>>`. The helpers here wrap the
//! common access + decode patterns so callers don't repeat the same match arms.
//!
//! Only generic, domain-agnostic decoders belong here. Pallet-specific shapes (e.g. a
//! `ChallengeId` struct with `deadline`/`index` fields) should stay alongside their parser.

use sp_core::H256;
use sp_runtime::AccountId32;
use subxt::ext::scale_value::{At, Composite, Primitive, Value, ValueDef};

/// Read a named field as a `u64` (decoded from the underlying `u128`).
pub fn field_u64(fields: &Composite<u32>, name: &str) -> Option<u64> {
    fields.at(name)?.as_u128().map(|v| v as u64)
}

/// Read a named field as a `u32` (decoded from the underlying `u128`).
pub fn field_u32(fields: &Composite<u32>, name: &str) -> Option<u32> {
    fields.at(name)?.as_u128().map(|v| v as u32)
}

/// Read a named field as a `u128`.
pub fn field_u128(fields: &Composite<u32>, name: &str) -> Option<u128> {
    fields.at(name)?.as_u128()
}

/// Read a named field and decode it as an [`AccountId32`].
pub fn field_account(fields: &Composite<u32>, name: &str) -> Option<AccountId32> {
    decode_account(fields.at(name)?)
}

/// Read a named field and decode it as an [`H256`].
pub fn field_h256(fields: &Composite<u32>, name: &str) -> Option<H256> {
    decode_h256(fields.at(name)?)
}

/// Read a named field as a `Vec<AccountId32>`. Missing or unparseable fields yield an
/// empty vec.
pub fn field_accounts(fields: &Composite<u32>, name: &str) -> Vec<AccountId32> {
    fields.at(name).map(decode_account_vec).unwrap_or_default()
}

/// Read a named field as a `Vec<u8>` (e.g. a `BoundedVec<u8, _>` or raw `Vec<u8>`).
pub fn field_bytes(fields: &Composite<u32>, name: &str) -> Option<Vec<u8>> {
    decode_bytes(fields.at(name)?)
}

/// Decode an [`AccountId32`] from a SCALE value.
///
/// `AccountId32` is a newtype struct in the SCALE type system, so subxt decodes it as
/// `Composite::Unnamed([Composite::Unnamed([byte × 32])])`. [`collect_le_bytes`] handles
/// arbitrary nesting depth.
pub fn decode_account(v: &Value<u32>) -> Option<AccountId32> {
    let mut bytes = [0u8; 32];
    if collect_le_bytes(v, &mut bytes, 0) == 32 {
        Some(AccountId32::new(bytes))
    } else {
        None
    }
}

/// Decode an [`H256`] from a SCALE value (same nesting shape as `AccountId32`).
pub fn decode_h256(v: &Value<u32>) -> Option<H256> {
    let mut bytes = [0u8; 32];
    if collect_le_bytes(v, &mut bytes, 0) == 32 {
        Some(H256::from(bytes))
    } else {
        None
    }
}

/// Decode a `Vec<u8>` from a SCALE value.
///
/// Handles a flat unnamed composite of byte-as-`u128` leaves and a single-element wrapper
/// composite (e.g. `BoundedVec<u8, _>` decoded as `Composite::Unnamed([Composite::Unnamed([
/// byte … ])])`). An empty unnamed composite decodes to an empty `Vec`.
pub fn decode_bytes(v: &Value<u32>) -> Option<Vec<u8>> {
    match &v.value {
        ValueDef::Composite(Composite::Unnamed(items)) => {
            if items.is_empty() {
                return Some(Vec::new());
            }
            let bytes: Vec<u8> = items
                .iter()
                .filter_map(|child| child.as_u128().map(|n| n as u8))
                .collect();
            if bytes.len() == items.len() {
                return Some(bytes);
            }
            if items.len() == 1 {
                return decode_bytes(&items[0]);
            }
            None
        }
        ValueDef::Composite(Composite::Named(items)) if items.len() == 1 => {
            decode_bytes(&items[0].1)
        }
        _ => None,
    }
}

/// Decode a `Vec<AccountId32>` from an unnamed composite of account composites.
pub fn decode_account_vec(v: &Value<u32>) -> Vec<AccountId32> {
    match &v.value {
        ValueDef::Composite(Composite::Unnamed(items)) => {
            items.iter().filter_map(decode_account).collect()
        }
        _ => vec![],
    }
}

/// Extract the variant name from a `ValueDef::Variant`. Returns `None` for non-variant
/// values.
pub fn variant_name(v: &Value<u32>) -> Option<String> {
    match &v.value {
        ValueDef::Variant(var) => Some(var.name.clone()),
        _ => None,
    }
}

/// Recursively collect raw bytes from a SCALE value into `buf` starting at `offset`,
/// returning the new offset.
///
/// Treats `Primitive::U128` leaves as one byte each and recurses into `Composite::Unnamed`
/// nodes — covering both flat byte arrays and newtype-wrapped arrays like `AccountId32`.
pub fn collect_le_bytes(v: &Value<u32>, buf: &mut [u8; 32], offset: usize) -> usize {
    match &v.value {
        ValueDef::Primitive(Primitive::U128(n)) => {
            if offset < 32 {
                buf[offset] = *n as u8;
                offset + 1
            } else {
                offset
            }
        }
        ValueDef::Composite(Composite::Unnamed(items)) => {
            let mut pos = offset;
            for item in items {
                pos = collect_le_bytes(item, buf, pos);
            }
            pos
        }
        _ => offset,
    }
}
