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
use storage_primitives::Commitment;
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

/// Read a named field and decode it as a [`Commitment`].
///
/// `Commitment` is a named-struct field (`mmr_root`/`start_seq`/`leaf_count`),
/// so subxt decodes it as a nested `Composite::Named` — pull that inner
/// composite out and reuse the flat-field decoders on it.
pub fn field_commitment(fields: &Composite<u32>, name: &str) -> Option<Commitment> {
    let inner = match &fields.at(name)?.value {
        ValueDef::Composite(c) => c,
        _ => return None,
    };
    Some(Commitment {
        mmr_root: field_h256(inner, "mmr_root")?,
        start_seq: field_u64(inner, "start_seq")?,
        leaf_count: field_u64(inner, "leaf_count")?,
    })
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

#[cfg(test)]
mod tests {
    use super::*;

    fn u64_leaf(n: u64) -> Value<u32> {
        Value {
            value: ValueDef::Primitive(Primitive::u128(n as u128)),
            context: 0,
        }
    }

    /// `H256`/`AccountId32` decode as a newtype wrapping a fixed-size byte array,
    /// i.e. `Composite::Unnamed([Composite::Unnamed([byte × 32])])` — see
    /// `decode_h256`'s doc comment.
    fn h256_leaf(byte: u8) -> Value<u32> {
        let bytes = Composite::Unnamed((0..32).map(|_| u64_leaf(byte as u64)).collect());
        Value {
            value: ValueDef::Composite(Composite::Unnamed(vec![Value {
                value: ValueDef::Composite(bytes),
                context: 0,
            }])),
            context: 0,
        }
    }

    fn commitment_leaf(mmr_root_byte: u8, start_seq: u64, leaf_count: u64) -> Value<u32> {
        Value {
            value: ValueDef::Composite(Composite::named([
                ("mmr_root", h256_leaf(mmr_root_byte)),
                ("start_seq", u64_leaf(start_seq)),
                ("leaf_count", u64_leaf(leaf_count)),
            ])),
            context: 0,
        }
    }

    #[test]
    fn field_commitment_decodes_nested_named_composite() {
        let fields = Composite::named([("commitment", commitment_leaf(0xAB, 10, 5))]);

        let commitment = field_commitment(&fields, "commitment").expect("decodes");

        assert_eq!(commitment.mmr_root, H256::repeat_byte(0xAB));
        assert_eq!(commitment.start_seq, 10);
        assert_eq!(commitment.leaf_count, 5);
    }

    #[test]
    fn field_commitment_none_when_field_missing() {
        let fields: Composite<u32> = Composite::named(Vec::<(String, Value<u32>)>::new());

        assert!(field_commitment(&fields, "commitment").is_none());
    }

    #[test]
    fn field_commitment_none_when_not_a_composite() {
        // A same-named field that isn't a composite (e.g. a bare primitive)
        // must not be mistaken for a `Commitment`.
        let fields = Composite::named([("commitment", u64_leaf(42))]);

        assert!(field_commitment(&fields, "commitment").is_none());
    }

    #[test]
    fn field_commitment_none_when_inner_field_missing() {
        // A commitment composite missing one of the three expected fields
        // (here: no `leaf_count`) must fail closed, not default to 0.
        let incomplete = Value {
            value: ValueDef::Composite(Composite::named([
                ("mmr_root", h256_leaf(0xAB)),
                ("start_seq", u64_leaf(10)),
            ])),
            context: 0,
        };
        let fields = Composite::named([("commitment", incomplete)]);

        assert!(field_commitment(&fields, "commitment").is_none());
    }
}
