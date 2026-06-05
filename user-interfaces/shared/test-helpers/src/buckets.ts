import { Binary, Enum } from "polkadot-api";
import { getApi, submitExtrinsic, submitExtrinsicBestBlock } from "./chain-api";
import { Alice, type DevSigner } from "./signers";

// ─── Negotiate + signed-terms helpers ────────────────────────────────────────

const DEFAULT_PROVIDER_URL = "http://127.0.0.1:3333";
const DEFAULT_PROVIDER_ACCOUNT = Alice.address;

// SCALE variant ordering of sp_runtime::MultiSignature.
const MULTI_SIGNATURE_VARIANT: Record<number, string> = {
  0: "Ed25519",
  1: "Sr25519",
  2: "Ecdsa",
  3: "Eth",
};

interface NegotiateRequest {
  owner: string;
  max_bytes: number | bigint;
  duration: number;
  price_per_byte: number | bigint;
  replica_params: unknown | null;
  bucket_id: bigint | null;
}

interface SignedTerms {
  terms: {
    owner: string;
    max_bytes: number | bigint;
    duration: number;
    price_per_byte: number | bigint;
    valid_until: number;
    nonce: number | bigint;
    replica_params: unknown | null;
    bucket_id: bigint | null;
  };
  signature: string;
}

async function negotiateTerms(
  providerUrl: string,
  request: NegotiateRequest,
): Promise<SignedTerms> {
  const res = await fetch(`${providerUrl.replace(/\/$/, "")}/negotiate`, {
    method: "POST",
    headers: { "content-type": "application/json" },
    body: JSON.stringify(request, (_k, v) =>
      typeof v === "bigint" ? v.toString() : v,
    ),
  });
  if (!res.ok) {
    const body = await res.text().catch(() => "");
    throw new Error(`/negotiate failed: ${res.status} ${body}`);
  }
  return res.json();
}

function hexToBytes(hex: string): Uint8Array {
  const h = hex.startsWith("0x") ? hex.slice(2) : hex;
  const out = new Uint8Array(h.length / 2);
  for (let i = 0; i < out.length; i++) {
    out[i] = parseInt(h.substring(i * 2, i * 2 + 2), 16);
  }
  return out;
}

/**
 * Build the `{ provider, terms, sig }` args shared by every signed-terms
 * extrinsic. Mirrors `console-ui/src/lib/storage.ts::buildSignedTermsArgs`.
 *
 * The inner of `MultiSignature::Sr25519` is `[u8; 64]` which PAPI v2 encodes
 * as `SizedBytes(64) = Codec<string>` — pass a `0x`-prefixed hex string,
 * NOT a `Uint8Array`. (See PAPI v2 migration doc + console-ui's working
 * implementation.)
 */
function buildSignedTermsArgs(providerAccount: string, signed: SignedTerms) {
  const sigBytes = hexToBytes(signed.signature);
  if (sigBytes.length < 1) {
    throw new Error("signature too short to contain a MultiSignature variant byte");
  }
  const variantName = MULTI_SIGNATURE_VARIANT[sigBytes[0]];
  if (!variantName) {
    throw new Error(`unknown MultiSignature variant byte: ${sigBytes[0]}`);
  }
  const sigPayloadHex =
    "0x" +
    Array.from(sigBytes.slice(1))
      .map((b) => b.toString(16).padStart(2, "0"))
      .join("");
  // eslint-disable-next-line @typescript-eslint/no-explicit-any
  const sig = Enum(variantName as any, sigPayloadHex);

  const t = signed.terms;
  const terms = {
    owner: t.owner,
    max_bytes: BigInt(t.max_bytes),
    duration: t.duration,
    price_per_byte: BigInt(t.price_per_byte),
    valid_until: t.valid_until,
    nonce: BigInt(t.nonce),
    // eslint-disable-next-line @typescript-eslint/no-explicit-any
    replica_params: (t.replica_params ?? undefined) as any,
    bucket_id: t.bucket_id ?? undefined,
  };
  return { provider: providerAccount, terms, sig };
}

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
