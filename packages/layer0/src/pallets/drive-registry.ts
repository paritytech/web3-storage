// SPDX-License-Identifier: Apache-2.0

/** Thin typed wrappers around the DriveRegistry pallet (Layer 1 — file system). */

import { Enum } from "polkadot-api";

import type { SignedTerms } from "@web3-storage/core";

import type { ParachainApi } from "../address.js";
import type { ChainSigner } from "../signers.js";
import { requireOneEvent, submitTx, type SubmitOpts } from "../tx.js";
import { buildSignedTermsArgs } from "./storage-provider.js";

/**
 * Create a drive by redeeming provider-signed terms (#105). Layer 0's
 * establish_storage_agreement_internal opens the underlying bucket + primary
 * agreement atomically inside create_drive, so `provider`/`signed` come from a
 * prior {@link negotiateTerms} against that provider's /negotiate endpoint.
 */
export async function createDrive(
  api: ParachainApi,
  owner: ChainSigner,
  name: string,
  provider: ChainSigner | { address: string },
  signed: SignedTerms,
  opts: SubmitOpts = {},
) {
  const result = await submitTx(
    api.tx.DriveRegistry.create_drive({
      name: new TextEncoder().encode(name),
      ...buildSignedTermsArgs(provider, signed),
    }),
    owner.signer,
    { label: "create_drive", ...opts },
  );
  const event = requireOneEvent(
    result.events,
    api.event.DriveRegistry.DriveCreated,
    "DriveCreated",
  );
  return {
    driveId: event.drive_id,
    bucketId: event.bucket_id,
    provider: provider.address,
  };
}

export async function shareDrive(
  api: ParachainApi,
  owner: ChainSigner,
  driveId: bigint,
  member: ChainSigner | { address: string },
  role: string,
  opts: SubmitOpts = {},
) {
  const result = await submitTx(
    api.tx.DriveRegistry.share_drive({
      drive_id: driveId,
      member: member.address,
      role: Enum(role as never),
    }),
    owner.signer,
    { label: `share_drive(${role})`, ...opts },
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
  opts: SubmitOpts = {},
) {
  const result = await submitTx(
    api.tx.DriveRegistry.unshare_drive({
      drive_id: driveId,
      member: member.address,
    }),
    owner.signer,
    { label: "unshare_drive", ...opts },
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
  opts: SubmitOpts = {},
) {
  const result = await submitTx(
    api.tx.DriveRegistry.delete_drive({ drive_id: driveId }),
    owner.signer,
    { label: "delete_drive", ...opts },
  );
  return requireOneEvent(
    result.events,
    api.event.DriveRegistry.DriveDeleted,
    "DriveDeleted",
  );
}
