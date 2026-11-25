// SPDX-License-Identifier: Apache-2.0

//! Transaction extension that drops challenge responses which cannot succeed.

use crate::{Call, Challenges, Config, Pallet};
use codec::{Decode, DecodeWithMemTracking, Encode};
use core::marker::PhantomData;
use frame_support::{
    dispatch::DispatchInfo,
    ensure,
    pallet_prelude::TransactionSource,
    traits::{Get, IsSubType},
    weights::Weight,
    CloneNoBound, DebugNoBound, DefaultNoBound, EqNoBound, PartialEqNoBound,
};
use scale_info::TypeInfo;
use sp_runtime::{
    impl_tx_ext_default,
    traits::{
        AsSystemOriginSigner, DispatchInfoOf, Dispatchable, TransactionExtension, ValidateResult,
    },
    transaction_validity::{InvalidTransaction, ValidTransaction},
};

type RuntimeCallOf<T> = <T as frame_system::Config>::RuntimeCall;

/// Rejects a `respond_to_challenge` at transaction validation when it cannot
/// succeed: `Stale` if the challenge no longer exists (answered, cancelled or
/// resolved) or its deadline has passed, `BadSigner` if the signer is not the
/// challenged provider. A rejected transaction is never included, so the
/// sender pays no fee. A valid response provides its challenge id as a tag, so
/// the transaction pool keeps one response per challenge. Every other call
/// passes unchanged.
#[derive(
    Encode,
    Decode,
    DecodeWithMemTracking,
    TypeInfo,
    CloneNoBound,
    EqNoBound,
    PartialEqNoBound,
    DefaultNoBound,
    DebugNoBound,
)]
#[scale_info(skip_type_params(T))]
pub struct CheckChallengeResponse<T: Config + Send + Sync>(PhantomData<T>);

impl<T: Config + Send + Sync> CheckChallengeResponse<T> {
    /// Create the extension. It contains no data.
    pub fn new() -> Self {
        Self::default()
    }
}

impl<T: Config + Send + Sync> TransactionExtension<RuntimeCallOf<T>> for CheckChallengeResponse<T>
where
    RuntimeCallOf<T>: Dispatchable<Info = DispatchInfo> + IsSubType<Call<T>>,
    <RuntimeCallOf<T> as Dispatchable>::RuntimeOrigin: AsSystemOriginSigner<T::AccountId> + Clone,
{
    const IDENTIFIER: &'static str = "CheckChallengeResponse";
    type Implicit = ();
    type Val = ();
    type Pre = ();

    fn weight(&self, call: &RuntimeCallOf<T>) -> Weight {
        match call.is_sub_type() {
            Some(Call::respond_to_challenge { .. }) => T::DbWeight::get().reads(2),
            _ => Weight::zero(),
        }
    }

    fn validate(
        &self,
        origin: <RuntimeCallOf<T> as Dispatchable>::RuntimeOrigin,
        call: &RuntimeCallOf<T>,
        _info: &DispatchInfoOf<RuntimeCallOf<T>>,
        _len: usize,
        _self_implicit: Self::Implicit,
        _inherited_implication: &impl Encode,
        _source: TransactionSource,
    ) -> ValidateResult<Self::Val, RuntimeCallOf<T>> {
        let Some(Call::respond_to_challenge { challenge_id, .. }) = call.is_sub_type() else {
            return Ok((ValidTransaction::default(), (), origin));
        };
        // After the deadline, dispatch fails with `ChallengeExpired` and the
        // challenge remains in storage until `resolve_expired_challenge`, so
        // validation rejects the response and the sender pays no fee.
        ensure!(
            Pallet::<T>::current_anchor_block() <= challenge_id.deadline,
            InvalidTransaction::Stale
        );
        let challenge = Challenges::<T>::get(challenge_id.deadline, challenge_id.index)
            .ok_or(InvalidTransaction::Stale)?;
        let signer = origin
            .as_system_origin_signer()
            .ok_or(InvalidTransaction::BadSigner)?;
        ensure!(*signer == challenge.provider, InvalidTransaction::BadSigner);

        let valid = ValidTransaction::with_tag_prefix("StorageProviderChallengeResponse")
            .and_provides(challenge_id)
            .into();
        Ok((valid, (), origin))
    }

    impl_tx_ext_default!(RuntimeCallOf<T>; prepare);
}
