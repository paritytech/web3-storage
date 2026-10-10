// SPDX-License-Identifier: Apache-2.0

/** S3-style interface types (bucket/object storage). */

import type { SignedTerms } from "@web3-storage/core";
import type { Visibility } from "@web3-storage/layer0";

import type { ProviderChoice } from "../provider-url.js";

export type { PrimaryProviderInfo } from "../provider-url.js";
export type { BucketInfo, BucketMember, MemberRole } from "../bucket-info.js";

export interface CreateBucketOptions {
  /** Bytes the agreement covers — the negotiated terms' max_bytes. */
  maxCapacity: bigint;
  /** Agreement duration in blocks — the negotiated terms' duration. */
  duration: number;
  /** Provider to negotiate with; auto-discovered (first accepting) when omitted. */
  provider?: ProviderChoice;
  /** Pre-negotiated terms (skips /negotiate). Requires provider.address. */
  signedTerms?: SignedTerms;
  /** Read visibility of the underlying Layer 0 bucket (default Private). */
  visibility?: Visibility;
}

export interface PutObjectOptions {
  contentType?: string;
  /** User metadata, stored in the object's manifest. Keys are lowercased. */
  metadata?: Record<string, string>;
  signal?: AbortSignal;
}

export interface PutObjectResult {
  /** CID of the object content (its `data_root`), 0x-hex. */
  cid: string;
  /** Same as `cid`. */
  etag: string;
  /** CID of the bucket's new root directory, 0x-hex. */
  rootCid: string;
  size: number;
}

export interface HeadObjectResponse {
  key: string;
  /** Stored content type (`application/octet-stream` when none was given). */
  contentType: string;
  size: number;
  /** User metadata with lowercased keys. */
  metadata: Record<string, string>;
  /** CID of the object content, 0x-hex. */
  etag: string;
  /** Milliseconds since epoch (stored with second precision). */
  lastModified: number;
}

export interface GetObjectResponse extends HeadObjectResponse {
  data: Uint8Array;
}

export interface ObjectSummary {
  key: string;
  size: number;
  /** Not set by `listObjects`; use `headObject` for the content CID. */
  etag?: string;
  /** Milliseconds since epoch (stored with second precision). */
  lastModified?: number;
}

export interface ListObjectsOptions {
  prefix?: string;
  /** Keys containing this after the prefix are grouped into `commonPrefixes`. */
  delimiter?: string;
  /** Return only keys after this one (S3 `start-after`). */
  startAfter?: string;
  /** Maximum objects plus common prefixes to return (default 1000). */
  maxKeys?: number;
  signal?: AbortSignal;
}

export interface ListObjectsResult {
  objects: ObjectSummary[];
  commonPrefixes: string[];
  isTruncated: boolean;
  /** When truncated: pass as `startAfter` to get the next page. */
  nextStartAfter?: string;
}
