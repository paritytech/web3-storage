// SPDX-License-Identifier: Apache-2.0

/**
 * S3-style objects in a bucket's file tree (./tree.ts): key `k` is the file
 * at path `/k`. `putObject` creates the directories a key needs and
 * `deleteObject` removes the directories it leaves empty, so the bucket has
 * no directory objects. The content type and the user metadata (keys
 * lowercased) are stored in the file's manifest. The limits of the tree
 * apply.
 */

import { compareBytes } from "@web3-storage/core";

import { FS_LIMITS, isValidEntryName, isWellFormedString } from "./format.js";
import { FileSystemError, parsePath, type FileInfo, type FsTree } from "./tree.js";

/** Default and maximum `maxKeys` of `listObjects`, as in S3 and the Rust client. */
const MAX_KEYS = 1000;

const encoder = new TextEncoder();

export interface PutObjectOptions {
  contentType?: string;
  /** User metadata, stored in the object's manifest. Keys are lowercased. */
  metadata?: Record<string, string>;
}

export interface PutObjectResult {
  /** CID of the object content (its `data_root`), 0x-hex. */
  cid: string;
  /** Same as `cid`. */
  etag: string;
  /** CID of the bucket's new root directory, 0x-hex. */
  rootCid: string;
  size: number;
}

export interface HeadObjectResponse {
  key: string;
  /** Stored content type (`application/octet-stream` when none was given). */
  contentType: string;
  size: number;
  /** User metadata with lowercased keys. */
  metadata: Record<string, string>;
  /** CID of the object content, 0x-hex. */
  etag: string;
  /** Milliseconds since epoch (stored with second precision). */
  lastModified: number;
}

export interface GetObjectResponse extends HeadObjectResponse {
  data: Uint8Array;
}

export interface ObjectSummary {
  key: string;
  size: number;
  /** Not set by `listObjects`; use `headObject` for the content CID. */
  etag?: string;
  /** Milliseconds since epoch (stored with second precision). */
  lastModified?: number;
}

export interface ListObjectsOptions {
  prefix?: string;
  /** Keys containing this after the prefix are grouped into `commonPrefixes`. */
  delimiter?: string;
  /** Return only keys after this one (S3 `start-after`). */
  startAfter?: string;
  /** Maximum objects plus common prefixes to return (default 1000). */
  maxKeys?: number;
}

export interface ListObjectsResult {
  objects: ObjectSummary[];
  commonPrefixes: string[];
  isTruncated: boolean;
  /** When truncated: pass as `startAfter` to get the next page. */
  nextStartAfter?: string;
}

/** The objects of one bucket, stored in its file tree. */
export class Objects {
  constructor(private readonly tree: FsTree) {}

  /**
   * Store an object, replacing an existing one. Metadata keys are
   * lowercased. Fails when the key's path collides with a stored key, e.g.
   * `a/b` when `a` is an object. Requires a Writer or Admin role.
   */
  async putObject(key: string, data: Uint8Array, options: PutObjectOptions = {}): Promise<PutObjectResult> {
    validateObjectKey(key);
    const userMetadata = lowercaseMetadata(options.metadata ?? {});
    const r = await this.tree.putFile(keyPath(key), data, { contentType: options.contentType, userMetadata });
    return { cid: r.contentRoot, etag: r.contentRoot, rootCid: r.rootCid, size: r.size };
  }

  /** Download an object and its metadata. The content is checked against the bucket's tree. */
  async getObject(key: string): Promise<GetObjectResponse> {
    validateObjectKey(key);
    const file = await this.tree.getFile(keyPath(key)).catch(noSuchKey(key));
    return { ...objectHead(key, file), data: file.bytes };
  }

  /** An object's metadata without its content. */
  async headObject(key: string): Promise<HeadObjectResponse> {
    validateObjectKey(key);
    const file = await this.tree.statFile(keyPath(key)).catch(noSuchKey(key));
    return objectHead(key, file);
  }

