// SPDX-License-Identifier: GPL-3.0-only

import { waitForPrimaryProvider } from "@web3-storage/sdk";
import { S3Client } from "@web3-storage/sdk/s3";
import { getApi } from "./chain-api";
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

// Drives and S3 buckets are both plain Layer 0 buckets. The chain has no
// bucket deletion (see the "Bucket without providers" section of the
// implementation design doc), so fixtures accumulate across runs: tests
// look up the bucket ids they created instead of expecting an empty list.

export interface CreateBucketOptions {
  /** Bytes to reserve. Default 10 MB. Alias: `maxCapacity`. */
  maxBytes?: bigint;
  maxCapacity?: bigint;
  /** Agreement duration in blocks. Default 10_000. Alias: `storagePeriod`. */
  duration?: number;
  storagePeriod?: number;
  /** Read visibility of the bucket (default Private). */
  visibility?: "Public" | "Private";
}

export interface BucketHandle {
  bucketId: bigint;
}

/**
 * Create a bucket via the negotiate → atomic establish flow: the SDK client
 * auto-discovers the accepting dev provider, POSTs /negotiate for signed
 * terms, then submits `create_bucket_with_primary`. Finalized submission
 * (test-setup semantics). The trailing `waitForPrimaryProvider` guards
 * against a misconfigured provider node.
 */
export async function createBucketViaApi(
  signer: DevSigner,
  opts: CreateBucketOptions = {},
): Promise<BucketHandle> {
  const api = getApi();
  const client = new S3Client({ api, signer, providerUrl: DEV_PROVIDER_URL });
  const { bucketId } = await client.createBucket({
    maxCapacity: opts.maxCapacity ?? opts.maxBytes ?? DEFAULT_MAX_BYTES,
    duration: opts.storagePeriod ?? opts.duration ?? DEFAULT_DURATION,
    visibility: opts.visibility,
  });
  try {
    await waitForPrimaryProvider(api, bucketId, { timeoutMs: 90_000 });
  } catch (e) {
    throw new Error(
      `createBucketViaApi: ${(e as Error).message} — provider node may not be running or not accepting agreements.`,
    );
  }
  return { bucketId };
}

/** A drive is a bucket; same fixture as {@link createBucketViaApi}. */
export const createDriveViaApi = createBucketViaApi;
export type CreateDriveOptions = CreateBucketOptions;
export type DriveHandle = BucketHandle;
