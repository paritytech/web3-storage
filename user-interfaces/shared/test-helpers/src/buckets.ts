// SPDX-License-Identifier: GPL-3.0-only

import { Binary } from "polkadot-api";
import { buildSignedTermsArgs, negotiateTerms } from "@web3-storage/papi";
import { getApi, submitExtrinsic, submitExtrinsicBestBlock } from "./chain-api";
import { Alice, type DevSigner } from "./signers";

// ─── Negotiate + signed-terms helpers ────────────────────────────────────────

const DEFAULT_PROVIDER_URL = "http://127.0.0.1:3333";
const DEFAULT_PROVIDER_ACCOUNT = Alice.address;

// ─── S3 Buckets (console-ui) ─────────────────────────────────────────────────

export interface CreateBucketOptions {
  name: string;
  /** Provider HTTP base URL for the /negotiate call. Defaults to env or localhost. */
  providerUrl?: string;
  /** Provider on-chain account. Defaults to the signer (CI's Alice is both owner and provider). */
  providerAccount?: string;
  /** Storage capacity in bytes negotiated with the provider. Defaults to 10 MiB. */
  maxBytes?: bigint;
  /** Agreement duration in blocks. Defaults to 10,000. */
  duration?: number;
  /** Price per byte per block. Defaults to 0 (test fixture). */
  pricePerByte?: bigint;
}

export interface BucketHandle {
  s3BucketId: bigint;
  layer0BucketId: bigint;
  name: string;
}

export async function createBucketViaApi(
  signer: DevSigner,
  opts: CreateBucketOptions,
): Promise<BucketHandle> {
  const api = getApi();
  const providerUrl = opts.providerUrl ?? DEFAULT_PROVIDER_URL;
  const providerAccount = opts.providerAccount ?? DEFAULT_PROVIDER_ACCOUNT;

  const signed = await negotiateTerms(providerUrl, {
    owner: signer.address,
    max_bytes: opts.maxBytes ?? 10_485_760n,
    duration: opts.duration ?? 10_000,
    price_per_byte: opts.pricePerByte ?? 0n,
    replica_params: null,
    bucket_id: null,
  });

  const result = await submitExtrinsic(
    api.tx.S3Registry.create_s3_bucket({
      name: Binary.fromText(opts.name),
      ...buildSignedTermsArgs(providerAccount, signed),
    }),
    signer.signer,
  );

  const events = api.event.S3Registry.S3BucketCreated.filter(result.events as never);
  if (events.length === 0) {
    throw new Error("S3BucketCreated event not found");
  }
  const { s3_bucket_id, layer0_bucket_id } = events[0].payload;
  return { s3BucketId: s3_bucket_id, layer0BucketId: layer0_bucket_id, name: opts.name };
}

export async function deleteBucketViaApi(signer: DevSigner, s3BucketId: bigint): Promise<void> {
  const api = getApi();
  await submitExtrinsicBestBlock(
    api.tx.S3Registry.delete_s3_bucket({ s3_bucket_id: s3BucketId }),
    signer.signer,
  );
}

/**
 * Delete all object metadata in a bucket. The runtime rejects
 * `delete_s3_bucket` while `object_count > 0`, so cleanup paths must
 * drain the bucket first.
 */
async function purgeBucketObjects(signer: DevSigner, s3BucketId: bigint): Promise<void> {
  const api = getApi();
  const entries = await api.query.S3Registry.Objects.getEntries(s3BucketId);
  for (const { keyArgs } of entries) {
    await submitExtrinsicBestBlock(
      api.tx.S3Registry.delete_object_metadata({
        s3_bucket_id: s3BucketId,
        key: keyArgs[1],
      }),
      signer.signer,
    );
  }
}

export async function cleanupBuckets(signer: DevSigner): Promise<number> {
  const api = getApi();
  const bucketIds = await api.query.S3Registry.UserBuckets.getValue(signer.address);
  if (!bucketIds || bucketIds.length === 0) return 0;
  let deleted = 0;
  for (const id of bucketIds) {
    try {
      await purgeBucketObjects(signer, id);
      await deleteBucketViaApi(signer, id);
      deleted++;
    } catch {
      // ignore — best-effort cleanup
    }
  }
  return deleted;
}

// ─── Drives (drive-ui) ───────────────────────────────────────────────────────

export interface CreateDriveOptions {
  name?: string;
  /** Provider HTTP base URL for the /negotiate call. Defaults to env or localhost. */
  providerUrl?: string;
  /** Provider on-chain account. Defaults to the signer. */
  providerAccount?: string;
  /** Storage capacity in bytes. Defaults to 10 MiB. */
  maxCapacity?: bigint;
  /** Agreement duration in blocks. Defaults to 10,000. */
  storagePeriod?: number;
  /** Price per byte per block. Defaults to 0. */
  pricePerByte?: bigint;
}

export interface DriveHandle {
  driveId: bigint;
  bucketId: bigint;
  name: string | undefined;
}

export async function createDriveViaApi(
  signer: DevSigner,
  opts: CreateDriveOptions = {},
): Promise<DriveHandle> {
  const api = getApi();
  const providerUrl = opts.providerUrl ?? DEFAULT_PROVIDER_URL;
  const providerAccount = opts.providerAccount ?? signer.address;

  const signed = await negotiateTerms(providerUrl, {
    owner: signer.address,
    max_bytes: opts.maxCapacity ?? 10_485_760n,
    duration: opts.storagePeriod ?? 10_000,
    price_per_byte: opts.pricePerByte ?? 0n,
    replica_params: null,
    bucket_id: null,
  });

  const nameBytes = opts.name ? Binary.fromText(opts.name) : undefined;
  const result = await submitExtrinsic(
    api.tx.DriveRegistry.create_drive({
      name: nameBytes,
      ...buildSignedTermsArgs(providerAccount, signed),
    }),
    signer.signer,
  );

  const created = api.event.DriveRegistry.DriveCreated.filter(result.events as never);
  if (created.length === 0) throw new Error("DriveCreated event not found");
  const { drive_id, bucket_id } = created[0].payload;
  return { driveId: drive_id, bucketId: bucket_id, name: opts.name };
}

export async function deleteDriveViaApi(signer: DevSigner, driveId: bigint): Promise<void> {
  const api = getApi();
  await submitExtrinsicBestBlock(
    api.tx.DriveRegistry.delete_drive({ drive_id: driveId }),
    signer.signer,
  );
}

export async function cleanupDrives(signer: DevSigner): Promise<number> {
  const api = getApi();
  const driveIds = await api.query.DriveRegistry.UserDrives.getValue(signer.address);
  if (!driveIds || driveIds.length === 0) return 0;
  let deleted = 0;
  for (const id of driveIds) {
    try {
      await deleteDriveViaApi(signer, id);
      deleted++;
    } catch {
      // ignore — best-effort cleanup
    }
  }
  return deleted;
}
