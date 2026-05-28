//! Per-provider replay protection for signed agreement terms.
//!
//! Each provider maintains a sliding window over the most recent
//! [`REPLAY_WINDOW_BITS`] nonces it has issued. The window is anchored at the
//! highest nonce ever accepted (`hwm`) and tracks the inclusive range
//! `hwm - (REPLAY_WINDOW_BITS - 1) ..= hwm`. Nonces older than the window
//! are rejected outright; nonces inside the window are accepted at most
//! once.
//!
//! # Bit layout
//!
//! The LSB of `bitmap[0]` represents `hwm`, the next bit represents
//! `hwm - 1`, and so on. Advancing the window by `d` slots shifts the
//! bitmap left by `d` bits, dropping the oldest entries.

use codec::{Decode, DecodeWithMemTracking, Encode, MaxEncodedLen};
use core::fmt::Debug;
use scale_info::TypeInfo;

/// Width of the sliding replay window, in bits / nonce slots.
pub const REPLAY_WINDOW_BITS: u32 = 256;

/// Sliding replay window over the most recent [`REPLAY_WINDOW_BITS`] nonces
/// accepted from a provider.
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

/// Reasons [`ReplayWindow::try_accept`] can reject a nonce.
#[derive(Debug, PartialEq, Eq)]
pub enum ReplayError {
    /// The nonce is inside the window and has already been marked as seen.
    AlreadyUsed,
    /// The nonce is more than [`REPLAY_WINDOW_BITS`] slots behind `hwm`.
    TooOld,
}

impl ReplayWindow {
    /// Records `nonce` as seen, advancing the window if necessary.
    ///
    /// When `nonce > hwm`, the bitmap is shifted left by `nonce - hwm` and
    /// `hwm` is updated; the new high-water mark is then marked at bit 0.
    /// When `nonce <= hwm`, the bit at distance `hwm - nonce` is set.
    ///
    /// # Errors
    ///
    /// * [`ReplayError::AlreadyUsed`] if the nonce is inside the window
    ///   and its bit is already set.
    /// * [`ReplayError::TooOld`] if the nonce is more than
    ///   [`REPLAY_WINDOW_BITS`] slots behind `hwm`.
    pub fn try_accept(&mut self, nonce: u64) -> Result<(), ReplayError> {
        if nonce > self.hwm {
            let shift = nonce - self.hwm;
            shift_left_le(&mut self.bitmap, shift);
            self.hwm = nonce;
            self.bitmap[0] |= 1;
            return Ok(());
        }

        let distance = self.hwm - nonce;
        if distance >= REPLAY_WINDOW_BITS as u64 {
            return Err(ReplayError::TooOld);
        }
        let byte = (distance / 8) as usize;
        let mask = 1u8 << (distance % 8);
        if self.bitmap[byte] & mask != 0 {
            return Err(ReplayError::AlreadyUsed);
        }
        self.bitmap[byte] |= mask;
        Ok(())
    }
}

/// Shifts a 256-bit little-endian bitmap left by `shift` bits.
///
/// Bit `i` (counting from the LSB of `bytes[0]`) moves to position
/// `i + shift`; positions `< shift` are cleared. Shifts of
/// [`REPLAY_WINDOW_BITS`] or more clear the entire bitmap.
fn shift_left_le(bytes: &mut [u8; 32], shift: u64) {
    if shift == 0 {
        return;
    }
    if shift >= REPLAY_WINDOW_BITS as u64 {
        *bytes = [0u8; 32];
        return;
    }
    let byte_shift = (shift / 8) as usize;
    let bit_shift = (shift % 8) as u32;

    let mut out = [0u8; 32];
    if bit_shift == 0 {
        for i in byte_shift..32 {
            out[i] = bytes[i - byte_shift];
        }
    } else {
        for i in byte_shift..32 {
            let src = i - byte_shift;
            let lo = bytes[src] << bit_shift;
            let hi = if src > 0 {
                bytes[src - 1] >> (8 - bit_shift)
            } else {
                0
            };
            out[i] = lo | hi;
        }
    }
    *bytes = out;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn first_nonce_sets_hwm() {
        let mut w = ReplayWindow::default();
        assert!(w.try_accept(5).is_ok());
        assert_eq!(w.hwm, 5);
        assert_eq!(w.bitmap[0] & 1, 1);
    }

    #[test]
    fn duplicate_nonce_rejected() {
        let mut w = ReplayWindow::default();
        w.try_accept(10).unwrap();
        assert_eq!(w.try_accept(10), Err(ReplayError::AlreadyUsed));
    }

    #[test]
    fn out_of_order_nonces_accepted() {
        let mut w = ReplayWindow::default();
        for n in [3u64, 7, 1, 10, 2] {
            assert!(w.try_accept(n).is_ok(), "nonce {n} should be accepted");
        }
        assert_eq!(w.hwm, 10);
        for n in [3u64, 7, 1, 10, 2] {
            assert_eq!(w.try_accept(n), Err(ReplayError::AlreadyUsed));
        }
    }

    #[test]
    fn nonce_at_window_edge_accepted() {
        let mut w = ReplayWindow::default();
        w.try_accept(300).unwrap();
        // Distance 255 is still inside the window.
        assert!(w.try_accept(300 - 255).is_ok());
    }

    #[test]
    fn nonce_past_window_edge_rejected() {
        let mut w = ReplayWindow::default();
        w.try_accept(300).unwrap();
        // Distance 256 is just past the window.
        assert_eq!(w.try_accept(300 - 256), Err(ReplayError::TooOld));
        // Distance much greater than 256.
        assert_eq!(w.try_accept(1), Err(ReplayError::TooOld));
    }

    #[test]
    fn forward_jump_beyond_window_clears_bitmap() {
        let mut w = ReplayWindow::default();
        w.try_accept(5).unwrap();
        w.try_accept(7).unwrap();
        w.try_accept(1000).unwrap();
        assert_eq!(w.hwm, 1000);
        assert_eq!(w.bitmap[0], 1);
        for b in &w.bitmap[1..] {
            assert_eq!(*b, 0);
        }
    }

    #[test]
    fn bitmap_shifts_track_distances() {
        let mut w = ReplayWindow::default();
        w.try_accept(100).unwrap();
        w.try_accept(101).unwrap();
        // 101 is hwm; 100 sits at distance 1, so bits 0 and 1 of byte 0 are set.
        assert_eq!(w.bitmap[0] & 0b11, 0b11);
        w.try_accept(109).unwrap();
        // After shifting left by 8, the bits previously at positions 0 and 1
        // land in byte 1 at positions 0 and 1; bit 0 of byte 0 marks the new hwm.
        assert_eq!(w.bitmap[0] & 1, 1);
        assert_eq!(w.bitmap[1] & 0b11, 0b11);
    }
}
