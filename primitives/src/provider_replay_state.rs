//! Per-provider replay protection for signed agreement terms.
//!
//! Each provider maintains a sliding window over the last
//! [`REPLAY_WINDOW_BITS`] nonces it has seen. The window is anchored at the
//! highest-seen nonce (`hwm`) and tracks acceptance state for the inclusive
//! range `hwm - (REPLAY_WINDOW_BITS - 1) ..= hwm`. Nonces older than the
//! window are rejected outright; nonces inside the window are accepted at
//! most once.
//!
//! Bit layout: the LSB of `bitmap[0]` represents `hwm`, the next bit
//! represents `hwm - 1`, and so on. Advancing the window by `d` slots shifts
//! the bitmap left by `d` bits, dropping the oldest entries.

use codec::{Decode, DecodeWithMemTracking, Encode, MaxEncodedLen};
use core::fmt::Debug;
use scale_info::TypeInfo;

/// Width of the sliding replay window, in bits / nonce slots.
pub const REPLAY_WINDOW_BITS: u32 = 256;

/// Sliding replay window tracking the most recent [`REPLAY_WINDOW_BITS`]
/// nonces a provider has signed.
#[derive(
    Clone,
    PartialEq,
    Eq,
    Encode,
    Decode,
    DecodeWithMemTracking,
    TypeInfo,
    MaxEncodedLen,
    Debug,
    Default,
)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct ReplayWindow {
    /// Highest nonce ever accepted for this provider (window anchor).
    pub hwm: u64,
    /// 256-bit acceptance bitmap; bit `i` (counting from the LSB of
    /// `bitmap[0]`) is set iff nonce `hwm - i` has been accepted.
    pub bitmap: [u8; 32],
}
