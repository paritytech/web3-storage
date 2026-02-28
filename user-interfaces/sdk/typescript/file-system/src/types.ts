/**
 * File System SDK Types
 */

/** Drive information from the chain */
export interface DriveInfo {
  /** Unique drive identifier */
  driveId: bigint;
  /** Drive owner account */
  owner: string;
  /** Human-readable drive name */
  name: string | null;
  /** Associated Layer 0 bucket ID */
  bucketId: bigint;
  /** Root CID of the drive's content tree */
  rootCid: string | null;
  /** Block number when drive was created */
  createdAt: bigint;
  /** Block number of last update */
  updatedAt: bigint;
}

/** Commit strategy for checkpoints */
export enum CommitStrategy {
  /** Checkpoint after every write */
  Immediate = "Immediate",
  /** Batch writes and checkpoint periodically */
  Batched = "Batched",
  /** Manual checkpoint control */
  Manual = "Manual",
}

/** Directory entry (file or subdirectory) */
export interface DirectoryEntry {
  /** Entry name */
  name: string;
  /** True if this is a directory */
  isDirectory: boolean;
  /** Size in bytes (0 for directories) */
  size: number;
  /** Content hash (CID) */
  cid: string | null;
  /** Content type (MIME type for files) */
  contentType: string | null;
}

/** Options for creating a drive */
export interface CreateDriveOptions {
  /** Human-readable name for the drive */
  name?: string;
  /** Storage capacity in bytes */
  capacity: bigint;
  /** Duration in blocks */
  duration: number;
  /** Maximum payment (with 12 decimals) */
  maxPayment: bigint;
  /** Minimum number of providers */
  minProviders?: number;
  /** Commit strategy */
  commitStrategy?: CommitStrategy;
}

/** Options for uploading a file */
export interface UploadOptions {
  /** MIME content type */
  contentType?: string;
  /** Custom metadata */
  metadata?: Record<string, string>;
}

/** Result of a file upload */
export interface UploadResult {
  /** Content hash (CID) of the uploaded data */
  cid: string;
  /** Size in bytes */
  size: number;
}

/** Result of a file download */
export interface DownloadResult {
  /** File data */
  data: Uint8Array;
  /** Content type */
  contentType: string | null;
  /** Size in bytes */
  size: number;
}

/** SDK configuration */
export interface FileSystemConfig {
  /** Parachain WebSocket URL */
  chainWs: string;
  /** Provider HTTP URL */
  providerUrl: string;
}
