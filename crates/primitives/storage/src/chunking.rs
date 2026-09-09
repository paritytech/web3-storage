// SPDX-License-Identifier: Apache-2.0

//! Chunking strategies for content storage.
//!
//! - [`chunk_fixed`]: even-sized chunks. A single-byte insertion at the start
//!   of a file invalidates every downstream chunk.
//! - [`chunk_cdc`]: content-defined chunks via FastCDC. Boundaries align on
//!   content, so an insertion only changes chunks straddling the edit.

use alloc::vec::Vec;

/// FastCDC minimum chunk size — below this, no boundary is emitted regardless
/// of the rolling hash. Keeps metadata overhead bounded for tiny files.
pub const CDC_MIN_SIZE: u32 = 64 * 1024;

/// FastCDC average (target) chunk size. Matches [`DEFAULT_CHUNK_SIZE`] so MMR
/// leaf counts and proof depths stay comparable to the fixed-size chunker.
///
/// [`DEFAULT_CHUNK_SIZE`]: crate::DEFAULT_CHUNK_SIZE
pub const CDC_AVG_SIZE: u32 = 256 * 1024;

/// FastCDC maximum chunk size — forced boundary even with no hash match.
/// Bounds worst-case proof / read cost.
pub const CDC_MAX_SIZE: u32 = 1024 * 1024;

/// Split `data` into fixed-size chunks of `chunk_size` bytes (the last chunk
/// may be smaller).
pub fn chunk_fixed(data: &[u8], chunk_size: usize) -> Vec<Vec<u8>> {
    data.chunks(chunk_size).map(<[u8]>::to_vec).collect()
}

/// Split `data` into content-defined chunks using FastCDC with the
/// [`CDC_MIN_SIZE`] / [`CDC_AVG_SIZE`] / [`CDC_MAX_SIZE`] parameters.
pub fn chunk_cdc(data: &[u8]) -> Vec<Vec<u8>> {
    chunk_cdc_with(data, CDC_MIN_SIZE, CDC_AVG_SIZE, CDC_MAX_SIZE)
}

/// Like [`chunk_cdc`] but returns slices of the input. Avoids allocating until
/// the caller decides what to do with each chunk.
pub fn chunk_cdc_borrowed(data: &[u8]) -> Vec<&[u8]> {
    if data.is_empty() {
        return Vec::new();
    }
    fastcdc::v2020::FastCDC::new(data, CDC_MIN_SIZE, CDC_AVG_SIZE, CDC_MAX_SIZE)
        .map(|chunk| &data[chunk.offset..chunk.offset + chunk.length])
        .collect()
}

