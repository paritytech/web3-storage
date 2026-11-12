// SPDX-License-Identifier: Apache-2.0

//! The CF_REFCOUNTS value format.
//!
//! One entry per stored node: how many committed leaves (live or stashed,
//! across all buckets) reach it, plus which bucket's `used_bytes` was
//! charged when the node was first stored — so erasure credits the exact
//! payer, and whether the node is a chunk or an internal node, so a tree walk
//! learns a chunk's size and kind without reading the chunk. SCALE-encoded
//! like every other value in this backend (three fixed-width LE u64s and the
//! kind tag, 25 bytes).

use crate::error::Error;
use codec::{Decode, DecodeAll, Encode};

/// Whether a stored node is a chunk or an internal node.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Encode, Decode)]
pub(super) enum NodeKind {
    Chunk,
    Internal,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Encode, Decode)]
pub(super) struct RefcountEntry {
    pub count: u64,
    pub charged_bucket: u64,
    pub size: u64,
    pub kind: NodeKind,
}

/// Encode a CF_REFCOUNTS value.
pub(super) fn encode_refcount(entry: &RefcountEntry) -> Vec<u8> {
    entry.encode()
}

/// Decode a CF_REFCOUNTS value.
pub(super) fn decode_refcount(value: &[u8]) -> Result<RefcountEntry, Error> {
    Ok(RefcountEntry::decode_all(&mut &value[..])?)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn refcount_round_trips_and_rejects_garbage() {
        let entry = RefcountEntry {
            count: 3,
            charged_bucket: 7,
            size: 1024,
            kind: NodeKind::Internal,
        };
        let bytes = encode_refcount(&entry);
        assert_eq!(bytes.len(), 25);
        assert_eq!(decode_refcount(&bytes).unwrap(), entry);
        assert!(decode_refcount(&bytes[..24]).is_err());
        assert!(decode_refcount(&[bytes.as_slice(), &[0]].concat()).is_err());
        assert!(decode_refcount(&[&bytes[..24], &[9]].concat()).is_err());
    }
}
