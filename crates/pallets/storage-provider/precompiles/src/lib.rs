// SPDX-License-Identifier: Apache-2.0

//! `pallet_revive` precompile exposing the client-side bucket lifecycle of
//! `pallet_storage_provider` to Solidity contracts.
//!
//! Address: `0x0000000000000000000000000000000009010000` (matcher
//! `Fixed(0x0901)`; the u16 sits at bytes 16-17 with a `0x0000` suffix at
//! bytes 18-19, which are reserved for built-in precompiles). Pattern follows
//! the XCM precompile reference (`polkadot/xcm/pallet-xcm/precompiles/src/lib.rs`
//! in `polkadot-stable2603`).

#![cfg_attr(not(feature = "std"), no_std)]

extern crate alloc;

use alloc::vec::Vec;
use codec::Decode;
use core::{fmt, marker::PhantomData, num::NonZero};
use frame_support::dispatch::RawOrigin;
use frame_system::pallet_prelude::BlockNumberFor;
use pallet_revive::{
    precompiles::{
        alloy::{self, sol_types::SolValue},
        AddressMatcher, Error, Ext, Precompile,
    },
    ExecOrigin as Origin,
};
use pallet_storage_provider::{BalanceOf, WeightInfo};
use storage_primitives::{BucketId, EndAction, Role, Visibility};
use tracing::error;

alloy::sol!("src/interface/IWeb3Storage.sol");
use IWeb3Storage::IWeb3StorageCalls;

const LOG_TARGET: &str = "web3-storage::precompile";

fn revert(error: &impl fmt::Debug, message: &str) -> Error {
    error!(target: LOG_TARGET, ?error, "{}", message);
    Error::Revert(message.into())
}

/// Decode a Solidity `bytes32` as a substrate `AccountId`. Our runtimes use
/// `AccountId32`, whose SCALE encoding is the raw 32 bytes — `Decode` matches.
fn decode_account<T>(bytes: &[u8; 32]) -> Result<T::AccountId, Error>
where
    T: frame_system::Config,
{
    T::AccountId::decode(&mut &bytes[..]).map_err(|e| {
        revert(
            &e,
            "Invalid account encoding: expected 32-byte substrate AccountId",
        )
    })
}

/// Rebuild the pallet's [`AgreementTermsOf`](pallet_storage_provider::AgreementTermsOf)
/// from its Solidity mirror so the SCALE encoding matches the payload the
/// provider signed.
fn decode_terms<T>(
    terms: &IWeb3Storage::PrimitiveAgreementTerms,
) -> Result<pallet_storage_provider::AgreementTermsOf<T>, Error>
where
    T: pallet_storage_provider::Config,
    BalanceOf<T>: From<u128>,
    BlockNumberFor<T>: From<u32>,
{
    Ok(storage_primitives::AgreementTerms {
        owner: decode_account::<T>(&terms.owner.0)?,
        max_bytes: terms.maxBytes,
        duration: BlockNumberFor::<T>::from(terms.duration),
        price_per_byte: BalanceOf::<T>::from(terms.pricePerByte),
        valid_until: BlockNumberFor::<T>::from(terms.validUntil),
        nonce: terms.nonce,
        bucket_id: terms.hasBucketId.then_some(terms.bucketId),
        replica_params: terms
            .hasReplicaParams
            .then(|| storage_primitives::ReplicaTerms {
                sync_balance: BalanceOf::<T>::from(terms.replicaParams.syncBalance),
                min_sync_interval: BlockNumberFor::<T>::from(terms.replicaParams.minSyncInterval),
                sync_price: BalanceOf::<T>::from(terms.replicaParams.syncPrice),
            }),
    })
}

/// The Solidity-side enums decode fallibly in `alloy`, so out-of-range
/// bytes revert before dispatch; the `__Invalid` arm is defensive only.
fn to_role(role: IWeb3Storage::Role) -> Result<Role, Error> {
    match role {
        IWeb3Storage::Role::Admin => Ok(Role::Admin),
        IWeb3Storage::Role::Writer => Ok(Role::Writer),
        IWeb3Storage::Role::Reader => Ok(Role::Reader),
        // Rejected by ABI decoding already; kept total to stay panic-free.
        IWeb3Storage::Role::__Invalid => Err(revert(&"__Invalid", "Invalid role")),
    }
}

