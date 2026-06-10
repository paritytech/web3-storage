/** Thin typed wrappers around the DriveRegistry pallet (Layer 1 — file system). */

import { Enum } from "polkadot-api";

import type { ParachainApi } from "../address.js";
import type { ChainSigner } from "../signers.js";
import { requireOneEvent, submitTx } from "../tx.js";

export async function createDrive(
  api: ParachainApi,
  owner: ChainSigner,
  name: string,
  params: any,
) {
  const result = await submitTx(
    api.tx.DriveRegistry.create_drive({
      name: new TextEncoder().encode(name),
      ...params,
    }),
    owner.signer,
    "create_drive",
  );
  const event = requireOneEvent(
    result.events,
    api.event.DriveRegistry.DriveCreated,
    "DriveCreated",
  );
  const requested = api.event.StorageProvider.AgreementRequested.filter(
    result.events as never,
  );
  return {
    driveId: event.drive_id,
    bucketId: event.bucket_id,
    matchedProvider: requested[0]?.payload.provider,
  };
}

export async function shareDrive(
  api: ParachainApi,
  owner: ChainSigner,
  driveId: bigint,
  member: ChainSigner | { address: string },
  role: string,
) {
  const result = await submitTx(
    api.tx.DriveRegistry.share_drive({
      drive_id: driveId,
      member: member.address,
      role: Enum(role as never),
    }),
    owner.signer,
    `share_drive(${role})`,
  );
  return requireOneEvent(
    result.events,
    api.event.DriveRegistry.DriveShared,
    "DriveShared",
  );
}

export async function unshareDrive(
  api: ParachainApi,
  owner: ChainSigner,
  driveId: bigint,
  member: ChainSigner | { address: string },
) {
  const result = await submitTx(
    api.tx.DriveRegistry.unshare_drive({
      drive_id: driveId,
      member: member.address,
    }),
    owner.signer,
    "unshare_drive",
  );
  return requireOneEvent(
    result.events,
    api.event.DriveRegistry.DriveUnshared,
    "DriveUnshared",
  );
}

export async function deleteDrive(
  api: ParachainApi,
  owner: ChainSigner,
  driveId: bigint,
) {
  const result = await submitTx(
    api.tx.DriveRegistry.delete_drive({ drive_id: driveId }),
    owner.signer,
    "delete_drive",
  );
  return requireOneEvent(
    result.events,
    api.event.DriveRegistry.DriveDeleted,
    "DriveDeleted",
  );
}
