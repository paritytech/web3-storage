// SPDX-License-Identifier: Apache-2.0

import { waitForPrimaryProvider } from "@web3-storage/sdk";
import { S3Client } from "@web3-storage/sdk/s3";
import { FileSystemClient } from "@web3-storage/sdk/fs";
import { getApi, submitExtrinsicBestBlock } from "./chain-api";
import type { DevSigner } from "./signers";

// Dev provider HTTP endpoint. The local provider node registers its multiaddr
// as /ip4/127.0.0.1/tcp/3333, so the SDK clients could resolve this from chain
// — but pinning it skips that lookup and matches how the UIs target the local
// provider. Overridable for non-default test setups.
const DEV_PROVIDER_URL = process.env.PROVIDER_URL ?? "http://127.0.0.1:3333";

// Sensible defaults for test fixtures. Large enough to satisfy provider
// capacity/duration checks, small enough to stay well under the dev stake.
const DEFAULT_MAX_BYTES = 10_000_000n;
const DEFAULT_DURATION = 10_000;

// ─── S3 Buckets ─────────────────────────────────────────────────────────────

export interface CreateBucketOptions {
  name: string;
  /** Bytes to reserve. Default 10 MB. */
  maxBytes?: bigint;
  /** Agreement duration in blocks. Default 10_000. */
  duration?: number;
  /** Max price per byte per block to accept from the provider. */
  pricePerByte?: bigint;
}

export interface BucketHandle {
  s3BucketId: bigint;
  layer0BucketId: bigint;
  name: string;
}

/**
 * Create an S3 bucket via the negotiate → atomic establish flow: the SDK's
 * S3Client auto-discovers the accepting dev provider, POSTs /negotiate for
 * signed terms, then submits `create_s3_bucket(name, provider, terms, sig)`.
 * Finalized submission (test-setup semantics) — the client defaults to
 * `submitMode: "finalized"`. `pricePerByte` only caps acceptance; the
 * provider signs its own listed price.
 */
export async function createBucketViaApi(
  signer: DevSigner,
  opts: CreateBucketOptions,
): Promise<BucketHandle> {
  const client = new S3Client({
    api: getApi(),
    signer,
    providerUrl: DEV_PROVIDER_URL,
  });
  const { s3BucketId, layer0BucketId } = await client.createBucket(opts.name, {
    maxCapacity: opts.maxBytes ?? DEFAULT_MAX_BYTES,
    duration: opts.duration ?? DEFAULT_DURATION,
  });
  return { s3BucketId, layer0BucketId, name: opts.name };
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
  /** Bytes to reserve. Default 10 MB. Alias: `maxBytes`. */
  maxCapacity?: bigint;
  maxBytes?: bigint;
  /** Agreement duration in blocks. Default 10_000. Alias: `duration`. */
  storagePeriod?: number;
  duration?: number;
  /** Max price per byte per block to accept from the provider. */
  pricePerByte?: bigint;
}

export interface DriveHandle {
  driveId: bigint;
  bucketId: bigint;
  name: string | undefined;
}

/**
 * Create a drive via the negotiate → atomic establish flow: the SDK's
 * FileSystemClient auto-discovers the accepting dev provider, POSTs /negotiate
 * for signed terms, then submits `create_drive(name, provider, terms, sig)`.
 * Finalized submission (test-setup semantics) — the client defaults to
 * `submitMode: "finalized"`. With the atomic establish the provider is primary
 * immediately, so the `waitForPrimaryProvider` below resolves fast; it's kept
 * as a guard against a misconfigured provider node.
 */
export async function createDriveViaApi(
  signer: DevSigner,
  opts: CreateDriveOptions,
): Promise<DriveHandle> {
  const api = getApi();
  const client = new FileSystemClient({ api, signer, providerUrl: DEV_PROVIDER_URL });

  const { driveId, bucketId } = await client.createDrive({
    name: opts.name,
    maxCapacity: opts.maxCapacity ?? opts.maxBytes ?? DEFAULT_MAX_BYTES,
    storagePeriod: opts.storagePeriod ?? opts.duration ?? DEFAULT_DURATION,
  });
  const handle: DriveHandle = { driveId, bucketId, name: opts.name };

  try {
    await waitForPrimaryProvider(api, handle.bucketId, { timeoutMs: 90_000 });
  } catch (e) {
    throw new Error(
      `createDriveViaApi: ${(e as Error).message} — provider node may not be running or not accepting agreements.`,
    );
  }
  return handle;
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
