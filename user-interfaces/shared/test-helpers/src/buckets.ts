import { getApi, submitExtrinsic, submitExtrinsicBestBlock } from "./chain-api";
import type { DevSigner } from "./signers";

// ─── S3 Buckets (console-ui) ─────────────────────────────────────────────────

export interface CreateBucketOptions {
  name: string;
  minProviders?: number;
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
  const nameBytes = new TextEncoder().encode(opts.name);
  const result = await submitExtrinsic(
    api.tx.S3Registry.create_s3_bucket({
      name: nameBytes,
      min_providers: opts.minProviders ?? 1,
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
  maxCapacity: bigint;
  storagePeriod: number;
  payment: bigint;
  minProviders?: number;
}

export interface DriveHandle {
  driveId: bigint;
  bucketId: bigint;
  name: string | undefined;
}

export async function createDriveViaApi(
  signer: DevSigner,
  opts: CreateDriveOptions,
): Promise<DriveHandle> {
  const api = getApi();

  const nameBytes = opts.name ? new TextEncoder().encode(opts.name) : undefined;
  const result = await submitExtrinsic(
    api.tx.DriveRegistry.create_drive({
      name: nameBytes,
      max_capacity: opts.maxCapacity,
      storage_period: opts.storagePeriod,
      payment: opts.payment,
      min_providers: opts.minProviders ?? undefined,
    }),
    signer.signer,
  );

  const created = api.event.DriveRegistry.DriveCreated.filter(result.events as never);
  if (created.length === 0) throw new Error("DriveCreated event not found");
  const { drive_id, bucket_id } = created[0].payload;
  const handle: DriveHandle = { driveId: drive_id, bucketId: bucket_id, name: opts.name };

  // create_drive auto-emits a request_agreement targeting the matched
  // provider. Only the provider can call accept_agreement (the drive owner
  // can't), so we wait for the provider node's auto-coordinator to settle
  // it. The coordinator polls every ~6s; accept_agreement finalizes in
  // ~12-24s — typical end-to-end is 30-36s, worst case ~50s under nonce
  // contention from rapid prior cleanup. 90s absorbs the worst case and
  // adds <30s to the worst-case test (which we'd happily pay vs. a flake).
  const start = Date.now();
  const timeoutMs = 90_000;
  while (Date.now() - start < timeoutMs) {
    const bucket = await api.query.StorageProvider.Buckets.getValue(handle.bucketId);
    if (bucket && bucket.primary_providers.length > 0) return handle;
    await new Promise((r) => setTimeout(r, 1500));
  }
  throw new Error(
    `createDriveViaApi: bucket ${handle.bucketId} primary_providers stayed empty after ${timeoutMs}ms — provider node may not be running or not auto-accepting.`,
  );
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
