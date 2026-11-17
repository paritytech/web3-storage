// SPDX-License-Identifier: Apache-2.0

//! The `CF_NODE_REFS` value format. SCALE-encoded like every other value in
//! this backend: three little-endian u64s and a one-byte kind, 25 bytes.

use crate::error::Error;
use codec::{Decode, DecodeAll, Encode};

/// Whether a stored node is a chunk or an internal node.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Encode, Decode)]
pub(super) enum NodeKind {
    Chunk,
    Internal,
}

/// What the store records about a node besides its body.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Encode, Decode)]
pub(super) struct RefcountEntry {
    /// How many paths from committed leaves, live or stashed, across all
    /// buckets, reach the node. Commit adds one per path and erasure subtracts
    /// one per path; at zero, erasure deletes the node and this row.
    pub count: u64,
    /// The bucket charged for the node when it was first stored. When erasure
    /// deletes the node, it subtracts `size` from this bucket's `used_bytes`
    /// and 1 from its `node_count`.
    pub charged_bucket: u64,
    /// The bytes charged for the node: the data length for a chunk, 64 (two
    /// child hashes) for an internal node.
    pub size: u64,
    /// Chunk or internal node. A tree walk reads a chunk's size from this row
    /// and does not read the chunk.
    pub kind: NodeKind,
}

pub(super) fn encode_refcount(entry: &RefcountEntry) -> Vec<u8> {
    entry.encode()
}

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
