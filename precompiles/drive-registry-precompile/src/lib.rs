//! `pallet_revive` precompile exposing `pallet_drive_registry` to Solidity
//! contracts.
//!
//! Address: `0x0000000000000000000000000000000009020000` (matcher
//! `Fixed(0x0902)`; same address layout as the storage-provider precompile —
//! the u16 sits at bytes 16-17 with a `0x0000` suffix at bytes 18-19).

#![cfg_attr(not(feature = "std"), no_std)]

extern crate alloc;

use alloc::vec::Vec;
use codec::Decode;
use core::{fmt, marker::PhantomData, num::NonZero};
use frame_support::dispatch::RawOrigin;
use frame_system::pallet_prelude::BlockNumberFor;
use pallet_drive_registry::WeightInfo;
use pallet_revive::{
    precompiles::{
        alloy::{self, sol_types::SolValue},
        AddressMatcher, Error, Ext, Precompile,
    },
    ExecOrigin as Origin,
};
use pallet_storage_provider::BalanceOf;
use storage_primitives::Role;
use tracing::error;

alloy::sol!("src/IDriveRegistry.sol");
use IDriveRegistry::IDriveRegistryCalls;

const LOG_TARGET: &str = "web3-storage::drive-registry-precompile";

fn revert(error: &impl fmt::Debug, message: &str) -> Error {
    error!(target: LOG_TARGET, ?error, "{}", message);
    Error::Revert(message.into())
}

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

fn decode_role(tag: u8) -> Result<Role, Error> {
    match tag {
        0 => Ok(Role::Admin),
        1 => Ok(Role::Writer),
        2 => Ok(Role::Reader),
        other => Err(revert(
            &other,
            "Invalid role tag (expected 0=Admin, 1=Writer, 2=Reader)",
        )),
    }
}

/// Precompile wrapping `pallet_drive_registry`'s public extrinsics.
pub struct DriveRegistryPrecompile<T>(PhantomData<T>);

impl<Runtime> Precompile for DriveRegistryPrecompile<Runtime>
where
    Runtime:
        pallet_drive_registry::Config + pallet_storage_provider::Config + pallet_revive::Config,
    BalanceOf<Runtime>: From<u128>,
    BlockNumberFor<Runtime>: From<u32>,
{
    type T = Runtime;
    type Interface = IDriveRegistryCalls;

    const MATCHER: AddressMatcher = match NonZero::new(0x0902) {
        Some(n) => AddressMatcher::Fixed(n),
        None => panic!("0x0902 is non-zero"),
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
            IDriveRegistryCalls::createDrive(IDriveRegistry::createDriveCall {
                name,
                maxCapacity,
                storagePeriod,
                payment,
                minProviders,
            }) => {
                env.charge(<Runtime as pallet_drive_registry::Config>::WeightInfo::create_drive())?;
                let drive_id = pallet_drive_registry::NextDriveId::<Runtime>::get();
                let name_opt = if name.is_empty() {
                    None
                } else {
                    Some(name.as_bytes().to_vec())
                };
                let min_providers_opt = if *minProviders == 0 {
                    None
                } else {
                    Some(*minProviders)
                };
                pallet_drive_registry::Pallet::<Runtime>::create_drive(
                    frame_origin,
                    name_opt,
                    *maxCapacity,
                    BlockNumberFor::<Runtime>::from(*storagePeriod),
                    BalanceOf::<Runtime>::from(*payment),
                    min_providers_opt,
                )
                .map_err(|e| revert(&e, "createDrive failed"))?;
                Ok(drive_id.abi_encode())
            }

            IDriveRegistryCalls::deleteDrive(IDriveRegistry::deleteDriveCall { driveId }) => {
                env.charge(<Runtime as pallet_drive_registry::Config>::WeightInfo::delete_drive())?;
                pallet_drive_registry::Pallet::<Runtime>::delete_drive(frame_origin, *driveId)
                    .map_err(|e| revert(&e, "deleteDrive failed"))?;
                Ok(Vec::new())
            }

            IDriveRegistryCalls::shareDrive(IDriveRegistry::shareDriveCall {
                driveId,
                member,
                role,
            }) => {
                env.charge(<Runtime as pallet_drive_registry::Config>::WeightInfo::share_drive())?;
                let member = decode_account::<Runtime>(&member.0)?;
                let role = decode_role(*role)?;
                pallet_drive_registry::Pallet::<Runtime>::share_drive(
                    frame_origin,
                    *driveId,
                    member,
                    role,
                )
                .map_err(|e| revert(&e, "shareDrive failed"))?;
                Ok(Vec::new())
            }

            IDriveRegistryCalls::unshareDrive(IDriveRegistry::unshareDriveCall {
                driveId,
                member,
            }) => {
                env.charge(
                    <Runtime as pallet_drive_registry::Config>::WeightInfo::unshare_drive(),
                )?;
                let member = decode_account::<Runtime>(&member.0)?;
                pallet_drive_registry::Pallet::<Runtime>::unshare_drive(
                    frame_origin,
                    *driveId,
                    member,
                )
                .map_err(|e| revert(&e, "unshareDrive failed"))?;
                Ok(Vec::new())
            }
        }
    }
}