/// Like [`chunk_cdc`] but with explicit parameters. Exposed for tests and
/// future tuning; callers should prefer [`chunk_cdc`].
pub fn chunk_cdc_with(data: &[u8], min: u32, avg: u32, max: u32) -> Vec<Vec<u8>> {
    if data.is_empty() {
        return Vec::new();
    }
    fastcdc::v2020::FastCDC::new(data, min, avg, max)
        .map(|chunk| data[chunk.offset..chunk.offset + chunk.length].to_vec())
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::blake2_256;
    use rand::{RngCore, SeedableRng};
    use std::collections::HashSet;

    fn random_bytes(seed: u64, len: usize) -> Vec<u8> {
        let mut rng = rand::rngs::StdRng::seed_from_u64(seed);
        let mut buf = vec![0u8; len];
        rng.fill_bytes(&mut buf);
        buf
    }

    fn chunk_hashes(chunks: &[Vec<u8>]) -> HashSet<[u8; 32]> {
        chunks.iter().map(|c| blake2_256(c).0).collect()
    }

    #[test]
    fn cdc_size_distribution_within_bounds() {
        let data = random_bytes(1, 4 * 1024 * 1024);
        let chunks = chunk_cdc(&data);
        assert!(chunks.len() > 1, "expected multiple chunks for 4 MiB");
        // Every chunk except the trailing one must respect [min, max].
        for chunk in chunks.iter().take(chunks.len() - 1) {
            assert!(
                chunk.len() >= CDC_MIN_SIZE as usize,
                "chunk smaller than CDC_MIN_SIZE: {}",
                chunk.len()
            );
            assert!(
                chunk.len() <= CDC_MAX_SIZE as usize,
                "chunk larger than CDC_MAX_SIZE: {}",
                chunk.len()
            );
        }
    }

    #[test]
    fn cdc_is_deterministic() {
        let data = random_bytes(2, 1024 * 1024);
        let a = chunk_cdc(&data);
        let b = chunk_cdc(&data);
        assert_eq!(a, b);
    }

    #[test]
    fn cdc_reassembles_byte_equal() {
        let data = random_bytes(3, 1024 * 1024 + 12345);
        let chunks = chunk_cdc(&data);
        let mut joined = Vec::with_capacity(data.len());
        for c in &chunks {
            joined.extend_from_slice(c);
        }
        assert_eq!(joined, data);
    }

    /// 8 MiB is the smallest file size where avg-256K chunking gives enough
    /// leaves (~32) that one chunk straddling the edit is a small fraction;
    /// at 1 MiB the test would cap at 75% regardless of CDC quality.
    const EDIT_TEST_LEN: usize = 8 * 1024 * 1024;

    /// Insertion in the middle must reuse most chunks. Fixed-size would reuse
    /// roughly half (only chunks before the insertion); CDC reuses everything
    /// outside the window straddling the edit.
    #[test]
    fn cdc_insertion_reuses_most_chunks() {
        let v1 = random_bytes(4, EDIT_TEST_LEN);
        let mut v2 = v1.clone();
        let insertion_point = v1.len() / 2;
        v2.splice(
            insertion_point..insertion_point,
            random_bytes(5, 200).iter().copied(),
        );

        let v1_chunks = chunk_cdc(&v1);
        let v2_chunks = chunk_cdc(&v2);
        let v1_hashes = chunk_hashes(&v1_chunks);
        let v2_hashes = chunk_hashes(&v2_chunks);
        let shared = v1_hashes.intersection(&v2_hashes).count();
        let ratio = shared as f64 / v1_hashes.len() as f64;
        assert!(
            ratio >= 0.9,
            "expected ≥ 90% chunk reuse on mid-file insertion, got {:.2}% \
             (v1={}, v2={}, shared={})",
            ratio * 100.0,
            v1_hashes.len(),
            v2_hashes.len(),
            shared
        );
    }

    #[test]
    fn cdc_deletion_reuses_most_chunks() {
        let v1 = random_bytes(6, EDIT_TEST_LEN);
        let mut v2 = v1.clone();
        let cut = v1.len() / 2;
        v2.drain(cut..cut + 200);

        let v1_hashes = chunk_hashes(&chunk_cdc(&v1));
        let v2_hashes = chunk_hashes(&chunk_cdc(&v2));
        let shared = v1_hashes.intersection(&v2_hashes).count();
        let ratio = shared as f64 / v1_hashes.len() as f64;
        assert!(
            ratio >= 0.9,
            "expected ≥ 90% chunk reuse on mid-file deletion, got {:.2}%",
            ratio * 100.0
        );
    }

    /// Appending data must leave every prior chunk's CID unchanged except
    /// possibly the trailing partial chunk.
    #[test]
    fn cdc_append_preserves_prior_chunks() {
        let v1 = random_bytes(7, 1024 * 1024);
        let mut v2 = v1.clone();
        v2.extend_from_slice(&random_bytes(8, 50_000));

        let v1_hashes = chunk_hashes(&chunk_cdc(&v1));
        let v2_hashes = chunk_hashes(&chunk_cdc(&v2));
        let shared = v1_hashes.intersection(&v2_hashes).count();
        // The final v1 chunk may have been smaller than CDC_MIN and got
        // merged with the new tail; allow at most one prior chunk to differ.
        assert!(
            shared + 1 >= v1_hashes.len(),
            "expected all-but-one v1 chunks to survive append, got shared={} of {}",
            shared,
            v1_hashes.len()
        );
    }

    #[test]
    fn fixed_round_trips() {
        let data = random_bytes(9, 1024 * 1024 + 7);
        let chunks = chunk_fixed(&data, 256 * 1024);
        // Last chunk smaller, others exactly 256 KiB.
        for chunk in chunks.iter().take(chunks.len() - 1) {
            assert_eq!(chunk.len(), 256 * 1024);
        }
        let joined: Vec<u8> = chunks.into_iter().flatten().collect();
        assert_eq!(joined, data);
    }
}
