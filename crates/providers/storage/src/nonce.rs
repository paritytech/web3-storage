// SPDX-License-Identifier: Apache-2.0

//! Persistence for the negotiation nonce counter's high-water mark.

/// Persistence layer for the nonce counter's high-water mark.
///
/// [`DiskNonceStore`](crate::DiskNonceStore) is the implementation, backed by
/// the provider's own database.
pub trait NonceStore: Send + Sync {
    /// Return the highest persisted nonce value, or `None` on a fresh store.
    fn load(&self) -> Option<u64>;

    /// Persist `value` as the new high-water mark. Monotonic: a lower value
    /// is silently ignored. Best-effort: errors are logged but not propagated.
    fn persist(&self, value: u64);

    /// Clear the persisted high-water mark so a re-registration starts fresh.
    ///
    /// Call this when the provider deregisters. On the next registration the
    /// counter will seed from `chain_hsn + 1` rather than the old watermark.
    /// Best-effort: errors are logged but not propagated.
    fn reset(&self);
}
