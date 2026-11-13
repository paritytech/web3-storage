// SPDX-License-Identifier: Apache-2.0

//! Plain types exchanged with the chain through [`ProviderChain`].
//!
//! Balances are `u128` in the chain's smallest unit, durations are `u64`
//! block counts, byte sizes are `u64`, and keys are raw bytes. A chain
//! implementation converts between these and its own encoding.
//!
//! [`ProviderChain`]: crate::ProviderChain

/// A provider's pricing, acceptance and capacity settings.
///
/// Field semantics follow the `StorageProvider` pallet's `ProviderSettings`
/// (see the rustdoc on `pallet-storage-provider`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ProviderSettings {
    /// Shortest agreement duration, in blocks, the provider accepts.
    pub min_duration: u64,
    /// Longest agreement duration, in blocks, the provider accepts.
    pub max_duration: u64,
    /// Price per byte per block.
    pub price_per_byte: u128,
    /// Whether the provider accepts new primary agreements.
    pub accepting_primary: bool,
    /// Price per confirmed replica sync. `None` means the provider takes no
    /// replica agreements.
    pub replica_sync_price: Option<u128>,
    /// Whether the provider accepts extensions of existing agreements.
    pub accepting_extensions: bool,
    /// Capacity limit in bytes. `0` means unlimited.
    pub max_capacity: u64,
    /// Smallest agreement size in bytes the provider accepts. `0` means no
    /// minimum.
    pub min_agreement_bytes: u64,
}

/// Why the `StorageProvider` pallet rejects a [`ProviderSettings`] value.
/// Each variant is the pallet error of the same name.
#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
pub enum SettingsError {
    /// `min_duration` is above `max_duration`.
    #[error("min_duration {min_duration} exceeds max_duration {max_duration}")]
    MinDurationExceedsMaxDuration {
        /// The rejected minimum.
        min_duration: u64,
        /// The rejected maximum.
        max_duration: u64,
    },
    /// A limited `max_capacity` is below the bytes already committed.
    #[error("max_capacity {max_capacity} is below the {committed_bytes} bytes already committed")]
    CapacityBelowCommitted {
        /// The rejected capacity.
        max_capacity: u64,
        /// Bytes committed across live agreements.
        committed_bytes: u64,
    },
    /// `min_agreement_bytes` is above a limited `max_capacity`.
    #[error("min_agreement_bytes {min_agreement_bytes} exceeds max_capacity {max_capacity}")]
    MinAgreementBytesExceedsMaxCapacity {
        /// The rejected minimum.
        min_agreement_bytes: u64,
        /// The capacity it exceeds.
        max_capacity: u64,
    },
    /// The stake cannot back a limited `max_capacity`: it is below
    /// `max_capacity * min_stake_per_byte`.
    #[error(
        "stake {stake} cannot back max_capacity {max_capacity}: {required_stake} required at {min_stake_per_byte} per byte"
    )]
    InsufficientStakeForCapacity {
        /// The locked stake.
        stake: u128,
        /// The rejected capacity.
        max_capacity: u64,
        /// The chain's minimum stake per byte.
        min_stake_per_byte: u128,
        /// `max_capacity * min_stake_per_byte`.
        required_stake: u128,
    },
    /// `max_capacity * min_stake_per_byte` does not fit a balance.
    #[error(
        "max_capacity {max_capacity} at {min_stake_per_byte} per byte overflows the required stake"
    )]
    ArithmeticOverflow {
        /// The rejected capacity.
        max_capacity: u64,
        /// The chain's minimum stake per byte.
        min_stake_per_byte: u128,
    },
}

