// SPDX-License-Identifier: Apache-2.0

use crate::*;
use frame_support::{pallet_prelude::*, traits::ReservableCurrency};
use sp_runtime::traits::CheckedMul;
use storage_primitives::ReplayWindow;

impl<T: Config> Pallet<T> {
    /// Reject any path that would create a new commitment for a
    /// provider who has announced deregistration. `deregister_provider`
    /// also flips `accepting_primary`/`accepting_extensions` to `false`,
    pub(crate) fn ensure_provider_active(provider: &ProviderInfo<T>) -> DispatchResult {
        ensure!(
            provider.deregister_at.is_none(),
            Error::<T>::DeregisterAnnounced
        );
        Ok(())
    }

    /// Validate provider settings against committed bytes and stake.
    ///
    /// Shared by `update_provider_settings` and `register_provider_internal`.
    pub(crate) fn validate_settings(
        settings: &ProviderSettings<T>,
        committed_bytes: u64,
        stake: BalanceOf<T>,
    ) -> DispatchResult {
        ensure!(
            settings.min_duration <= settings.max_duration,
            Error::<T>::MinDurationExceedsMaxDuration
        );

        // Validate max_capacity >= committed_bytes (unless 0 = unlimited)
        if settings.max_capacity > 0 {
            ensure!(
                settings.max_capacity >= committed_bytes,
                Error::<T>::CapacityBelowCommitted
            );

            // Validate stake backs declared capacity
            use sp_runtime::traits::SaturatedConversion;
            let capacity_as_balance: BalanceOf<T> = settings.max_capacity.saturated_into();
            let required_stake = T::MinStakePerByte::get()
                .checked_mul(&capacity_as_balance)
                .ok_or(Error::<T>::ArithmeticOverflow)?;
            ensure!(
                stake >= required_stake,
                Error::<T>::InsufficientStakeForCapacity
            );
        }

        Ok(())
    }

    /// Register a provider, reserving their stake.
    ///
    /// Shared by the `register_provider` extrinsic (which passes
    /// `ProviderSettings::default()`) and genesis build (which passes the
    /// full settings, since post-genesis there is no one to call
    /// `update_provider_settings`).
    pub(crate) fn register_provider_internal(
        who: &T::AccountId,
        multiaddr: BoundedVec<u8, T::MaxMultiaddrLength>,
        public_key: BoundedVec<u8, ConstU32<64>>,
        stake: BalanceOf<T>,
        settings: ProviderSettings<T>,
    ) -> DispatchResult {
        ensure!(
            !Providers::<T>::contains_key(who),
            Error::<T>::ProviderAlreadyRegistered
        );
        ensure!(
            stake >= T::MinProviderStake::get(),
            Error::<T>::InsufficientStake
        );

        // Validate public key length (32 bytes for Sr25519/Ed25519, 33 for Ecdsa compressed)
        let key_len = public_key.len();
        ensure!(
            key_len == 32 || key_len == 33 || key_len == 64,
            Error::<T>::InvalidPublicKey
        );

        Self::validate_settings(&settings, 0, stake)?;

        // Reserve stake
        T::Currency::reserve(who, stake)?;

        let current_block = Self::current_block();

        let provider_info = ProviderInfo {
            multiaddr,
            public_key,
            stake,
            committed_bytes: 0,
            settings,
            stats: ProviderStats {
                registered_at: current_block,
                ..Default::default()
            },
            deregister_at: None,
        };

        Providers::<T>::insert(who, provider_info);
        ProviderReplayStates::<T>::insert(who, ReplayWindow::default());

        Self::deposit_event(Event::ProviderRegistered {
            provider: who.clone(),
            stake,
        });

        Ok(())
    }
}
