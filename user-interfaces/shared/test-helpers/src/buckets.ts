import { Enum } from "@polkadot-api/substrate-bindings";
import { getApi, submitExtrinsic } from "./chain-api";
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
  await submitExtrinsic(
    api.tx.S3Registry.delete_s3_bucket({ s3_bucket_id: s3BucketId }),
    signer.signer,
  );
}

export async function cleanupBuckets(signer: DevSigner): Promise<number> {
  const api = getApi();
  const bucketIds = await api.query.S3Registry.UserBuckets.getValue(signer.address);
  if (!bucketIds || bucketIds.length === 0) return 0;
  let deleted = 0;
  for (const id of bucketIds) {
    try {
      await deleteBucketViaApi(signer, id);
      deleted++;
    } catch {
      // ignore — best-effort cleanup
    }
  }
  return deleted;
}

// ─── Drives (drive-ui) ───────────────────────────────────────────────────────

export type CommitStrategy =
  | { type: "Immediate" }
  | { type: "Batched"; interval: number }
  | { type: "Manual" };

export interface CreateDriveOptions {
  name?: string;
  maxCapacity: bigint;
  storagePeriod: number;
  payment: bigint;
  minProviders?: number;
  commitStrategy?: CommitStrategy;
}

export interface DriveHandle {
  driveId: bigint;
  bucketId: bigint;
  name: string | undefined;
}

function encodeCommitStrategy(s: CommitStrategy): unknown {
  switch (s.type) {
    case "Immediate":
      return Enum("Immediate");
    case "Manual":
      return Enum("Manual");
    case "Batched":
      return Enum("Batched", { interval: s.interval });
  }
}

export async function createDriveViaApi(
  signer: DevSigner,
  opts: CreateDriveOptions,
): Promise<DriveHandle> {
  const api = getApi();
  const strategy = opts.commitStrategy ?? { type: "Batched", interval: 100 };

  const nameBytes = opts.name ? new TextEncoder().encode(opts.name) : undefined;
  const result = await submitExtrinsic(
    api.tx.DriveRegistry.create_drive({
      name: nameBytes,
      max_capacity: opts.maxCapacity,
      storage_period: opts.storagePeriod,
      payment: opts.payment,
      min_providers: opts.minProviders ?? undefined,
      commit_strategy: encodeCommitStrategy(strategy) as never,
    }),
    signer.signer,
  );

  const created = api.event.DriveRegistry.DriveCreated.filter(result.events as never);
  if (created.length > 0) {
    const { drive_id, bucket_id } = created[0].payload;
    return { driveId: drive_id, bucketId: bucket_id, name: opts.name };
  }
  const createdWithStorage = api.event.DriveRegistry.DriveCreatedWithStorage.filter(
    result.events as never,
  );
  if (createdWithStorage.length > 0) {
    const { drive_id, bucket_id } = createdWithStorage[0].payload;
    return { driveId: drive_id, bucketId: bucket_id, name: opts.name };
  }
  throw new Error("DriveCreated event not found");
}

export async function deleteDriveViaApi(signer: DevSigner, driveId: bigint): Promise<void> {
  const api = getApi();
  await submitExtrinsic(
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
