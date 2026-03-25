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
  /** Full path */
  path: string;
  /** Entry type: "file" or "dir" */
  entry_type: "file" | "dir";
  /** Size in bytes (0 for directories) */
  size: number;
  /** Last modified timestamp (unix ms) */
  mtime: number;
}

/** Result of listing a directory */
export interface DirectoryListing {
  /** Directory path that was listed */
  path: string;
  /** Entries in the directory */
  entries: DirectoryEntry[];
  /** Number of files */
  fileCount: number;
  /** Number of subdirectories */
  dirCount: number;
  /** Total size of all entries in bytes */
  totalSize: number;
}

/** Drive index/integrity information */
export interface IndexRoot {
  /** Merkle root of the drive metadata */
  metadataMerkleRoot: string;
  /** Number of files in the drive */
  fileCount: number;
  /** Number of directories in the drive */
  dirCount: number;
  /** Total size of all files in bytes */
  totalSize: number;
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
  /** Content type (MIME) */
  contentType: string;
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
