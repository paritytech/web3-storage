/**
 * S3 SDK Types
 */

/** S3 Bucket information */
export interface BucketInfo {
  /** S3 bucket ID */
  s3BucketId: bigint;
  /** Human-readable bucket name */
  name: string;
  /** Associated Layer 0 bucket ID */
  layer0BucketId: bigint;
  /** Bucket owner */
  owner: string;
  /** Block number when created */
  createdAt: bigint;
}

/** S3 Object metadata */
export interface ObjectMetadata {
  /** Object key (path) */
  key: string;
  /** Content hash (CID) */
  cid: string;
  /** Size in bytes */
  size: number;
  /** Last modified timestamp */
  lastModified: bigint;
  /** MIME content type */
  contentType: string | null;
  /** ETag (derived from CID) */
  etag: string;
  /** User-defined metadata */
  metadata: Record<string, string>;
}

/** Options for put_object */
export interface PutObjectOptions {
  /** MIME content type */
  contentType?: string;
  /** User-defined metadata */
  metadata?: Record<string, string>;
}

/** Response from put_object */
export interface PutObjectResponse {
  /** Content hash (CID) */
  cid: string;
  /** ETag */
  etag: string;
  /** Size in bytes */
  size: number;
}

/** Response from get_object */
export interface GetObjectResponse {
  /** Object data */
  data: Uint8Array;
  /** Object metadata */
  metadata: ObjectMetadata;
}

/** Parameters for list_objects_v2 */
export interface ListObjectsParams {
  /** Filter by key prefix */
  prefix?: string;
  /** Delimiter for grouping (usually "/") */
  delimiter?: string;
  /** Maximum number of keys to return */
  maxKeys?: number;
  /** Continuation token for pagination */
  continuationToken?: string;
}

/** Response from list_objects_v2 */
export interface ListObjectsResponse {
  /** Object summaries */
  contents: ObjectSummary[];
  /** Common prefixes (when delimiter is used) */
  commonPrefixes: string[];
  /** True if results are truncated */
  isTruncated: boolean;
  /** Token for next page */
  nextContinuationToken?: string;
  /** Number of keys returned */
  keyCount: number;
}

/** Summary of an object (for listing) */
export interface ObjectSummary {
  /** Object key */
  key: string;
  /** Size in bytes */
  size: number;
  /** Last modified timestamp */
  lastModified: bigint;
  /** ETag */
  etag: string;
}

/** Options for creating a bucket */
export interface CreateBucketOptions {
  /** Minimum number of storage providers */
  minProviders?: number;
  /** Storage capacity in bytes (for atomic bucket+agreement creation) */
  capacity?: bigint;
  /** Duration in blocks (for atomic bucket+agreement creation) */
  duration?: number;
  /** Maximum payment with 12 decimals (for atomic bucket+agreement creation) */
  maxPayment?: bigint;
}

/** SDK configuration */
export interface S3Config {
  /** Parachain WebSocket URL */
  chainWs: string;
  /** Provider HTTP URL */
  providerUrl: string;
}
