/**
 * S3Client — S3Registry buckets/object-metadata (chain) + the provider
 * node's /s3 object HTTP surface. Chain ops delegate to the layer-0 pallet
 * wrappers (silent, in-block, no auto-retry); HTTP ops go through core's
 * retrying fetch and are signed when the signer has a raw keypair.
 *
 * Bytes are opaque here: client-side encryption (when used) wraps/unwraps
 * app-side, so CID verification covers exactly what the provider stores.
 */

import {
  computeCid,
  CidMismatchError,
  DEFAULT_CHUNK_SIZE,
  httpFetch,
  signProviderRequest,
  toHex,
  type HttpFetchOpts,
} from "@web3-storage/core";
import {
  asHex,
  createS3BucketWithStorage as createS3BucketWithStorageTx,
  deleteObjectMetadata as deleteObjectMetadataTx,
  deleteS3Bucket as deleteS3BucketTx,
  putObjectMetadata as putObjectMetadataTx,
  requireOneEvent,
  submitTx,
  READ_OPTS,
  type ChainSigner,
  type ParachainApi,
  type SubmitOpts,
  type TxStatusListener,
  type WaitOpts,
} from "@web3-storage/layer0";

import { ProviderUrlResolver } from "../provider-url.js";
import type {
  BucketInfo,
  BucketRef,
  CreateBucketOptions,
  CreateBucketWithStorageOptions,
  GetObjectResponse,
  ObjectMetadata,
  ObjectSummary,
  PutObjectOptions,
  PutObjectResult,
} from "./types.js";

export interface S3ClientOptions {
  api: ParachainApi;
  signer?: ChainSigner | null;
  /** Explicit provider URL (dev/tests) — skips on-chain resolution. */
  providerUrl?: string;
  /** Injection point for unit tests. */
  fetch?: typeof fetch;
  /** Tx progress listener. Default null (silent) — apps drive their own UI. */
  onStatus?: TxStatusListener | null;
}

const utf8 = (s: string) => new TextEncoder().encode(s);

export class S3Client {
  private readonly api: ParachainApi;
  private signer: ChainSigner | null;
  private readonly providers: ProviderUrlResolver;
  private readonly fetchOpts: HttpFetchOpts;
  private readonly onStatus: TxStatusListener | null;

  constructor(opts: S3ClientOptions) {
    this.api = opts.api;
    this.signer = opts.signer ?? null;
    this.providers = new ProviderUrlResolver(opts.api, opts.providerUrl);
    this.fetchOpts = opts.fetch ? { fetchImpl: opts.fetch } : {};
    this.onStatus = opts.onStatus ?? null;
  }

  setSigner(signer: ChainSigner | null): void {
    this.signer = signer;
  }

  private requireSigner(): ChainSigner {
    if (!this.signer) throw new Error("Signer not set");
    return this.signer;
  }

  private submitOpts(): SubmitOpts {
    return { retryStale: 0, onStatus: this.onStatus };
  }

  private authHeaders(method: string, layer0BucketId: bigint): Record<string, string> {
    const kp = this.signer?.keypair;
    return kp ? signProviderRequest(kp, method, layer0BucketId) : {};
  }

  // ── Validation (S3 conventions, lifted from the original SDK stub) ──────

  validateBucketName(name: string): void {
    if (name.length < 3 || name.length > 63) {
      throw new Error("Bucket name must be 3-63 characters");
    }
    if (!/^[a-z0-9]([a-z0-9-]*[a-z0-9])?$/.test(name)) {
      throw new Error(
        "Bucket name must be lowercase alphanumeric with optional inner hyphens",
      );
    }
  }

  validateObjectKey(key: string): void {
    if (key.length < 1 || key.length > 1024) {
      throw new Error("Object key must be 1-1024 characters");
    }
  }

  // ── Bucket chain ops ────────────────────────────────────────────────────

  /** Create a bucket without a storage agreement (agreement requested separately). */
  async createBucket(name: string, opts: CreateBucketOptions = {}): Promise<BucketInfo> {
    this.validateBucketName(name);
    const signer = this.requireSigner();
    const result = await submitTx(
      this.api.tx.S3Registry.create_s3_bucket({
        name: utf8(name),
        min_providers: opts.minProviders ?? 1,
      }),
      signer.signer,
      { label: "create_s3_bucket", ...this.submitOpts() },
    );
    const ev = requireOneEvent(
      result.events,
      this.api.event.S3Registry.S3BucketCreated,
      "S3BucketCreated",
    );
    return {
      s3BucketId: ev.s3_bucket_id,
      layer0BucketId: ev.layer0_bucket_id,
      name,
      owner: signer.address,
      createdAt: 0,
      objectCount: 0,
    };
  }

