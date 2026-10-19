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
  /** MIME type from the provider's `Content-Type` header, or a generic fallback. */
  contentType: string;
}

export interface ObjectSummary {
  key: string;
  size: number;
  etag?: string;
  lastModified?: number;
}
