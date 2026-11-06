// SPDX-License-Identifier: Apache-2.0

/** File-system interface types (drive-backed storage). */

import type { SignedTerms } from "@web3-storage/core";
import type { Visibility } from "@web3-storage/layer0";

import type { ProviderChoice } from "../provider-url.js";

export type { PrimaryProviderInfo } from "../provider-url.js";
export type { BucketInfo, BucketMember, MemberRole } from "../bucket-info.js";
import type { BucketInfo } from "../bucket-info.js";

/** A drive is a Layer 0 bucket; the chain stores nothing drive-specific. */
export type DriveInfo = BucketInfo;

export interface CreateDriveOptions {
  /** Bytes the agreement covers — the negotiated terms' max_bytes. */
  maxCapacity: bigint;
  /** Agreement duration in blocks — the negotiated terms' duration. */
  storagePeriod: number;
  /** Provider to negotiate with; auto-discovered (first accepting) when omitted. */
  provider?: ProviderChoice;
  /** Pre-negotiated terms (skips /negotiate). Requires provider.address. */
  signedTerms?: SignedTerms;
  /** Read visibility of the underlying Layer 0 bucket (default Private). */
  visibility?: Visibility;
}

export interface FsEntry {
  name: string;
  path: string;
  entryType: "file" | "directory";
  size: number;
  /** Milliseconds since epoch (stored with second precision). */
  mtime: number;
  /** File: CID of its manifest. Directory: CID of its directory node. 0x-hex. */
  cid: string;
}

export interface UploadOptions {
  contentType?: string;
  signal?: AbortSignal;
}

export interface UploadResult {
  /** CID of the file content (its `data_root`), 0x-hex. */
  dataRoot: string;
  /** CID of the file's manifest, 0x-hex. */
  manifestCid: string;
  /** CID of the drive's new root directory, 0x-hex. */
  rootCid: string;
  size: number;
}

export interface FileWithType {
  bytes: Uint8Array;
  /** MIME type stored in the file's manifest (`application/octet-stream` when none was given). */
  contentType: string;
}
