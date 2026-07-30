// SPDX-License-Identifier: GPL-3.0-only

//! Integration tests for the fixed-offset `Challenge<T>` SCALE decoder used by
//! the challenge responder.
//!
//! These exercise `decode_challenge_for_provider` against hand-rolled bytes
//! matching the pallet's `Challenge<T>` layout. We deliberately build the bytes
//! here rather than depend on the pallet's own struct so the test fails loudly
//! if the byte layout drifts between the two crates.

use storage_provider_node::challenge_responder::decode_challenge_for_provider;

/// Build raw bytes matching the pallet's `Challenge<T>` layout (see
/// `provider-node/src/challenge_responder.rs` for the field offsets).
fn build_challenge(provider: [u8; 32]) -> Vec<u8> {
    let mut buf = Vec::new();
    let bucket_id: u64 = 42;
    let challenger: [u8; 32] = [2u8; 32];
    let mmr_root: [u8; 32] = [3u8; 32];
    let start_seq: u64 = 100;
    let leaf_index: u64 = 7;
    let chunk_index: u64 = 0;
    let deposit: u128 = 1_000_000_000_000;
    let authorized: bool = true;
    buf.extend_from_slice(&bucket_id.to_le_bytes());
    buf.extend_from_slice(&provider);
    buf.extend_from_slice(&challenger);
    buf.extend_from_slice(&mmr_root);
    buf.extend_from_slice(&start_seq.to_le_bytes());
    buf.extend_from_slice(&leaf_index.to_le_bytes());
    buf.extend_from_slice(&chunk_index.to_le_bytes());
    buf.extend_from_slice(&deposit.to_le_bytes());
    buf.push(authorized as u8);
    buf
}

/// A single-value `Challenge` whose provider matches us decodes into `Some`
/// with the expected fields.
#[test]
fn decode_single_challenge_matching_provider() {
    let our_bytes: [u8; 32] = [9u8; 32];
    let encoded = build_challenge(our_bytes);

    let decoded = decode_challenge_for_provider(&encoded, &our_bytes)
        .expect("decodes")
        .expect("matches our provider");
    assert_eq!(decoded.bucket_id, 42);
    assert_eq!(decoded.start_seq, 100);
    assert_eq!(decoded.leaf_index, 7);
    assert_eq!(decoded.chunk_index, 0);
    assert_eq!(decoded.challenger, [2u8; 32]);
    assert_eq!(decoded.mmr_root.0, [3u8; 32]);
}

/// A `Challenge` targeting a different provider decodes into `None`.
#[test]
fn decode_single_challenge_other_provider() {
    let our_bytes: [u8; 32] = [9u8; 32];
    let other_bytes: [u8; 32] = [1u8; 32];
    let encoded = build_challenge(other_bytes);

    let decoded = decode_challenge_for_provider(&encoded, &our_bytes).expect("decodes");
    assert!(
        decoded.is_none(),
        "challenge for another provider is filtered out"
    );
}

/// A truncated value (shorter than the fixed layout) is a decode error.
#[test]
fn decode_single_challenge_truncated() {
    let our_bytes: [u8; 32] = [9u8; 32];
    let mut encoded = build_challenge(our_bytes);
    encoded.pop(); // one byte short of the fixed 145-byte layout
    assert!(decode_challenge_for_provider(&encoded, &our_bytes).is_err());
}
