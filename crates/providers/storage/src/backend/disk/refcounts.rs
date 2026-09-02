// SPDX-License-Identifier: Apache-2.0

//! The CF_REFCOUNTS value format.
//!
//! One entry per stored node: how many committed leaves (live or stashed,
//! across all buckets) reach it, plus which bucket's `used_bytes` was
//! charged when the node was first stored — so erasure credits the exact
//! payer. Encoded with bincode like every other value in this backend
//! (three fixed-width LE u64s, 24 bytes).

use crate::error::Error;

#[derive(serde::Serialize, serde::Deserialize)]
struct RefcountEntry {
    count: u64,
    charged_bucket: u64,
    size: u64,
}

/// Encode a CF_REFCOUNTS value.
pub(super) fn encode_refcount(count: u64, charged_bucket: u64, size: u64) -> Vec<u8> {
    bincode::serialize(&RefcountEntry {
        count,
        charged_bucket,
        size,
    })
    .expect("serializing three u64s cannot fail")
}

/// Decode a CF_REFCOUNTS value into `(count, charged_bucket, size)`.
pub(super) fn decode_refcount(value: &[u8]) -> Result<(u64, u64, u64), Error> {
    let entry: RefcountEntry = bincode::deserialize(value)
        .map_err(|e| Error::Storage(format!("corrupt refcount entry: {e}")))?;
    Ok((entry.count, entry.charged_bucket, entry.size))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn refcount_round_trips_and_rejects_garbage() {
        let bytes = encode_refcount(3, 7, 1024);
        assert_eq!(decode_refcount(&bytes).unwrap(), (3, 7, 1024));
        assert!(decode_refcount(&bytes[..20]).is_err());
    }
}