fn to_visibility(visibility: IWeb3Storage::Visibility) -> Result<Visibility, Error> {
    match visibility {
        IWeb3Storage::Visibility::Public => Ok(Visibility::Public),
        IWeb3Storage::Visibility::Private => Ok(Visibility::Private),
        // Rejected by ABI decoding already; kept total to stay panic-free.
        IWeb3Storage::Visibility::__Invalid => Err(revert(&"__Invalid", "Invalid visibility")),
    }
}

/// Precompile wrapping `pallet_storage_provider`'s client-side extrinsics.
pub struct StorageProviderPrecompile<T>(PhantomData<T>);

impl<Runtime> Precompile for StorageProviderPrecompile<Runtime>
where
    Runtime: pallet_storage_provider::Config + pallet_revive::Config,
    BalanceOf<Runtime>: From<u128>,
    BlockNumberFor<Runtime>: From<u32>,
{
    type T = Runtime;
    type Interface = IWeb3StorageCalls;

    const MATCHER: AddressMatcher = match NonZero::new(0x0901) {
        Some(n) => AddressMatcher::Fixed(n),
        None => panic!("0x0901 is non-zero"),
    };
    const HAS_CONTRACT_INFO: bool = false;

    fn call(
        _address: &[u8; 20],
        input: &Self::Interface,
        env: &mut impl Ext<T = Self::T>,
    ) -> Result<Vec<u8>, Error> {
        // Every selector mutates pallet storage.
        if env.is_read_only() {
            return Err(Error::Error(
                pallet_revive::Error::<Runtime>::StateChangeDenied.into(),
            ));
        }

        let frame_origin = match env.caller() {
            Origin::Root => RawOrigin::Root.into(),
            Origin::Signed(account_id) => RawOrigin::Signed(account_id.clone()).into(),
        };

        match input {
            IWeb3StorageCalls::establishStorageAgreement(
                IWeb3Storage::establishStorageAgreementCall {
                    provider,
                    terms,
                    signature,
                    visibility,
                },
            ) => {
                env.charge(<Runtime as pallet_storage_provider::Config>::WeightInfo::establish_storage_agreement())?;
                let provider = decode_account::<Runtime>(&provider.0)?;
                let terms = decode_terms::<Runtime>(terms)?;
                let sig =
                    sp_runtime::MultiSignature::decode(&mut signature.as_ref()).map_err(|e| {
                        revert(
                            &e,
                            "Invalid signature encoding: expected SCALE-encoded MultiSignature",
                        )
                    })?;
                // `NextBucketId` is incremented inside the extrinsic; capture
                // the pre-dispatch value so we can return the id assigned to
                // this call.
                let bucket_id: BucketId = pallet_storage_provider::NextBucketId::<Runtime>::get();
                pallet_storage_provider::Pallet::<Runtime>::establish_storage_agreement(
                    frame_origin,
                    provider,
                    terms,
                    sig,
                    to_visibility(*visibility)?,
                )
                .map_err(|e| revert(&e, "establishStorageAgreement failed"))?;
                Ok(bucket_id.abi_encode())
            }

            IWeb3StorageCalls::freezeBucket(IWeb3Storage::freezeBucketCall { bucketId }) => {
                env.charge(
                    <Runtime as pallet_storage_provider::Config>::WeightInfo::freeze_bucket(),
                )?;
                pallet_storage_provider::Pallet::<Runtime>::freeze_bucket(frame_origin, *bucketId)
                    .map_err(|e| revert(&e, "freezeBucket failed"))?;
                Ok(Vec::new())
            }

            IWeb3StorageCalls::setBucketVisibility(IWeb3Storage::setBucketVisibilityCall {
                bucketId,
                visibility,
            }) => {
                env.charge(
                    <Runtime as pallet_storage_provider::Config>::WeightInfo::set_bucket_visibility(
                    ),
                )?;
                pallet_storage_provider::Pallet::<Runtime>::set_bucket_visibility(
                    frame_origin,
                    *bucketId,
                    to_visibility(*visibility)?,
                )
                .map_err(|e| revert(&e, "setBucketVisibility failed"))?;
                Ok(Vec::new())
            }

            IWeb3StorageCalls::setMember(IWeb3Storage::setMemberCall {
                bucketId,
                member,
                role,
            }) => {
                env.charge(
                    <Runtime as pallet_storage_provider::Config>::WeightInfo::set_bucket_member(),
                )?;
                let member = decode_account::<Runtime>(&member.0)?;
                let role = to_role(*role)?;
                pallet_storage_provider::Pallet::<Runtime>::set_member(
                    frame_origin,
                    *bucketId,
                    member,
                    role,
                )
                .map_err(|e| revert(&e, "setMember failed"))?;
                Ok(Vec::new())
            }

            IWeb3StorageCalls::removeMember(IWeb3Storage::removeMemberCall {
                bucketId,
                member,
            }) => {
                env.charge(
                    <Runtime as pallet_storage_provider::Config>::WeightInfo::remove_bucket_member(
                    ),
                )?;
                let member = decode_account::<Runtime>(&member.0)?;
                pallet_storage_provider::Pallet::<Runtime>::remove_member(
                    frame_origin,
                    *bucketId,
                    member,
                )
                .map_err(|e| revert(&e, "removeMember failed"))?;
                Ok(Vec::new())
            }

            IWeb3StorageCalls::topUpAgreement(IWeb3Storage::topUpAgreementCall {
                bucketId,
                provider,
                additionalBytes,
                maxPayment,
            }) => {
                env.charge(
                    <Runtime as pallet_storage_provider::Config>::WeightInfo::top_up_agreement(),
                )?;
                let provider = decode_account::<Runtime>(&provider.0)?;
                pallet_storage_provider::Pallet::<Runtime>::top_up_agreement(
                    frame_origin,
                    *bucketId,
                    provider,
                    *additionalBytes,
                    BalanceOf::<Runtime>::from(*maxPayment),
                )
                .map_err(|e| revert(&e, "topUpAgreement failed"))?;
                Ok(Vec::new())
            }

            IWeb3StorageCalls::extendAgreement(IWeb3Storage::extendAgreementCall {
                bucketId,
                provider,
                additionalDuration,
                maxPayment,
            }) => {
                env.charge(
                    <Runtime as pallet_storage_provider::Config>::WeightInfo::extend_agreement(),
                )?;
                let provider = decode_account::<Runtime>(&provider.0)?;
                pallet_storage_provider::Pallet::<Runtime>::extend_agreement(
                    frame_origin,
                    *bucketId,
                    provider,
                    BlockNumberFor::<Runtime>::from(*additionalDuration),
                    BalanceOf::<Runtime>::from(*maxPayment),
                )
                .map_err(|e| revert(&e, "extendAgreement failed"))?;
                Ok(Vec::new())
            }

            IWeb3StorageCalls::endAgreementPay(IWeb3Storage::endAgreementPayCall {
                bucketId,
                provider,
            }) => {
                // `end_agreement`'s WeightInfo takes `a` (admin-count modifier); 1 is conservative.
                env.charge(
                    <Runtime as pallet_storage_provider::Config>::WeightInfo::end_agreement(1),
                )?;
                let provider = decode_account::<Runtime>(&provider.0)?;
                pallet_storage_provider::Pallet::<Runtime>::end_agreement(
                    frame_origin,
                    *bucketId,
                    provider,
                    EndAction::Pay,
                )
                .map_err(|e| revert(&e, "endAgreementPay failed"))?;
                Ok(Vec::new())
            }

            IWeb3StorageCalls::endAgreementBurn(IWeb3Storage::endAgreementBurnCall {
                bucketId,
                provider,
                burnPercent,
            }) => {
                env.charge(
                    <Runtime as pallet_storage_provider::Config>::WeightInfo::end_agreement(1),
                )?;
                let provider = decode_account::<Runtime>(&provider.0)?;
                pallet_storage_provider::Pallet::<Runtime>::end_agreement(
                    frame_origin,
                    *bucketId,
                    provider,
                    EndAction::Burn {
                        burn_percent: *burnPercent,
                    },
                )
                .map_err(|e| revert(&e, "endAgreementBurn failed"))?;
                Ok(Vec::new())
            }

            IWeb3StorageCalls::challengeCheckpoint(IWeb3Storage::challengeCheckpointCall {
                bucketId,
                provider,
                leafIndex,
                chunkIndex,
            }) => {
                env.charge(
                    <Runtime as pallet_storage_provider::Config>::WeightInfo::challenge_checkpoint(
                    ),
                )?;
                let provider = decode_account::<Runtime>(&provider.0)?;
                pallet_storage_provider::Pallet::<Runtime>::challenge_checkpoint(
                    frame_origin,
                    *bucketId,
                    provider,
                    storage_primitives::ChunkLocation {
                        leaf_index: *leafIndex,
                        chunk_index: *chunkIndex,
                    },
                )
                .map_err(|e| revert(&e, "challengeCheckpoint failed"))?;
                Ok(Vec::new())
            }
        }
    }
}
