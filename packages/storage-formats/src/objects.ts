// SPDX-License-Identifier: Apache-2.0

/**
 * S3-style objects stored in a bucket's file tree: key `k` is the file
 * `/k`. The mapping is the Rust `storage_formats::s3` module, compiled to wasm.
 */

import { loadWasm } from "#load-wasm";

import {
  WasmObjects,
  type GetObjectResponse,
  type HeadObjectResponse,
  type ListObjectsOptions,
  type ListObjectsResult,
  type ObjectSummary,
  type PutObjectInput,
  type PutObjectResult,
} from "../pkg/storage_formats.js";
import { FileSystemError, withErrors, type FsTree } from "./tree.js";

/** Rust `S3ClientError` variants; other failures are a {@link FileSystemError}. */
const OBJECT_ERROR_CODES = new Set(["ObjectNotFound", "InvalidObjectKey", "KeyConflict", "InvalidMetadata"]);

/** An object operation failed. `code` is the name of the Rust `S3ClientError` variant. */
export class ObjectError extends Error {
  readonly code: string;
  constructor(code: string, message: string, options?: ErrorOptions) {
    super(message, options);
    this.name = "ObjectError";
    this.code = code;
  }
}

const objectError = (code: string, message: string, cause: unknown) =>
  OBJECT_ERROR_CODES.has(code) ? new ObjectError(code, message, { cause }) : new FileSystemError(code, message, { cause });

/** The objects of one bucket, stored in its file tree. */
export class Objects {
  private objects?: WasmObjects;

  constructor(private readonly tree: FsTree) {}

  private async wasm(): Promise<WasmObjects> {
    await loadWasm();
    this.objects ??= new WasmObjects(this.tree.store, this.tree.bucketId);
    return this.objects;
  }

  /** Store `data` under `key`, replacing an object with the same key. */
  putObject(key: string, data: Uint8Array, options?: PutObjectInput): Promise<PutObjectResult> {
    return withErrors(async () => (await this.wasm()).putObject(key, data, options), objectError);
  }

  /** The object `key`: its metadata and content. */
  getObject(key: string): Promise<GetObjectResponse> {
    return withErrors(async () => (await this.wasm()).getObject(key), objectError);
  }

  /** The metadata of object `key`, without its content. */
  headObject(key: string): Promise<HeadObjectResponse> {
    return withErrors(async () => (await this.wasm()).headObject(key), objectError);
  }

  /** Delete the object `key`, and the directories it leaves empty. */
  deleteObject(key: string): Promise<void> {
    return withErrors(async () => (await this.wasm()).deleteObject(key), objectError);
  }

  /** One page of objects, sorted by key bytes. Each page reads the tree again. */
  listObjects(options?: ListObjectsOptions): Promise<ListObjectsResult> {
    return withErrors(async () => (await this.wasm()).listObjects(options), objectError);
  }

  /** Every object under `prefix`, page by page. */
  async listAllObjects(options: { prefix?: string } = {}): Promise<{ objects: ObjectSummary[] }> {
    const objects: ObjectSummary[] = [];
    let startAfter: string | undefined;
    for (;;) {
      const page = await this.listObjects({ prefix: options.prefix, startAfter });
      objects.push(...page.objects);
      if (!page.isTruncated || !page.nextStartAfter) return { objects };
      startAfter = page.nextStartAfter;
    }
  }
}
