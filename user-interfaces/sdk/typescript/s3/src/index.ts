// SPDX-License-Identifier: Apache-2.0

/**
 * Web3 Storage S3 SDK
 *
 * TypeScript client for the S3-Compatible Interface (Layer 1)
 *
 * @packageDocumentation
 */

export { S3Client } from "./client.js";
export type {
  S3Config,
  BucketInfo,
  ObjectMetadata,
  ObjectSummary,
  PutObjectOptions,
  PutObjectResponse,
  GetObjectResponse,
  ListObjectsParams,
  ListObjectsResponse,
  CreateBucketOptions,
} from "./types.js";
