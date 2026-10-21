// SPDX-License-Identifier: Apache-2.0

/**
 * S3Client: S3 buckets on plain Layer 0 buckets. The chain stores no bucket
 * name and no object metadata; a bucket is identified by its id. Objects
 * are files in the bucket's file-system tree (../tree.ts): key `k` is path
 * `/k`, and the directories between are created and removed implicitly.
 * Only the provider's Layer 0 routes are used.
 *
 * Chain ops delegate to the layer-0 pallet wrappers (silent, no auto-retry,
 * finalized submission and reads by default; tests and examples opt into
 * best-block via readOpts/submitMode). Writes are signed with the signer,
 * which needs a Writer or Admin role. Reads are unauthenticated: anyone who
 * knows a CID can read the blob, so confidential objects need client-side
 * encryption (bytes are opaque here). Every read is checked against the
 * CIDs in the tree.
 *
 * One writer at a time: two clients that write the same bucket
 * concurrently can lose one of the changes (the last commit wins).
 */

import { isWellFormedString } from "@web3-storage/core";
import {
  createBucketWithPrimary as createBucketWithPrimaryTx,
  type WaitOpts,
} from "@web3-storage/layer0";

import { Layer1Client, type Layer1ClientOptions } from "../base-client.js";
import { getBucketInfos, listMemberBuckets } from "../bucket-info.js";
import { withHttpContext } from "../http-context.js";
import { resolveCreationTerms } from "../provider-url.js";
import { FileSystemError, parsePath, type FileInfo } from "../tree.js";
import type {
  BucketInfo,
  CreateBucketOptions,
  GetObjectResponse,
  HeadObjectResponse,
  ListObjectsOptions,
  ListObjectsResult,
  ObjectSummary,
  PutObjectOptions,
  PutObjectResult,
} from "./types.js";

export type S3ClientOptions = Layer1ClientOptions;

/** Default and maximum `maxKeys` of `listObjects`, as in S3 and the Rust client. */
const MAX_KEYS = 1000;

const encoder = new TextEncoder();

export class S3Client extends Layer1Client {
  // ── Validation ──────────────────────────────────────────────────────────

  /**
   * Throw unless `key` is a valid object key: well-formed UTF-16 (no lone
   * surrogate), 1-1024 bytes of UTF-8, split
   * on `/` into segments of 1-256 bytes, no empty segment (no leading,
   * trailing or double `/`), and no `.` or `..` segment.
   */
  validateObjectKey(key: string): void {
    if (!isWellFormedString(key)) throw new Error("Object key must not contain a lone UTF-16 surrogate");
    const len = encoder.encode(key).length;
    if (len < 1 || len > 1024) throw new Error(`Object key must be 1-1024 bytes: "${key}"`);
    for (const segment of key.split("/")) {
      if (segment === "") {
        throw new Error(`Object key must not have empty segments (leading, trailing or double "/"): "${key}"`);
      }
      if (segment === "." || segment === "..") {
        throw new Error(`Object key must not have "." or ".." segments: "${key}"`);
      }
      if (encoder.encode(segment).length > 256) {
        throw new Error(`Object key must have segments of at most 256 bytes: "${key}"`);
      }
    }
  }

  // ── Bucket chain ops ────────────────────────────────────────────────────

  /**
   * Create an S3 bucket: a Layer 0 bucket with one primary agreement. Picks
   * a provider (explicit `opts.provider` or auto-discovered), POSTs
   * /negotiate for signed terms (unless `opts.signedTerms` is supplied),
   * then redeems them in `create_bucket_with_primary`.
   */
  async createBucket(opts: CreateBucketOptions): Promise<{ bucketId: bigint; provider: string }> {
    const signer = this.requireSigner();
    const { provider, signedTerms } = await resolveCreationTerms(this.api, {
      owner: signer.address,
      maxBytes: opts.maxCapacity,
      duration: opts.duration,
      provider: opts.provider,
      signedTerms: opts.signedTerms,
      urlOverride: this.creationUrlOverride,
      readOpts: this.readOpts,
      fetchOpts: this.fetchOpts,
    });
    const { bucketId } = await createBucketWithPrimaryTx(this.api, signer, provider, signedTerms, {
      ...this.submitOpts(),
      visibility: opts.visibility,
    });
    return { bucketId, provider: provider.address };
  }

  async headBucket(bucketId: bigint): Promise<BucketInfo | null> {
    const [info] = await getBucketInfos(this.api, [bucketId], this.readOpts);
    return info ?? null;
  }

  /**
   * Every bucket `account` (default: the signer) is a member of, owned or
   * shared. The chain does not record which buckets hold S3 objects, so
   * this lists all of them.
   */
  async listBuckets(account?: string): Promise<BucketInfo[]> {
    return listMemberBuckets(this.api, account ?? this.requireSigner().address, this.readOpts);
  }

  // ── Provider resolution ─────────────────────────────────────────────────

  getProviderUrl(bucketId: bigint): Promise<string> {
    return this.providers.get(bucketId);
  }

  invalidateProviderUrl(bucketId?: bigint): void {
    this.providers.invalidate(bucketId);
  }

