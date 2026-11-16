// SPDX-License-Identifier: Apache-2.0

/**
 * @web3-storage/storage-formats: a file system and S3-style objects stored in a
 * bucket. The format, the tree and the object mapping are the Rust crate
 * `clients/storage-formats`, compiled to wasm (`pkg/`, built by
 * `scripts/build-wasm.sh`). This package loads the wasm and supplies the blob
 * store over the provider's HTTP API. Needs a provider URL and a signer, no
 * chain connection.
 */

export { FileSystemError, FsTree, type WriteQueue } from "./tree.js";
export { ObjectError, S3Bucket } from "./objects.js";
export * from "./store.js";
export type {
  BlobStore,
  DeleteOptions,
  FileInfo,
  FileRead,
  GetObjectResponse,
  HeadObjectResponse,
  ListObjectsOptions,
  ListObjectsResult,
  ListOptions,
  ObjectSummary,
  PutFileOptions,
  PutFileResult,
  PutObjectInput,
  PutObjectResult,
  TreeEntry,
  TreeWriteResult,
} from "../pkg/storage_formats.js";