  /** Create a bucket atomically with a storage agreement (auto-matched provider). */
  async createBucketWithStorage(
    name: string,
    opts: CreateBucketWithStorageOptions,
  ): Promise<{ s3BucketId: bigint; layer0BucketId: bigint; matchedProvider?: string }> {
    this.validateBucketName(name);
    return createS3BucketWithStorageTx(
      this.api,
      this.requireSigner(),
      name,
      {
        max_capacity: opts.maxCapacity,
        duration: opts.duration,
        max_payment: opts.maxPayment,
      },
      this.submitOpts(),
    );
  }

  async headBucket(name: string): Promise<BucketInfo | null> {
    const bucketId = await this.api.query.S3Registry.BucketNameToId.getValue(
      utf8(name),
      READ_OPTS,
    );
    if (bucketId === undefined) return null;
    const bucket = await this.api.query.S3Registry.S3Buckets.getValue(bucketId, READ_OPTS);
    if (!bucket) return null;
    return {
      s3BucketId: bucketId,
      layer0BucketId: bucket.layer0_bucket_id,
      name: new TextDecoder().decode(bucket.name),
      owner: bucket.owner,
      createdAt: Number(bucket.created_at),
      objectCount: Number(bucket.object_count),
    };
  }

  async listBuckets(owner?: string): Promise<BucketInfo[]> {
    const who = owner ?? this.requireSigner().address;
    const ids = await this.api.query.S3Registry.UserBuckets.getValue(who, READ_OPTS);
    const buckets: BucketInfo[] = [];
    for (const id of ids ?? []) {
      const bucket = await this.api.query.S3Registry.S3Buckets.getValue(id, READ_OPTS);
      if (!bucket) continue;
      buckets.push({
        s3BucketId: id,
        layer0BucketId: bucket.layer0_bucket_id,
        name: new TextDecoder().decode(bucket.name),
        owner: bucket.owner,
        createdAt: Number(bucket.created_at),
        objectCount: Number(bucket.object_count),
      });
    }
    return buckets;
  }

  async deleteBucket(s3BucketId: bigint): Promise<void> {
    await deleteS3BucketTx(this.api, this.requireSigner(), s3BucketId, this.submitOpts());
  }

  // ── Object metadata chain ops ───────────────────────────────────────────

  async getObjectMetadata(s3BucketId: bigint, key: string): Promise<ObjectMetadata | null> {
    const stored = await this.api.query.S3Registry.Objects.getValue(
      s3BucketId,
      utf8(key),
      READ_OPTS,
    );
    if (!stored) return null;
    const userMetadata: Record<string, string> = {};
    const dec = new TextDecoder();
    for (const entry of stored.user_metadata ?? []) {
      userMetadata[dec.decode(entry.key)] = dec.decode(entry.value);
    }
    return {
      key,
      cid: asHex(stored.cid),
      size: stored.size,
      contentType: stored.content_type ? dec.decode(stored.content_type) : undefined,
      userMetadata,
    };
  }

  async putObjectMetadata(
    s3BucketId: bigint,
    key: string,
    obj: { cid: Uint8Array; size: bigint },
    contentType = "application/octet-stream",
    userMetadata: Array<[string, string]> = [],
  ): Promise<void> {
    this.validateObjectKey(key);
    await putObjectMetadataTx(
      this.api,
      this.requireSigner(),
      s3BucketId,
      key,
      obj,
      contentType,
      userMetadata,
      this.submitOpts(),
    );
  }

  async deleteObjectMetadata(s3BucketId: bigint, key: string): Promise<void> {
    await deleteObjectMetadataTx(
      this.api,
      this.requireSigner(),
      s3BucketId,
      key,
      this.submitOpts(),
    );
  }

  // ── Provider resolution ─────────────────────────────────────────────────

  getProviderUrl(layer0BucketId: bigint): Promise<string> {
    return this.providers.get(layer0BucketId);
  }

  invalidateProviderUrl(layer0BucketId?: bigint): void {
    this.providers.invalidate(layer0BucketId);
  }

