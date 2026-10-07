// SPDX-License-Identifier: Apache-2.0

/**
 * S3Client — S3 buckets (plain Layer 0 buckets) + the provider node's /s3
 * object HTTP surface. The chain stores no bucket name and no object
 * metadata: a bucket is identified by its id, and the provider's index
 * maps object keys to content. Chain ops delegate to the layer-0 pallet
 * wrappers (silent, no auto-retry, finalized submission + finalized reads by
 * default — UI-grade; tests/examples opt into in-block/best via
 * readOpts/submitMode); HTTP ops go through core's retrying fetch and are
 * signed with the signer's raw keypair, which the provider always requires.
 *
 * Bytes are opaque here: client-side encryption (when used) wraps/unwraps
 * app-side. Downloads by key are unverified until the key -> content
 * mapping is committed (#410).
 */

import { httpFetch } from "@web3-storage/core";
import {
  createBucketWithPrimary as createBucketWithPrimaryTx,
  type WaitOpts,
} from "@web3-storage/layer0";

import { Layer1Client, type Layer1ClientOptions } from "../base-client.js";
import { getBucketInfos, listMemberBuckets } from "../bucket-info.js";
import { resolveCreationTerms } from "../provider-url.js";
import type {
  BucketInfo,
  CreateBucketOptions,
  GetObjectResponse,
  ObjectSummary,
  PutObjectOptions,
  PutObjectResult,
} from "./types.js";

export type S3ClientOptions = Layer1ClientOptions;

export class S3Client extends Layer1Client {
  // ── Validation ──────────────────────────────────────────────────────────

  validateObjectKey(key: string): void {
    if (key.length < 1 || key.length > 1024) {
      throw new Error("Object key must be 1-1024 characters");
    }
  }

  // ── Bucket chain ops ────────────────────────────────────────────────────

  /**
   * Create an S3 bucket: a Layer 0 bucket with one primary agreement. Picks
   * a provider (explicit `opts.provider` or auto-discovered), POSTs
   * /negotiate for signed terms (unless `opts.signedTerms` is supplied),
   * then redeems them in `create_bucket_with_primary`.
   */
  async createBucket(opts: CreateBucketOptions): Promise<{ bucketId: bigint; provider: string }> {
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
    const { bucketId } = await createBucketWithPrimaryTx(this.api, signer, provider, signedTerms, {
      ...this.submitOpts(),
      visibility: opts.visibility,
    });
    return { bucketId, provider: provider.address };
  }

  async headBucket(bucketId: bigint): Promise<BucketInfo | null> {
    const [info] = await getBucketInfos(this.api, [bucketId], this.readOpts);
    return info ?? null;
  }

  /**
   * Every bucket `account` (default: the signer) is a member of, owned or
   * shared. The chain does not record which buckets hold S3 objects, so
   * this lists all of them.
   */
  async listBuckets(account?: string): Promise<BucketInfo[]> {
    return listMemberBuckets(this.api, account ?? this.requireSigner().address, this.readOpts);
  }

  // ── Provider resolution ─────────────────────────────────────────────────

  getProviderUrl(bucketId: bigint): Promise<string> {
    return this.providers.get(bucketId);
  }

  invalidateProviderUrl(bucketId?: bigint): void {
    this.providers.invalidate(bucketId);
  }

  waitForProvider(bucketId: bigint, opts?: WaitOpts): Promise<string> {
    return this.providers.waitForProvider(bucketId, opts);
  }

  // ── Object HTTP ops ─────────────────────────────────────────────────────

  async putObject(
    bucketId: bigint,
    key: string,
    data: Uint8Array,
    options: PutObjectOptions = {},
  ): Promise<PutObjectResult> {
    this.validateObjectKey(key);
    const providerUrl = await this.getProviderUrl(bucketId);
    const headers: Record<string, string> = {
      "Content-Type": options.contentType || "application/octet-stream",
      ...(await this.authHeaders("PUT", bucketId)),
    };
    for (const [k, v] of Object.entries(options.metadata ?? {})) {
      headers[`x-amz-meta-${k}`] = v;
    }
    const response = await httpFetch(
      `${providerUrl}/s3/${bucketId}/object?key=${encodeURIComponent(key)}`,
      { method: "PUT", headers, body: data as BodyInit, signal: options.signal },
      this.fetchOpts,
    );
    if (!response.ok) {
      throw new Error(`Upload failed: ${response.status} ${await response.text().catch(() => "")}`);
    }
    const body = await response.json().catch(() => ({}));
    return { cid: body.data_root ?? body.etag, size: data.length };
  }

  /**
   * Download an object by key. UNVERIFIED: the key -> content mapping comes
   * from the provider's index, which nothing on chain commits to (#410).
   */
  async getObject(
    bucketId: bigint,
    key: string,
    opts: { signal?: AbortSignal } = {},
  ): Promise<GetObjectResponse> {
    const providerUrl = await this.getProviderUrl(bucketId);
    const response = await httpFetch(
      `${providerUrl}/s3/${bucketId}/object?key=${encodeURIComponent(key)}`,
      { signal: opts.signal, headers: await this.authHeaders("GET", bucketId) },
      this.fetchOpts,
    );
    if (!response.ok) {
      throw new Error(`Download failed: ${response.status} ${await response.text().catch(() => "")}`);
    }
    return {
      data: new Uint8Array(await response.arrayBuffer()),
      contentType: response.headers.get("content-type") || "application/octet-stream",
    };
  }

  async deleteObject(bucketId: bigint, key: string): Promise<void> {
    const providerUrl = await this.getProviderUrl(bucketId);
    const response = await httpFetch(
      `${providerUrl}/s3/${bucketId}/object?key=${encodeURIComponent(key)}`,
      { method: "DELETE", headers: await this.authHeaders("DELETE", bucketId) },
      this.fetchOpts,
    );
    if (!response.ok) {
      throw new Error(`Delete object failed: ${response.status} ${await response.text().catch(() => "")}`);
    }
  }

  async listObjects(bucketId: bigint, prefix?: string): Promise<ObjectSummary[]> {
    const providerUrl = await this.getProviderUrl(bucketId);
    const params = new URLSearchParams();
    if (prefix) params.set("prefix", prefix);
    const response = await httpFetch(
      `${providerUrl}/s3/${bucketId}/objects?${params.toString()}`,
      { headers: await this.authHeaders("GET", bucketId) },
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
