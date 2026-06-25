// SPDX-License-Identifier: GPL-3.0-only

/** S3-style interface types (bucket/object storage). */

import type { SignedTerms } from "@web3-storage/core";

import type { ProviderChoice } from "../provider-url.js";

export type { PrimaryProviderInfo } from "../provider-url.js";
import type { PrimaryProviderInfo } from "../provider-url.js";

export interface BucketInfo {
  s3BucketId: bigint;
  layer0BucketId: bigint;
  name: string;
  owner: string;
  /** Creation block number (u32). */
  createdAt: number;
  /** On-chain object count (u64); bigint to avoid precision loss above 2^53. */
  objectCount: bigint;
  /** Primary providers of the underlying layer-0 bucket. */
  providerInfo: PrimaryProviderInfo[];
}

/**
 * Bucket handle for object ops. HTTP routes key off `layer0BucketId`;
 * `s3BucketId` is only needed to verify downloads against on-chain metadata
 * — without it, getObject returns `verified: false`.
 */
export interface BucketRef {
  layer0BucketId: bigint;
  s3BucketId?: bigint;
}

export interface CreateBucketOptions {
  /** Bytes the agreement covers — the negotiated terms' max_bytes. */
  maxCapacity: bigint;
  /** Agreement duration in blocks — the negotiated terms' duration. */
  duration: number;
  /** Provider to negotiate with; auto-discovered (first accepting) when omitted. */
  provider?: ProviderChoice;
  /** Pre-negotiated terms (skips /negotiate). Requires provider.address. */
  signedTerms?: SignedTerms;
}

export interface ObjectMetadata {
  key: string;
  /** 0x-hex blake2b-256 data_root. */
  cid: string;
  size: bigint;
  contentType?: string;
  userMetadata: Record<string, string>;
}

export interface PutObjectOptions {
  contentType?: string;
  /** Round-tripped as x-amz-meta-* headers. */
  metadata?: Record<string, string>;
  signal?: AbortSignal;
}

export interface PutObjectResult {
  /** data_root CID (falls back to the provider's etag field). */
  cid?: string;
  size: number;
}

export interface GetObjectResponse {
  data: Uint8Array;
  /**
   * True when the bytes hash to the on-chain CID. False when no on-chain
   * metadata exists, or the payload spans multiple chunks (data_root is then
   * a Merkle root a flat hash can't reproduce — DAG-walk verification is
   * tracked separately). Single-chunk mismatches THROW CidMismatchError
   * instead of returning false.
   */
  verified: boolean;
}

export interface ObjectSummary {
  key: string;
  size: number;
  etag?: string;
  lastModified?: number;
}