impl ProviderSettings {
    /// The pallet's `validate_settings`, applied before a transaction is
    /// submitted: `min_duration <= max_duration`, and, when `max_capacity`
    /// is limited (non-zero), `max_capacity >= committed_bytes`,
    /// `min_agreement_bytes <= max_capacity`, and
    /// `stake >= max_capacity * min_stake_per_byte`. Checks run in the
    /// pallet's order, so the first error reported is the one the chain
    /// would report.
    ///
    /// `committed_bytes` and `stake` are the provider's current on-chain
    /// values; `min_stake_per_byte` is the chain's `MinStakePerByte`
    /// parameter.
    pub fn validate(
        &self,
        committed_bytes: u64,
        stake: u128,
        min_stake_per_byte: u128,
    ) -> Result<(), SettingsError> {
        if self.min_duration > self.max_duration {
            return Err(SettingsError::MinDurationExceedsMaxDuration {
                min_duration: self.min_duration,
                max_duration: self.max_duration,
            });
        }
        if self.max_capacity == 0 {
            return Ok(());
        }
        if self.max_capacity < committed_bytes {
            return Err(SettingsError::CapacityBelowCommitted {
                max_capacity: self.max_capacity,
                committed_bytes,
            });
        }
        if self.min_agreement_bytes > self.max_capacity {
            return Err(SettingsError::MinAgreementBytesExceedsMaxCapacity {
                min_agreement_bytes: self.min_agreement_bytes,
                max_capacity: self.max_capacity,
            });
        }
        let required_stake = min_stake_per_byte
            .checked_mul(u128::from(self.max_capacity))
            .ok_or(SettingsError::ArithmeticOverflow {
                max_capacity: self.max_capacity,
                min_stake_per_byte,
            })?;
        if stake < required_stake {
            return Err(SettingsError::InsufficientStakeForCapacity {
                stake,
                max_capacity: self.max_capacity,
                min_stake_per_byte,
                required_stake,
            });
        }
        Ok(())
    }
}

/// A partial change to [`ProviderSettings`]: every `Some` field replaces the
/// current value, every `None` field keeps it.
///
/// `replica_sync_price` is an `Option<Option<_>>`: the outer `None` keeps
/// the current value, `Some(None)` stops accepting replicas, `Some(Some(p))`
/// sets the price.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ProviderSettingsUpdate {
    /// New `min_duration`, if changed.
    pub min_duration: Option<u64>,
    /// New `max_duration`, if changed.
    pub max_duration: Option<u64>,
    /// New `price_per_byte`, if changed.
    pub price_per_byte: Option<u128>,
    /// New `accepting_primary`, if changed.
    pub accepting_primary: Option<bool>,
    /// New `replica_sync_price`, if changed.
    pub replica_sync_price: Option<Option<u128>>,
    /// New `accepting_extensions`, if changed.
    pub accepting_extensions: Option<bool>,
    /// New `max_capacity`, if changed.
    pub max_capacity: Option<u64>,
    /// New `min_agreement_bytes`, if changed.
    pub min_agreement_bytes: Option<u64>,
}

impl ProviderSettingsUpdate {
    /// `current` with every `Some` field of this update applied.
    pub fn apply(&self, current: &ProviderSettings) -> ProviderSettings {
        ProviderSettings {
            min_duration: self.min_duration.unwrap_or(current.min_duration),
            max_duration: self.max_duration.unwrap_or(current.max_duration),
            price_per_byte: self.price_per_byte.unwrap_or(current.price_per_byte),
            accepting_primary: self.accepting_primary.unwrap_or(current.accepting_primary),
            replica_sync_price: self
                .replica_sync_price
                .unwrap_or(current.replica_sync_price),
            accepting_extensions: self
                .accepting_extensions
                .unwrap_or(current.accepting_extensions),
            max_capacity: self.max_capacity.unwrap_or(current.max_capacity),
            min_agreement_bytes: self
                .min_agreement_bytes
                .unwrap_or(current.min_agreement_bytes),
        }
    }
}

/// Input to a provider registration.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ProviderRegistration {
    /// Network address clients connect to, e.g. `/ip4/1.2.3.4/tcp/3333`.
    pub multiaddr: String,
    /// Raw public key of the provider's signing keypair: 32 bytes for
    /// sr25519 and ed25519, 33 for compressed ecdsa.
    pub public_key: Vec<u8>,
    /// Stake to lock, in the chain's smallest unit.
    pub stake: u128,
}