  waitForProvider(layer0BucketId: bigint, opts?: WaitOpts): Promise<string> {
    return this.providers.waitForProvider(layer0BucketId, opts);
  }

  // ── Object HTTP ops ─────────────────────────────────────────────────────

  async putObject(
    bucket: BucketRef,
    key: string,
    data: Uint8Array,
    options: PutObjectOptions = {},
  ): Promise<PutObjectResult> {
    this.validateObjectKey(key);
    const providerUrl = await this.getProviderUrl(bucket.layer0BucketId);
    const headers: Record<string, string> = {
      "Content-Type": options.contentType || "application/octet-stream",
      ...this.authHeaders("PUT", bucket.layer0BucketId),
    };
    for (const [k, v] of Object.entries(options.metadata ?? {})) {
      headers[`x-amz-meta-${k}`] = v;
    }
    const response = await httpFetch(
      `${providerUrl}/s3/${Number(bucket.layer0BucketId)}/object?key=${encodeURIComponent(key)}`,
      { method: "PUT", headers, body: data as BodyInit, signal: options.signal },
      this.fetchOpts,
    );
    if (!response.ok) {
      throw new Error(`Upload failed: ${response.status} ${await response.text().catch(() => "")}`);
    }
    const body = await response.json().catch(() => ({}));
    return { cid: body.data_root ?? body.etag, size: data.length };
  }

  async getObject(
    bucket: BucketRef,
    key: string,
    opts: { signal?: AbortSignal; verify?: boolean } = {},
  ): Promise<GetObjectResponse> {
    const providerUrl = await this.getProviderUrl(bucket.layer0BucketId);
    const response = await httpFetch(
      `${providerUrl}/s3/${Number(bucket.layer0BucketId)}/object?key=${encodeURIComponent(key)}`,
      { signal: opts.signal, headers: this.authHeaders("GET", bucket.layer0BucketId) },
      this.fetchOpts,
    );
    if (!response.ok) {
      throw new Error(`Download failed: ${response.status} ${await response.text().catch(() => "")}`);
    }
    const data = new Uint8Array(await response.arrayBuffer());

    if (opts.verify === false) return { data, verified: false };

    const meta = await this.getObjectMetadata(bucket.s3BucketId, key).catch(() => null);
    if (!meta) return { data, verified: false };
    const actual = toHex(computeCid(data)).toLowerCase();
    if (actual === meta.cid.toLowerCase()) return { data, verified: true };
    if (data.length <= DEFAULT_CHUNK_SIZE) {
      // Single chunk: data_root == chunk hash, so a mismatch is proof of
      // corrupted/substituted bytes — hard fail.
      throw new CidMismatchError(meta.cid, actual);
    }
    // Multi-chunk: the on-chain cid is a Merkle root a flat hash can't
    // reproduce; DAG-walk verification is tracked separately.
    return { data, verified: false };
  }

  async deleteObject(bucket: BucketRef, key: string): Promise<void> {
    const providerUrl = await this.getProviderUrl(bucket.layer0BucketId);
    const response = await httpFetch(
      `${providerUrl}/s3/${Number(bucket.layer0BucketId)}/object?key=${encodeURIComponent(key)}`,
      { method: "DELETE", headers: this.authHeaders("DELETE", bucket.layer0BucketId) },
      this.fetchOpts,
    );
    if (!response.ok) {
      throw new Error(`Delete object failed: ${response.status} ${await response.text().catch(() => "")}`);
    }
  }

  async listObjects(bucket: BucketRef, prefix?: string): Promise<ObjectSummary[]> {
    const providerUrl = await this.getProviderUrl(bucket.layer0BucketId);
    const params = new URLSearchParams();
    if (prefix) params.set("prefix", prefix);
    const response = await httpFetch(
      `${providerUrl}/s3/${Number(bucket.layer0BucketId)}/objects?${params.toString()}`,
      { headers: this.authHeaders("GET", bucket.layer0BucketId) },
      this.fetchOpts,
    );
    if (!response.ok) throw new Error(`List objects failed: ${response.status}`);
    const result = await response.json();
    type WireObject = { key: string; size: number; last_modified?: number; etag?: string };
    return ((result.contents ?? []) as WireObject[]).map((o) => ({
      key: o.key,
      size: o.size,
      etag: o.etag,
      lastModified: o.last_modified !== undefined ? o.last_modified * 1000 : undefined,
    }));
  }
}
