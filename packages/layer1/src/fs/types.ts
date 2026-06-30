// SPDX-License-Identifier: GPL-3.0-only

/** File-system interface types (drive-backed storage). */

import type { SignedTerms } from "@web3-storage/core";

import type { ProviderChoice } from "../provider-url.js";

export type { PrimaryProviderInfo } from "../provider-url.js";
import type { PrimaryProviderInfo } from "../provider-url.js";

export interface DriveInfo {
  driveId: bigint;
  bucketId: bigint;
  owner: string;
  name: string | null;
  maxCapacity: bigint;
  // block-number fields are u32 on chain → number in PAPI's typed API
  createdAt: number;
  storagePeriod: number;
  expiresAt: number;
  /** Primary providers of the underlying layer-0 bucket. */
  providerInfo: PrimaryProviderInfo[];
}

export interface CreateDriveOptions {
  name?: string;
  /** Bytes the agreement covers — the negotiated terms' max_bytes. */
  maxCapacity: bigint;
  /** Agreement duration in blocks — the negotiated terms' duration. */
  storagePeriod: number;
  /** Provider to negotiate with; auto-discovered (first accepting) when omitted. */
  provider?: ProviderChoice;
  /** Pre-negotiated terms (skips /negotiate). Requires provider.address. */
  signedTerms?: SignedTerms;
}

export interface FsEntry {
  name: string;
  path: string;
  entryType: "file" | "directory";
  size: number;
  /** Milliseconds since epoch. */
  mtime: number;
}

export type MemberRole = "Admin" | "Writer" | "Reader";

export interface BucketMember {
  account: string;
  role: MemberRole;
}

export interface UploadOptions {
  contentType?: string;
  signal?: AbortSignal;
}

export interface UploadResult {
  /** data_root CID echoed by the provider, when present in the response. */
  dataRoot?: string;
  size: number;
}

export interface IndexRoot {
  indexRoot: string;
  fileCount: number;
  dirCount: number;
  /** Total byte count (u64 on the wire) — kept as bigint to avoid 2^53 loss. */
  totalSize: bigint;
}

export interface CheckpointDuty {
  bucketId: number;
  mmrRoot: string;
  startSeq: number;
  leafCount: number;
  ready: boolean;
}