  /**
   * Delete an object. Deleting a missing key succeeds (S3 semantics).
   * Directories left empty are removed. Requires a Writer or Admin role.
   */
  async deleteObject(key: string): Promise<void> {
    validateObjectKey(key);
    await this.tree.delete(keyPath(key), { missingOk: true, filesOnly: true, pruneEmptyParents: true });
  }

  /**
   * List objects in S3 key order (bytes ascending). `prefix` filters keys;
   * `startAfter` skips keys up to and including it, and a common prefix
   * equal to it; `delimiter` groups keys that contain it after the prefix
   * into `commonPrefixes`; `maxKeys` (default 1000, clamped to 1-1000)
   * limits objects plus common prefixes. On a truncated page, pass
   * `nextStartAfter` as `startAfter` to continue.
   *
   * Each call reads the tree again, so pages can come from different
   * bucket roots. Use {@link listAllObjects} to get every key from one read.
   */
  async listObjects(options: ListObjectsOptions = {}): Promise<ListObjectsResult> {
    const prefix = options.prefix ?? "";
    const requested = options.maxKeys ?? MAX_KEYS;
    if (Number.isNaN(requested)) throw new Error("maxKeys must be a number");
    const maxKeys = Math.min(Math.max(Math.floor(requested), 1), MAX_KEYS);
    const objects = await this.objectsUnder({ ...options, prefix });
    return paginateKeys(objects, { ...options, prefix, maxKeys });
  }

  /**
   * Every object whose key starts with `prefix`, in S3 key order, from one
   * read of the bucket's tree. With a `delimiter`, keys that contain it
   * after the prefix are grouped into `commonPrefixes`. No page limit.
   */
  async listAllObjects(
    options: { prefix?: string; delimiter?: string } = {},
  ): Promise<{ objects: ObjectSummary[]; commonPrefixes: string[] }> {
    const prefix = options.prefix ?? "";
    const objects = await this.objectsUnder({ ...options, prefix });
    const { objects: page, commonPrefixes } = paginateKeys(objects, {
      prefix,
      delimiter: options.delimiter,
      maxKeys: Number.POSITIVE_INFINITY,
    });
    return { objects: page, commonPrefixes };
  }

  /** Every object under the deepest directory that `prefix` names, unsorted. */
  private async objectsUnder(opts: { prefix: string; delimiter?: string; startAfter?: string }): Promise<ObjectSummary[]> {
    const { prefix, delimiter, startAfter } = opts;
    const baseDir = prefix.includes("/") ? prefix.slice(0, prefix.lastIndexOf("/")) : "";
    try {
      parsePath(keyPath(baseDir));
    } catch {
      // No valid key lies under an invalid directory path.
      return [];
    }
    const keyPrefix = baseDir === "" ? "" : `${baseDir}/`;
    const relStartAfter = startAfter?.startsWith(keyPrefix) ? startAfter.slice(keyPrefix.length) : "";
    const slash = relStartAfter.indexOf("/");
    const startAfterInSubdir = slash >= 0 && slash + 1 < relStartAfter.length;
    if (delimiter === "/" && !startAfterInSubdir) {
      const entries = await this.tree.listNonEmpty(keyPath(baseDir)).catch((err: unknown) => {
        if (err instanceof FileSystemError && (err.code === "NotFound" || err.code === "NotADirectory")) return [];
        throw err;
      });
      return entries.map((entry) => ({
        key: keyPrefix + entry.name + (entry.entryType === "directory" ? "/" : ""),
        size: Number(entry.size),
        lastModified: Number(entry.mtime) * 1000,
      }));
    }
    const files = await this.tree.listFiles(keyPath(baseDir));
    return files.map(({ relativePath, entry }) => ({
      key: keyPrefix + relativePath,
      size: Number(entry.size),
      lastModified: Number(entry.mtime) * 1000,
    }));
  }
}