/// A provider's on-chain record, as far as the helper operations need it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ProviderRecord {
    /// Advertised network address.
    pub multiaddr: String,
    /// Registered raw public key.
    pub public_key: Vec<u8>,
    /// Locked stake, in the chain's smallest unit.
    pub stake: u128,
    /// Bytes committed across all live agreements.
    pub committed_bytes: u64,
    /// Current settings.
    pub settings: ProviderSettings,
    /// Block at which an announced deregistration can complete. `None` when
    /// no deregistration is announced.
    pub deregister_at: Option<u64>,
}

/// Result of a transaction that the chain accepted.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TxReceipt {
    /// `0x`-prefixed hex hash of the block that included the transaction.
    /// `None` when the chain client learned that the transaction landed
    /// without observing the block, e.g. from a duplicate rejection on a
    /// retry.
    pub block_hash: Option<String>,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn settings() -> ProviderSettings {
        ProviderSettings {
            min_duration: 10,
            max_duration: 100,
            price_per_byte: 5,
            accepting_primary: true,
            replica_sync_price: Some(7),
            accepting_extensions: true,
            max_capacity: 0,
            min_agreement_bytes: 0,
        }
    }

    #[test]
    fn empty_update_keeps_current_settings() {
        assert_eq!(
            ProviderSettingsUpdate::default().apply(&settings()),
            settings()
        );
    }

    /// Every check of the pallet's `validate_settings`, in its order.
    #[test]
    fn validate_mirrors_the_pallet() {
        const STAKE: u128 = 1_000_000;
        const PER_BYTE: u128 = 10;
        const COMMITTED: u64 = 4_096;
        let check = |s: ProviderSettings| s.validate(COMMITTED, STAKE, PER_BYTE);

        // Unlimited capacity: only the duration order matters.
        check(settings()).unwrap();
        check(ProviderSettings {
            min_agreement_bytes: u64::MAX,
            ..settings()
        })
        .unwrap();
        assert_eq!(
            check(ProviderSettings {
                min_duration: 101,
                ..settings()
            }),
            Err(SettingsError::MinDurationExceedsMaxDuration {
                min_duration: 101,
                max_duration: 100,
            })
        );

        // Limited capacity.
        check(ProviderSettings {
            max_capacity: 100_000,
            min_agreement_bytes: 100_000,
            ..settings()
        })
        .unwrap();
        assert_eq!(
            check(ProviderSettings {
                max_capacity: 1_024,
                ..settings()
            }),
            Err(SettingsError::CapacityBelowCommitted {
                max_capacity: 1_024,
                committed_bytes: COMMITTED,
            })
        );
        assert_eq!(
            check(ProviderSettings {
                max_capacity: 8_192,
                min_agreement_bytes: 8_193,
                ..settings()
            }),
            Err(SettingsError::MinAgreementBytesExceedsMaxCapacity {
                min_agreement_bytes: 8_193,
                max_capacity: 8_192,
            })
        );
        assert_eq!(
            check(ProviderSettings {
                max_capacity: 100_001,
                ..settings()
            }),
            Err(SettingsError::InsufficientStakeForCapacity {
                stake: STAKE,
                max_capacity: 100_001,
                min_stake_per_byte: PER_BYTE,
                required_stake: 1_000_010,
            })
        );
        assert_eq!(
            ProviderSettings {
                max_capacity: u64::MAX,
                ..settings()
            }
            .validate(0, u128::MAX, u128::MAX),
            Err(SettingsError::ArithmeticOverflow {
                max_capacity: u64::MAX,
                min_stake_per_byte: u128::MAX,
            })
        );

        // The duration check comes first, as in the pallet.
        assert!(matches!(
            check(ProviderSettings {
                min_duration: 101,
                max_capacity: 1_024,
                ..settings()
            }),
            Err(SettingsError::MinDurationExceedsMaxDuration { .. })
        ));
    }

    #[test]
    fn update_replaces_only_set_fields() {
        let update = ProviderSettingsUpdate {
            price_per_byte: Some(9),
            replica_sync_price: Some(None),
            max_capacity: Some(1 << 30),
            ..Default::default()
        };
        let expected = ProviderSettings {
            price_per_byte: 9,
            replica_sync_price: None,
            max_capacity: 1 << 30,
            ..settings()
        };
        assert_eq!(update.apply(&settings()), expected);
    }
}
