// SPDX-License-Identifier: Apache-2.0

/**
 * The file-system tree stored inside a bucket. The format, the tree and the
 * limits are the Rust crate `clients/bucket-tree` (see its `tree` module
 * docs), compiled to wasm; this class loads it and runs it over a
 * {@link BucketBlobStore}.
 */

import { loadWasm } from "#load-wasm";

import {
  WasmTree,
  type DeleteOptions,
  type FileRead,
  type ListOptions,
  type PutFileOptions,
  type PutFileResult,
  type TreeEntry,
  type TreeWriteResult,
} from "../pkg/bucket_tree.js";
import type { BucketBlobStore } from "./store.js";

/**
 * A file-system operation failed. `code` is the name of the Rust
 * `FsClientError` variant, e.g. `PathNotFound` or `EntryExists`.
 */
export class FileSystemError extends Error {
  readonly code: string;
  constructor(code: string, message: string, options?: ErrorOptions) {
    super(message, options);
    this.name = "FileSystemError";
    this.code = code;
  }
}

/**
 * Run `call` and turn a wasm error (an `Error` with `code`) into
 * `toError(code, message)`. Errors the blob store throws (for example a
 * provider 403) come through with their message.
 */
export async function withErrors<T>(
  call: () => Promise<T>,
  toError: (code: string, message: string, cause: unknown) => Error,
): Promise<T> {
  try {
    return await call();
  } catch (err) {
    const code = (err as { code?: unknown } | null)?.code;
    if (err instanceof Error && typeof code === "string") throw toError(code, err.message, err);
    throw err;
  }
}

const fsError = (code: string, message: string, cause: unknown) => new FileSystemError(code, message, { cause });

/** File-system operations on one bucket. */
export class FsTree {
  readonly bucketId: bigint;
  private tree?: WasmTree;

  constructor(readonly store: BucketBlobStore) {
    this.bucketId = store.bucketId;
  }

  private async wasm(): Promise<WasmTree> {
    await loadWasm();
    this.tree ??= new WasmTree(this.store, this.bucketId);
    return this.tree;
  }

  /** CID of the root directory, 0x-hex, or `null` for a bucket with no commits. */
  rootCid(): Promise<string | null> {
    return withErrors(async () => (await this.wasm()).rootCid(), fsError);
  }

  /** The entries of the directory at `path`, sorted by name bytes; with `recursive`, every entry below it. */
  list(path: string, options?: ListOptions): Promise<TreeEntry[]> {
    return withErrors(async () => (await this.wasm()).list(path, options), fsError);
  }

  /** The file at `path`: its metadata and content. */
  getFile(path: string): Promise<FileRead> {
    return withErrors(async () => (await this.wasm()).getFile(path), fsError);
  }

  /** Write the file at `path`, creating the directories above it. */
  putFile(path: string, bytes: Uint8Array, options?: PutFileOptions): Promise<PutFileResult> {
    return withErrors(async () => (await this.wasm()).putFile(path, bytes, options), fsError);
  }

  /** Create the directory at `path`, and the directories above it. */
  mkdir(path: string): Promise<TreeWriteResult> {
    return withErrors(async () => (await this.wasm()).mkdir(path), fsError);
  }

  /** Delete the file or empty directory at `path`. */
  delete(path: string, options?: DeleteOptions): Promise<TreeWriteResult> {
    return withErrors(async () => (await this.wasm()).delete(path, options), fsError);
  }
}
