//! `pallet_revive` precompile exposing `pallet_s3_registry` to Solidity
//! contracts.
//!
//! Address: `0x0000000000000000000000000000000009030000` (matcher
//! `Fixed(0x0903)`; same address layout as the storage-provider and
//! drive-registry precompiles — the u16 sits at bytes 16-17 with a
//! `0x0000` suffix at bytes 18-19).

#![cfg_attr(not(feature = "std"), no_std)]

extern crate alloc;

use alloc::{vec, vec::Vec};
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
use pallet_s3_registry::WeightInfo;
use pallet_storage_provider::BalanceOf;
use sp_core::H256;
use tracing::error;

alloy::sol!("src/IS3Registry.sol");
use IS3Registry::IS3RegistryCalls;

const LOG_TARGET: &str = "web3-storage::s3-registry-precompile";

fn revert(error: &impl fmt::Debug, message: &str) -> Error {
    error!(target: LOG_TARGET, ?error, "{}", message);
    Error::Revert(message.into())
}

/// Precompile wrapping `pallet_s3_registry`'s public extrinsics.
pub struct S3RegistryPrecompile<T>(PhantomData<T>);

impl<Runtime> Precompile for S3RegistryPrecompile<Runtime>
where
    Runtime: pallet_s3_registry::Config + pallet_storage_provider::Config + pallet_revive::Config,
    BalanceOf<Runtime>: From<u128>,
    BlockNumberFor<Runtime>: From<u32>,
{
    type T = Runtime;
    type Interface = IS3RegistryCalls;

    const MATCHER: AddressMatcher = match NonZero::new(0x0903) {
        Some(n) => AddressMatcher::Fixed(n),
        None => panic!("0x0903 is non-zero"),
    };
    const HAS_CONTRACT_INFO: bool = false;

    fn call(
        _address: &[u8; 20],
        input: &Self::Interface,
        env: &mut impl Ext<T = Self::T>,
    ) -> Result<Vec<u8>, Error> {
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
            IS3RegistryCalls::createS3Bucket(IS3Registry::createS3BucketCall {
                name,
                minProviders,
            }) => {
                env.charge(
                    <Runtime as pallet_s3_registry::Config>::WeightInfo::create_s3_bucket(),
                )?;
                let s3_bucket_id = pallet_s3_registry::NextS3BucketId::<Runtime>::get();
                pallet_s3_registry::Pallet::<Runtime>::create_s3_bucket(
                    frame_origin,
                    name.as_bytes().to_vec(),
                    *minProviders,
                )
                .map_err(|e| revert(&e, "createS3Bucket failed"))?;
                Ok(s3_bucket_id.abi_encode())
            }

            IS3RegistryCalls::createS3BucketWithStorage(
                IS3Registry::createS3BucketWithStorageCall {
                    name,
                    maxCapacity,
                    duration,
                    maxPayment,
                },
            ) => {
                env.charge(<Runtime as pallet_s3_registry::Config>::WeightInfo::create_s3_bucket_with_storage())?;
                let s3_bucket_id = pallet_s3_registry::NextS3BucketId::<Runtime>::get();
                pallet_s3_registry::Pallet::<Runtime>::create_s3_bucket_with_storage(
                    frame_origin,
                    name.as_bytes().to_vec(),
                    *maxCapacity,
                    BlockNumberFor::<Runtime>::from(*duration),
                    BalanceOf::<Runtime>::from(*maxPayment),
                )
                .map_err(|e| revert(&e, "createS3BucketWithStorage failed"))?;
                Ok(s3_bucket_id.abi_encode())
            }

            IS3RegistryCalls::deleteS3Bucket(IS3Registry::deleteS3BucketCall { s3BucketId }) => {
                env.charge(
                    <Runtime as pallet_s3_registry::Config>::WeightInfo::delete_s3_bucket(),
                )?;
                pallet_s3_registry::Pallet::<Runtime>::delete_s3_bucket(frame_origin, *s3BucketId)
                    .map_err(|e| revert(&e, "deleteS3Bucket failed"))?;
                Ok(Vec::new())
            }

            IS3RegistryCalls::putObjectMetadata(IS3Registry::putObjectMetadataCall {
                s3BucketId,
                key,
                cid,
                size,
                contentType,
            }) => {
                env.charge(
                    <Runtime as pallet_s3_registry::Config>::WeightInfo::put_object_metadata(),
                )?;
                pallet_s3_registry::Pallet::<Runtime>::put_object_metadata(
                    frame_origin,
                    *s3BucketId,
                    key.as_bytes().to_vec(),
                    H256::from_slice(&cid.0),
                    *size,
                    contentType.as_bytes().to_vec(),
                    vec![],
                )
                .map_err(|e| revert(&e, "putObjectMetadata failed"))?;
                Ok(Vec::new())
            }

            IS3RegistryCalls::deleteObjectMetadata(IS3Registry::deleteObjectMetadataCall {
                s3BucketId,
                key,
            }) => {
                env.charge(
                    <Runtime as pallet_s3_registry::Config>::WeightInfo::delete_object_metadata(),
                )?;
                pallet_s3_registry::Pallet::<Runtime>::delete_object_metadata(
                    frame_origin,
                    *s3BucketId,
                    key.as_bytes().to_vec(),
                )
                .map_err(|e| revert(&e, "deleteObjectMetadata failed"))?;
                Ok(Vec::new())
            }

            IS3RegistryCalls::copyObjectMetadata(IS3Registry::copyObjectMetadataCall {
                srcBucketId,
                srcKey,
                dstBucketId,
                dstKey,
            }) => {
                env.charge(
                    <Runtime as pallet_s3_registry::Config>::WeightInfo::copy_object_metadata(),
                )?;
                pallet_s3_registry::Pallet::<Runtime>::copy_object_metadata(
                    frame_origin,
                    *srcBucketId,
                    srcKey.as_bytes().to_vec(),
                    *dstBucketId,
                    dstKey.as_bytes().to_vec(),
                )
                .map_err(|e| revert(&e, "copyObjectMetadata failed"))?;
                Ok(Vec::new())
            }
        }
    }
}
