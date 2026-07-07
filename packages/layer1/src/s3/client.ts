// SPDX-License-Identifier: Apache-2.0

/**
 * S3Client — S3Registry buckets/object-metadata (chain) + the provider
 * node's /s3 object HTTP surface. Chain ops delegate to the layer-0 pallet
 * wrappers (silent, no auto-retry, finalized submission + finalized reads by
 * default — UI-grade; tests/examples opt into in-block/best via
 * readOpts/submitMode); HTTP ops go through core's retrying fetch and are
 * signed with the signer's raw keypair, which the provider always requires.
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
  createS3Bucket as createS3BucketTx,
  deleteObjectMetadata as deleteObjectMetadataTx,
  deleteS3Bucket as deleteS3BucketTx,
  putObjectMetadata as putObjectMetadataTx,
  type ChainSigner,
  type ParachainApi,
  type SubmitOpts,
  type TxStatusListener,
  type WaitOpts,
} from "@web3-storage/layer0";

import {
  ProviderUrlResolver,
  resolveBucketProviders,
  resolveCreationTerms,
} from "../provider-url.js";
import type {
  BucketInfo,
  BucketRef,
  CreateBucketOptions,
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
  /**
   * Read view for chain lookups. Defaults to "finalized" (UI-grade,
   * reorg-safe). Tests/examples pass READ_OPTS ({at: "best"}) to match their
   * in-block submission semantics.
   */
  readOpts?: { at: "best" | "finalized" };
  /**
   * Submission doneness. Defaults to "finalized" (UI-grade). Tests/examples
   * pass "best" for speed.
   */
  submitMode?: "best" | "finalized";
}

const utf8 = (s: string) => new TextEncoder().encode(s);

export class S3Client {
  private readonly api: ParachainApi;
  private signer: ChainSigner | null;
  private readonly providers: ProviderUrlResolver;
  private readonly fetchOpts: HttpFetchOpts;
  private readonly onStatus: TxStatusListener | null;
  private readonly readOpts: { at: "best" | "finalized" };
  private readonly submitMode: "best" | "finalized";
  private readonly creationUrlOverride?: string;

  constructor(opts: S3ClientOptions) {
    this.api = opts.api;
    this.signer = opts.signer ?? null;
    this.readOpts = opts.readOpts ?? { at: "finalized" };
    this.submitMode = opts.submitMode ?? "finalized";
    this.providers = new ProviderUrlResolver(opts.api, opts.providerUrl, this.readOpts);
    this.creationUrlOverride = opts.providerUrl;
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
    return { mode: this.submitMode, retryStale: 0, onStatus: this.onStatus };
  }

  private authHeaders(method: string, layer0BucketId: bigint): Record<string, string> {
    return signProviderRequest(this.requireSigner().keypair, method, layer0BucketId);
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

  /**
   * Create an S3 bucket via the negotiate -> establish flow: pick a provider
   * (explicit `opts.provider` or auto-discovered), POST /negotiate for signed
   * terms (unless `opts.signedTerms` is supplied), then redeem them in
   * create_s3_bucket — which opens the underlying Layer 0 bucket + agreement
   * atomically.
   */
  async createBucket(name: string, opts: CreateBucketOptions): Promise<BucketInfo> {
    this.validateBucketName(name);
    const signer = this.requireSigner();
    const { provider, signedTerms } = await resolveCreationTerms(this.api, {
      owner: signer.address,
      maxBytes: opts.maxCapacity,
      duration: opts.duration,
      provider: opts.provider,
      signedTerms: opts.signedTerms,
      urlOverride: this.creationUrlOverride,
      readOpts: this.readOpts,
      fetchOpts: this.fetchOpts,
    });
    const { s3BucketId, layer0BucketId } = await createS3BucketTx(
      this.api,
      signer,
      name,
      provider,
      signedTerms,
      this.submitOpts(),
    );
    return {
      s3BucketId,
      layer0BucketId,
      name,
      owner: signer.address,
      createdAt: 0,
      objectCount: 0n,
      providerInfo: [{ account: provider.address, multiaddr: "", url: null }],
    };
  }

  async headBucket(name: string): Promise<BucketInfo | null> {
    const bucketId = await this.api.query.S3Registry.BucketNameToId.getValue(
      utf8(name),
      this.readOpts,
    );
    if (bucketId === undefined) return null;
    const bucket = await this.api.query.S3Registry.S3Buckets.getValue(bucketId, this.readOpts);
    if (!bucket) return null;
    const [providerInfo] = await resolveBucketProviders(
      this.api,
      [bucket.layer0_bucket_id],
      this.readOpts,
    );
    return {
      s3BucketId: bucketId,
      layer0BucketId: bucket.layer0_bucket_id,
      name: new TextDecoder().decode(bucket.name),
      owner: bucket.owner,
      createdAt: Number(bucket.created_at),
      objectCount: bucket.object_count,
      providerInfo: providerInfo ?? [],
    };
  }

  async listBuckets(owner?: string): Promise<BucketInfo[]> {
    const who = owner ?? this.requireSigner().address;
    const ids = await this.api.query.S3Registry.UserBuckets.getValue(who, this.readOpts);
    if (!ids || ids.length === 0) return [];

    // Batch the bucket lookups into one query instead of N round-trips.
    const bucketValues = await this.api.query.S3Registry.S3Buckets.getValues(
      ids.map((id) => [id] as const),
      this.readOpts,
    );

    const buckets: BucketInfo[] = [];
    const layer0BucketIds: bigint[] = [];
    bucketValues.forEach((bucket, i) => {
      if (!bucket) return;
      buckets.push({
        s3BucketId: ids[i]!,
        layer0BucketId: bucket.layer0_bucket_id,
        name: new TextDecoder().decode(bucket.name),
        owner: bucket.owner,
        createdAt: Number(bucket.created_at),
        objectCount: bucket.object_count,
        providerInfo: [],
      });
      layer0BucketIds.push(bucket.layer0_bucket_id);
    });

    // `providersByBucket[i]` aligns with `buckets[i]` (both built skipping nulls).
    const providersByBucket = await resolveBucketProviders(
      this.api,
      layer0BucketIds,
      this.readOpts,
    );
    buckets.forEach((bucket, i) => {
      bucket.providerInfo = providersByBucket[i] ?? [];
    });

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
      this.readOpts,
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
      `${providerUrl}/s3/${bucket.layer0BucketId}/object?key=${encodeURIComponent(key)}`,
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
      `${providerUrl}/s3/${bucket.layer0BucketId}/object?key=${encodeURIComponent(key)}`,
      { signal: opts.signal, headers: this.authHeaders("GET", bucket.layer0BucketId) },
      this.fetchOpts,
    );
    if (!response.ok) {
      throw new Error(`Download failed: ${response.status} ${await response.text().catch(() => "")}`);
    }
    const data = new Uint8Array(await response.arrayBuffer());

    if (opts.verify === false || bucket.s3BucketId === undefined) {
      return { data, verified: false };
    }

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
      `${providerUrl}/s3/${bucket.layer0BucketId}/object?key=${encodeURIComponent(key)}`,
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
      `${providerUrl}/s3/${bucket.layer0BucketId}/objects?${params.toString()}`,
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
