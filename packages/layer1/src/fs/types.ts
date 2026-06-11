/** File-system interface types (drive-backed storage). */

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
  payment: bigint;
}

export interface CreateDriveOptions {
  name?: string;
  maxCapacity: bigint;
  storagePeriod: number;
  payment: bigint;
  minProviders?: number;
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
  totalSize: number;
}

export interface CheckpointDuty {
  bucketId: number;
  mmrRoot: string;
  startSeq: number;
  leafCount: number;
  ready: boolean;
}