/**
 * Throw unless `key` is a valid object key: well-formed UTF-16 (no lone
 * surrogate), 1-1024 bytes of UTF-8, split on `/` into segments of 1-256
 * bytes, no empty segment (no leading, trailing or double `/`), and no `.`
 * or `..` segment.
 */
function validateObjectKey(key: string): void {
  if (!isWellFormedString(key)) throw new Error("Object key must not contain a lone UTF-16 surrogate");
  const len = encoder.encode(key).length;
  if (len < 1 || len > 1024) throw new Error(`Object key must be 1-1024 bytes: "${key}"`);
  for (const segment of key.split("/")) {
    if (!isValidEntryName(encoder.encode(segment))) {
      throw new Error(`Object key must have valid segments (not empty, ".", ".." or over ${FS_LIMITS.maxEntryNameLength} bytes): "${key}"`);
    }
  }
}

/** Object key to tree path. An empty key is the root directory. */
function keyPath(key: string): string {
  return "/" + key;
}

function noSuchKey(key: string) {
  return (err: unknown): never => {
    if (err instanceof FileSystemError && ["NotFound", "NotADirectory", "IsADirectory"].includes(err.code)) {
      throw new FileSystemError("NotFound", `NoSuchKey: ${key}`);
    }
    throw err;
  };
}

function objectHead(key: string, file: FileInfo): HeadObjectResponse {
  return {
    key,
    contentType: file.contentType,
    size: Number(file.size),
    metadata: file.userMetadata,
    etag: file.contentRoot,
    lastModified: Number(file.mtime) * 1000,
  };
}

/**
 * Lowercase the metadata keys, as the Rust S3 client does. Rejects two keys
 * that are equal after lowercasing. The tree stores the entries sorted by key.
 */
function lowercaseMetadata(metadata: Record<string, string>): Record<string, string> {
  const lowered: Record<string, string> = {};
  for (const [k, v] of Object.entries(metadata)) {
    const key = k.toLowerCase();
    if (Object.hasOwn(lowered, key)) {
      throw new Error(`Invalid metadata: key "${key}" appears twice after lowercasing`);
    }
    lowered[key] = v;
  }
  return lowered;
}

/**
 * Select one page of `objects` with S3 list semantics, like the Rust
 * client's `list_page`: sort by key bytes, keep keys that start with
 * `prefix` and sort after `startAfter`. With a `delimiter`, keys that
 * contain it after the prefix collapse into one common prefix (the key up
 * to and including the delimiter); a common prefix equal to `startAfter` is
 * skipped. `maxKeys` (at least 1) limits objects plus common prefixes. On a
 * truncated page, `nextStartAfter` is the last key or common prefix of the
 * page.
 */
export function paginateKeys(
  objects: ObjectSummary[],
  opts: { prefix: string; delimiter?: string; startAfter?: string; maxKeys: number },
): ListObjectsResult {
  const { prefix, startAfter, maxKeys } = opts;
  const delimiter = opts.delimiter || undefined;
  const after = startAfter === undefined ? undefined : encoder.encode(startAfter);
  const sorted = objects
    .map((object) => ({ object, bytes: encoder.encode(object.key) }))
    .sort((a, b) => compareBytes(a.bytes, b.bytes));
  const result: ListObjectsResult = { objects: [], commonPrefixes: [], isTruncated: false };
  let last: string | undefined;
  for (const { object, bytes } of sorted) {
    const key = object.key;
    if (!key.startsWith(prefix) || (after !== undefined && compareBytes(bytes, after) <= 0)) continue;
    const at = delimiter ? key.indexOf(delimiter, prefix.length) : -1;
    const group = at >= 0 ? key.slice(0, at + delimiter!.length) : undefined;
    if (group !== undefined && (group === startAfter || group === last)) continue;
    if (result.objects.length + result.commonPrefixes.length >= maxKeys) {
      result.isTruncated = true;
      break;
    }
    if (group !== undefined) result.commonPrefixes.push(group);
    else result.objects.push(object);
    last = group ?? key;
  }
  if (result.isTruncated) result.nextStartAfter = last;
  return result;
}