  waitForProvider(bucketId: bigint, opts?: WaitOpts): Promise<string> {
    return this.providers.waitForProvider(bucketId, opts);
  }

  // ── Object ops (the tree in the bucket; see ../tree.ts) ───────────────

  /**
   * Store an object, replacing an existing one. Metadata keys are
   * lowercased. Fails when the key's path collides with a stored key, e.g.
   * `a/b` when `a` is an object. Requires a Writer or Admin role.
   */
  async putObject(
    bucketId: bigint,
    key: string,
    data: Uint8Array,
    options: PutObjectOptions = {},
  ): Promise<PutObjectResult> {
    this.validateObjectKey(key);
    const userMetadata = lowercaseMetadata(options.metadata ?? {});
    const tree = await this.openTree(bucketId, { write: true, signal: options.signal });
    const r = await withHttpContext("Upload failed", () =>
      tree.putFile(keyPath(key), data, { contentType: options.contentType, userMetadata }),
    );
    return { cid: r.contentRoot, etag: r.contentRoot, rootCid: r.rootCid, size: r.size };
  }

  /** Download an object and its metadata. The content is checked against the bucket's tree. */
  async getObject(
    bucketId: bigint,
    key: string,
    opts: { signal?: AbortSignal } = {},
  ): Promise<GetObjectResponse> {
    this.validateObjectKey(key);
    const tree = await this.openTree(bucketId, { signal: opts.signal });
    const file = await withHttpContext("Download failed", () => tree.getFile(keyPath(key)).catch(noSuchKey(key)));
    return { ...objectHead(key, file), data: file.bytes };
  }

  /** An object's metadata without its content. */
  async headObject(
    bucketId: bigint,
    key: string,
    opts: { signal?: AbortSignal } = {},
  ): Promise<HeadObjectResponse> {
    this.validateObjectKey(key);
    const tree = await this.openTree(bucketId, { signal: opts.signal });
    const file = await withHttpContext("Head failed", () => tree.statFile(keyPath(key)).catch(noSuchKey(key)));
    return objectHead(key, file);
  }

  /**
   * Delete an object. Deleting a missing key succeeds (S3 semantics).
   * Directories left empty are removed. Requires a Writer or Admin role.
   */
  async deleteObject(bucketId: bigint, key: string, opts: { signal?: AbortSignal } = {}): Promise<void> {
    this.validateObjectKey(key);
    const tree = await this.openTree(bucketId, { write: true, signal: opts.signal });
    await withHttpContext("Delete object failed", () =>
      tree.delete(keyPath(key), { missingOk: true, filesOnly: true, pruneEmptyParents: true }),
    );
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
  async listObjects(bucketId: bigint, options: ListObjectsOptions = {}): Promise<ListObjectsResult> {
    const prefix = options.prefix ?? "";
    const requested = options.maxKeys ?? MAX_KEYS;
    if (Number.isNaN(requested)) throw new Error("maxKeys must be a number");
    const maxKeys = Math.min(Math.max(Math.floor(requested), 1), MAX_KEYS);
    const objects = await this.objectsUnder(bucketId, prefix, options.signal);
    return paginateKeys(objects, { ...options, prefix, maxKeys });
  }

  /**
   * Every object whose key starts with `prefix`, in S3 key order, from one
   * read of the bucket's tree. With a `delimiter`, keys that contain it
   * after the prefix are grouped into `commonPrefixes`. No page limit.
   */
  async listAllObjects(
    bucketId: bigint,
    options: { prefix?: string; delimiter?: string; signal?: AbortSignal } = {},
  ): Promise<{ objects: ObjectSummary[]; commonPrefixes: string[] }> {
    const prefix = options.prefix ?? "";
    const objects = await this.objectsUnder(bucketId, prefix, options.signal);
    const { objects: page, commonPrefixes } = paginateKeys(objects, {
      prefix,
      delimiter: options.delimiter,
      maxKeys: Number.POSITIVE_INFINITY,
    });
    return { objects: page, commonPrefixes };
  }

  /** Every object under the deepest directory that `prefix` names, unsorted. */
  private async objectsUnder(bucketId: bigint, prefix: string, signal?: AbortSignal): Promise<ObjectSummary[]> {
    const baseDir = prefix.includes("/") ? prefix.slice(0, prefix.lastIndexOf("/")) : "";
    try {
      parsePath(keyPath(baseDir));
    } catch {
      // No valid key lies under an invalid directory path.
      return [];
    }
    const tree = await this.openTree(bucketId, { signal });
    const files = await withHttpContext("List objects failed", () => tree.listFiles(keyPath(baseDir)));
    return files.map(({ relativePath, entry }) => ({
      key: baseDir === "" ? relativePath : `${baseDir}/${relativePath}`,
      size: Number(entry.size),
      lastModified: Number(entry.mtime) * 1000,
    }));
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

function compareBytes(a: Uint8Array, b: Uint8Array): number {
  const len = Math.min(a.length, b.length);
  for (let i = 0; i < len; i++) if (a[i] !== b[i]) return a[i] - b[i];
  return a.length - b.length;
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
