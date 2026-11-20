// SPDX-License-Identifier: Apache-2.0

//! Provider settings checks and partial updates.

use crate::ProviderSettings;

/// Why a provider registration or a [`ProviderSettings`] value is rejected.
/// The settings variants are the `StorageProvider` pallet errors of the same
/// name.
#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
pub enum SettingsError {
    /// The provider's multiaddr is empty.
    #[error("multiaddr must not be empty")]
    EmptyMultiaddr,
    /// The provider's stake is zero.
    #[error("stake must be greater than zero")]
    ZeroStake,
    /// `min_duration` is above `max_duration`.
    #[error("min_duration {min_duration} exceeds max_duration {max_duration}")]
    MinDurationExceedsMaxDuration {
        /// The rejected minimum.
        min_duration: u32,
        /// The rejected maximum.
        max_duration: u32,
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
    /// Before those, it rejects an empty `multiaddr` and a zero `stake`.
    ///
    /// `multiaddr`, `committed_bytes` and `stake` are the provider's values:
    /// the registration input for a new provider, the on-chain record for a
    /// registered one. `min_stake_per_byte` is the chain's `MinStakePerByte`
    /// parameter.
    pub fn validate(
        &self,
        multiaddr: &str,
        committed_bytes: u64,
        stake: u128,
        min_stake_per_byte: u128,
    ) -> Result<(), SettingsError> {
        if multiaddr.is_empty() {
            return Err(SettingsError::EmptyMultiaddr);
        }
        if stake == 0 {
            return Err(SettingsError::ZeroStake);
        }
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
    pub min_duration: Option<u32>,
    /// New `max_duration`, if changed.
    pub max_duration: Option<u32>,
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
