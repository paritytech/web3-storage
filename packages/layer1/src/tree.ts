// SPDX-License-Identifier: Apache-2.0

/**
 * The file-system tree that FileSystemClient and S3Client keep inside a
 * Layer 0 bucket. This module is the only code that knows the format, so S3
 * can move to another structure (#410) without touching the clients.
 *
 * Format: `DirectoryNode` and `FileManifest` from
 * `crates/primitives/file-system`, SCALE-encoded, each stored as a blob whose
 * CID is its `data_root`. A directory entry points at a child directory's
 * node or a file's manifest; a manifest lists the content chunk hashes.
 *
 * Root: the last MMR leaf of the bucket is the CID of the root directory. A
 * bucket with no leaves is an empty drive. Consequences:
 * - Only the FS/S3 clients may write to the bucket. A raw Layer 0 commit
 *   becomes the last leaf and the tree no longer loads.
 * - An Admin prefix delete (`/delete` with a new `start_seq`) can drop blobs
 *   that the current tree still references.
 *
 * Writes are read-modify-write with copy-on-write: load the root, upload
 * the new content, manifest and every rewritten directory up to the root,
 * then one `/commit` with the new root last. Untouched subtrees keep their
 * CIDs. There is one writer at a time: two writers that load the same root
 * and both commit lose one change (the last commit wins). Each operation
 * loads the root again right before it writes.
 *
 * Reads are unauthenticated and checked against the CIDs. A blob's hash is
 * enough to read it (#383/#396); confidentiality needs client-side
 * encryption.
 */

import {
  bytesEq,
  chunkCount,
  decodeDirectoryNode,
  decodeFileManifest,
  DIRECTORY_NODE_MAX_SIZE,
  encodeDirectoryNode,
  encodeFileManifest,
  FILE_MANIFEST_MAX_SIZE,
  FS_LIMITS,
  hexToBytes,
  isWellFormedString,
  paddedMerkleRoot,
  toHex,
  createLimiter,
  validateDirectoryNode,
  verifyLastMmrLeaf,
  type DirectoryEntry,
  type DirectoryNode,
  type FileManifest,
  type MetadataEntry,
} from "@web3-storage/core";
import {
  BlobTooLargeError,
  commitDataRoots,
  getCommitment,
  getMmrProof,
  readBlob,
  uploadBlob,
  type ChainSigner,
  type ProviderHttpOpts,
} from "@web3-storage/layer0";

/** Directory reads in flight at once while listing a tree. */
const DIRECTORY_READ_CONCURRENCY = 4;

/** Default content type of a file stored without one. */
export const DEFAULT_CONTENT_TYPE = "application/octet-stream";

export type FileSystemErrorCode =
  | "InvalidPath"
  | "NotFound"
  | "AlreadyExists"
  | "NotADirectory"
  | "IsADirectory"
  | "DirectoryNotEmpty"
  | "DirectoryFull"
  | "FileTooLarge"
  | "NotAFileSystem";

/** A file-system operation failed; `code` says why. */
export class FileSystemError extends Error {
  readonly code: FileSystemErrorCode;
  constructor(code: FileSystemErrorCode, message: string) {
    super(message);
    this.name = "FileSystemError";
    this.code = code;
  }
}

/** Blob storage and root discovery for one bucket. */
export interface BlobStore {
  readonly bucketId: bigint;
  /**
   * Read and check a blob. `size` selects the `/read` path; `maxSize`
   * rejects a larger blob (see `readBlob` in @web3-storage/layer0).
   */
  readBlob(cid: Uint8Array, opts?: { size?: bigint; maxSize?: number }): Promise<Uint8Array>;
  /** Store a blob; returns its CID and chunk hashes. */
  uploadBlob(bytes: Uint8Array): Promise<{ cid: Uint8Array; chunkHashes: Uint8Array[] }>;
  /** Append CIDs to the bucket's MMR, in order. */
  commit(cids: Uint8Array[]): Promise<void>;
  /** CID in the bucket's last MMR leaf, or `null` when the bucket has none. */
  lastCommittedRoot(): Promise<Uint8Array | null>;
}

/**
 * {@link BlobStore} over the provider's Layer 0 HTTP routes. `signer` is
 * needed only for writes.
 */
export function providerBlobStore(
  providerUrl: string,
  bucketId: bigint,
  signer: ChainSigner | null,
  http: ProviderHttpOpts = {},
): BlobStore {
  const requireSigner = () => {
    if (!signer) throw new Error("Signer not set");
    return signer;
  };
  return {
    bucketId,
    readBlob: (cid, opts = {}) => readBlob(providerUrl, cid, { ...http, ...opts }),
    async uploadBlob(bytes) {
      const r = await uploadBlob(providerUrl, bucketId, bytes, requireSigner(), http);
      return { cid: hexToBytes(r.dataRoot), chunkHashes: r.chunkHashes };
    },
    async commit(cids) {
      await commitDataRoots(providerUrl, bucketId, cids, requireSigner(), http);
    },
    async lastCommittedRoot() {
      const commitment = await getCommitment(providerUrl, bucketId, http);
      if (!commitment || commitment.leafCount === 0n) return null;
      const proof = await getMmrProof(providerUrl, bucketId, commitment.leafCount - 1n, http);
      // The proof ties the last leaf to the provider's MMR root. That root is
      // the provider's claim until compared with an on-chain checkpoint; the
      // commitment signature is not checked here.
      if (!verifyLastMmrLeaf(proof, commitment.mmrRoot, commitment.leafCount)) {
        throw new Error(`Bucket ${bucketId}: the provider's MMR proof for the last leaf is invalid`);
      }
      return proof.leaf.dataRoot;
    },
  };
}

/** An entry in a directory listing. */
export interface TreeEntry {
  name: string;
  /** Absolute path, e.g. `/docs/a.txt`. */
  path: string;
  entryType: "file" | "directory";
  /** File: manifest CID. Directory: directory node CID. 0x-hex. */
  cid: string;
  /** Content bytes (0 for a directory). */
  size: bigint;
  /** Unix seconds. */
  mtime: bigint;
}

/** A file's metadata, read from its manifest. */
export interface FileInfo {
  path: string;
  size: bigint;
  /** Unix seconds. */
  mtime: bigint;
  contentType: string;
  /** Root of the content's chunk tree (its CID), 0x-hex. */
  contentRoot: string;
  /** CID of the file's manifest, 0x-hex. */
  manifestCid: string;
  userMetadata: Record<string, string>;
}

export interface FileRead extends FileInfo {
  bytes: Uint8Array;
}

/** Result of a write. */
export interface TreeWriteResult {
  /** CID of the new root directory, 0x-hex. */
  rootCid: string;
}

export interface PutFileResult extends TreeWriteResult {
  contentRoot: string;
  manifestCid: string;
  size: number;
}

export interface PutFileOptions {
  contentType?: string;
  /** Stored in `FileManifest.user_metadata`, sorted by key bytes. */
  userMetadata?: Record<string, string>;
}

export interface DeleteOptions {
  /** Return `null` instead of throwing when the path does not exist. */
  missingOk?: boolean;
  /** Treat a directory at the path as missing (S3 keys name only files). */
  filesOnly?: boolean;
  /** Also remove parent directories that become empty (never the root). */
  pruneEmptyParents?: boolean;
}

const encoder = new TextEncoder();
const decoder = new TextDecoder();

function compareBytes(a: Uint8Array, b: Uint8Array): number {
  const len = Math.min(a.length, b.length);
  for (let i = 0; i < len; i++) if (a[i] !== b[i]) return a[i] - b[i];
  return a.length - b.length;
}

/**
 * Throw unless `name` is a valid entry name: well-formed UTF-16 (no lone
 * surrogate), 1..=256 UTF-8 bytes, no `/`, not `.` or `..`.
 */
export function validateEntryName(name: string): void {
  if (!isWellFormedString(name)) throw new FileSystemError("InvalidPath", "Name contains a lone UTF-16 surrogate");
  const len = encoder.encode(name).length;
  if (len < 1 || len > FS_LIMITS.maxEntryNameLength) {
    throw new FileSystemError("InvalidPath", `Name must be 1-${FS_LIMITS.maxEntryNameLength} bytes: "${name}"`);
  }
  if (name.includes("/")) throw new FileSystemError("InvalidPath", `Name must not contain "/": "${name}"`);
  if (name === "." || name === "..") throw new FileSystemError("InvalidPath", `Name must not be "${name}"`);
}

/**
 * Split an absolute path into its names. `/` is the root (no names). Throws
 * on a relative path, an empty segment (`//`, trailing `/`) or an invalid name.
 */
export function parsePath(path: string): string[] {
  if (!path.startsWith("/")) throw new FileSystemError("InvalidPath", `Path must start with "/": "${path}"`);
  if (path === "/") return [];
  const names = path.slice(1).split("/");
  for (const name of names) {
    if (name === "") throw new FileSystemError("InvalidPath", `Path has an empty segment: "${path}"`);
    validateEntryName(name);
  }
  return names;
}

function joinPath(names: string[]): string {
  return "/" + names.join("/");
}

function findEntry(node: DirectoryNode, name: string): DirectoryEntry | undefined {
  const bytes = encoder.encode(name);
  return node.children.find((e) => bytesEq(e.name, bytes));
}

/** Insert or replace `entry`, keeping children sorted by name bytes. */
function setEntry(node: DirectoryNode, entry: DirectoryEntry, dirPath: string): void {
  const i = node.children.findIndex((e) => compareBytes(e.name, entry.name) >= 0);
  if (i >= 0 && bytesEq(node.children[i].name, entry.name)) {
    node.children[i] = entry;
    return;
  }
  if (node.children.length >= FS_LIMITS.maxDirectoryChildren) {
    throw new FileSystemError(
      "DirectoryFull",
      `Directory ${dirPath} already has ${FS_LIMITS.maxDirectoryChildren} entries`,
    );
  }
  node.children.splice(i < 0 ? node.children.length : i, 0, entry);
}

function removeEntry(node: DirectoryNode, name: string): void {
  const bytes = encoder.encode(name);
  node.children = node.children.filter((e) => !bytesEq(e.name, bytes));
}

/** Entries sorted by key bytes, so equal metadata always gives the same manifest CID. */
function toMetadata(record: Record<string, string>): MetadataEntry[] {
  return Object.entries(record)
    .map(([k, v]) => ({ key: encoder.encode(k), value: encoder.encode(v) }))
    .sort((a, b) => compareBytes(a.key, b.key));
}

function fromMetadata(entries: MetadataEntry[]): Record<string, string> {
  return Object.fromEntries(entries.map((e) => [decoder.decode(e.key), decoder.decode(e.value)]));
}

/** The loaded root directory. `cid` is `null` for an empty drive. */
interface LoadedRoot {
  cid: Uint8Array | null;
  node: DirectoryNode;
}

/** File-system operations on one bucket. */
export class FsTree {
  constructor(
    private readonly store: BlobStore,
    /** Clock in unix seconds (tests inject a fixed one). */
    private readonly now: () => bigint = () => BigInt(Math.floor(Date.now() / 1000)),
  ) {}

  private emptyDirectory(): DirectoryNode {
    return { driveId: this.store.bucketId, children: [], metadata: [] };
  }

  /**
   * Read a directory node or manifest blob of at most `maxSize` bytes. A
   * larger blob throws `NotAFileSystem` with `context`.
   */
  private async readFormatBlob(cid: Uint8Array, maxSize: number, context: string): Promise<Uint8Array> {
    try {
      return await this.store.readBlob(cid, { maxSize });
    } catch (err) {
      if (err instanceof BlobTooLargeError) throw new FileSystemError("NotAFileSystem", `${context}: ${err.message}`);
      throw err;
    }
  }

  /** Read, decode and validate a directory node; `NotAFileSystem` if it is not a valid one. */
  private async readDirectory(cid: Uint8Array, path: string): Promise<DirectoryNode> {
    const bytes = await this.readFormatBlob(
      cid,
      DIRECTORY_NODE_MAX_SIZE,
      `${path} (${toHex(cid)}) is not a directory node`,
    );
    try {
      const node = decodeDirectoryNode(bytes);
      validateDirectoryNode(node);
      return node;
    } catch (err) {
      throw new FileSystemError(
        "NotAFileSystem",
        `${path} (${toHex(cid)}) is not a directory node: ${(err as Error).message}`,
      );
    }
  }

  /**
   * Load the current root directory: the last MMR leaf of the bucket (see
   * "Root" in the module doc). Throws `NotAFileSystem` when that blob is
   * not a valid directory node of this bucket.
   */
  async loadRoot(): Promise<LoadedRoot> {
    const cid = await this.store.lastCommittedRoot();
    if (!cid) return { cid: null, node: this.emptyDirectory() };
    let node: DirectoryNode;
    try {
      node = await this.readDirectory(cid, "/");
    } catch (err) {
      if (err instanceof FileSystemError) {
        throw this.notAFileSystem(cid, err.message);
      }
      throw err;
    }
    if (node.driveId !== this.store.bucketId) {
      throw this.notAFileSystem(cid, `the root directory belongs to drive ${node.driveId}`);
    }
    return { cid, node };
  }

  private notAFileSystem(rootCid: Uint8Array, reason: string): FileSystemError {
    return new FileSystemError(
      "NotAFileSystem",
      `Bucket ${this.store.bucketId} is not a file-system bucket: last committed root ${toHex(rootCid)}: ${reason}`,
    );
  }

  /**
   * CID of the current root directory, or `null` for an empty drive. Loads
   * and checks the root like {@link loadRoot}.
   */
  async rootCid(): Promise<string | null> {
    const { cid } = await this.loadRoot();
    return cid ? toHex(cid) : null;
  }

  /**
   * Load the directories along `names` from `root`: `[root, d1, d2, ...]`.
   * With `create`, missing directories become new empty ones; without, a
   * missing one throws `NotFound`.
   */
  private async descend(root: DirectoryNode, names: string[], create: boolean): Promise<DirectoryNode[]> {
    const nodes = [root];
    for (let i = 0; i < names.length; i++) {
      const path = joinPath(names.slice(0, i + 1));
      const entry = findEntry(nodes[i], names[i]);
      if (!entry) {
        if (!create) throw new FileSystemError("NotFound", `No such directory: ${path}`);
        nodes.push(this.emptyDirectory());
      } else if (entry.entryType !== "directory") {
        throw new FileSystemError("NotADirectory", `Not a directory: ${path}`);
      } else {
        nodes.push(await this.readDirectory(entry.cid, path));
      }
    }
    return nodes;
  }

  /**
   * Upload `nodes[0..=top]` bottom-up, linking each into its parent, and
   * commit `blobs` followed by the directory CIDs, root last.
   */
  private async writePath(nodes: DirectoryNode[], names: string[], top: number, blobs: Uint8Array[]): Promise<string> {
    const mtime = this.now();
    const cids = [...blobs];
    let rootCid: Uint8Array | null = null;
    for (let i = top; i >= 0; i--) {
      const { cid } = await this.store.uploadBlob(encodeDirectoryNode(nodes[i]));
      cids.push(cid);
      if (i === 0) rootCid = cid;
      else {
        setEntry(
          nodes[i - 1],
          { name: encoder.encode(names[i - 1]), entryType: "directory", cid, size: 0n, mtime },
          joinPath(names.slice(0, i - 1)),
        );
      }
    }
    await this.store.commit(commitOrder(cids));
    return toHex(rootCid!);
  }

  /** List a directory. `recursive` includes every descendant, depth-first. */
  async list(path: string, opts: { recursive?: boolean } = {}): Promise<TreeEntry[]> {
    const names = parsePath(path);
    const { node: root } = await this.loadRoot();
    const nodes = await this.descend(root, names, false);
    const out: TreeEntry[] = [];
    const visit = async (node: DirectoryNode, dirNames: string[]) => {
      for (const e of node.children) {
        const name = decoder.decode(e.name);
        const entryNames = [...dirNames, name];
        out.push({
          name,
          path: joinPath(entryNames),
          entryType: e.entryType,
          cid: toHex(e.cid),
          size: e.size,
          mtime: e.mtime,
        });
        if (opts.recursive && e.entryType === "directory") {
          await visit(await this.readDirectory(e.cid, joinPath(entryNames)), entryNames);
        }
      }
    };
    await visit(nodes[nodes.length - 1], names);
    return out;
  }

  /**
   * Every file under `path` (recursively), with its path relative to `path`
   * (e.g. `a/b.txt`). Returns an empty list when `path` does not exist or is
   * a file.
   */
  async listFiles(path: string): Promise<Array<{ relativePath: string; entry: TreeEntry }>> {
    const names = parsePath(path);
    const { node: root } = await this.loadRoot();
    let base: DirectoryNode;
    try {
      const nodes = await this.descend(root, names, false);
      base = nodes[nodes.length - 1];
    } catch (err) {
      if (err instanceof FileSystemError && (err.code === "NotFound" || err.code === "NotADirectory")) return [];
      throw err;
    }
    const out: Array<{ relativePath: string; entry: TreeEntry }> = [];
    const limit = createLimiter(DIRECTORY_READ_CONCURRENCY);
    const visit = async (node: DirectoryNode, rel: string[]): Promise<void> => {
      const subdirs: Promise<void>[] = [];
      for (const e of node.children) {
        const name = decoder.decode(e.name);
        const entryRel = [...rel, name];
        if (e.entryType === "directory") {
          const fullPath = joinPath([...names, ...entryRel]);
          subdirs.push(limit(() => this.readDirectory(e.cid, fullPath)).then((child) => visit(child, entryRel)));
        } else {
          out.push({
            relativePath: entryRel.join("/"),
            entry: {
              name,
              path: joinPath([...names, ...entryRel]),
              entryType: "file",
              cid: toHex(e.cid),
              size: e.size,
              mtime: e.mtime,
            },
          });
        }
      }
      await Promise.all(subdirs);
    };
    await visit(base, []);
    return out;
  }

  /** Find the file entry at `path` and read its manifest. */
  private async loadFile(path: string): Promise<{ entry: DirectoryEntry; manifest: FileManifest; info: FileInfo }> {
    const names = parsePath(path);
    if (names.length === 0) throw new FileSystemError("IsADirectory", "Is a directory: /");
    const { node: root } = await this.loadRoot();
    const nodes = await this.descend(root, names.slice(0, -1), false);
    const entry = findEntry(nodes[nodes.length - 1], names[names.length - 1]);
    if (!entry) throw new FileSystemError("NotFound", `No such file: ${path}`);
    if (entry.entryType !== "file") throw new FileSystemError("IsADirectory", `Is a directory: ${path}`);
    const bytes = await this.readFormatBlob(entry.cid, FILE_MANIFEST_MAX_SIZE, `${path}: invalid file manifest`);
    let manifest: FileManifest;
    try {
      manifest = decodeFileManifest(bytes);
    } catch (err) {
      throw new FileSystemError("NotAFileSystem", `${path}: invalid file manifest: ${(err as Error).message}`);
    }
    const valid =
      manifest.chunks.length === chunkCount(manifest.totalSize) &&
      manifest.chunks.every((c, i) => c.sequence === i);
    if (!valid) throw new FileSystemError("NotAFileSystem", `${path}: file manifest chunk list is inconsistent`);
    const contentRoot = paddedMerkleRoot(manifest.chunks.map((c) => c.cid));
    return {
      entry,
      manifest,
      info: {
        path,
        size: manifest.totalSize,
        mtime: entry.mtime,
        contentType: decoder.decode(manifest.mimeType) || DEFAULT_CONTENT_TYPE,
        contentRoot: toHex(contentRoot),
        manifestCid: toHex(entry.cid),
        userMetadata: fromMetadata(manifest.userMetadata),
      },
    };
  }

  /** A file's metadata, without its content. */
  async statFile(path: string): Promise<FileInfo> {
    return (await this.loadFile(path)).info;
  }

  /** A file's content and metadata. The content is checked against the manifest. */
  async getFile(path: string): Promise<FileRead> {
    const { manifest, info } = await this.loadFile(path);
    const bytes = await this.store.readBlob(hexToBytes(info.contentRoot), { size: manifest.totalSize });
    return { ...info, bytes };
  }

  /**
   * Store `bytes` at `path`. Creates missing parent directories and replaces
   * an existing file. Throws `IsADirectory` when `path` is a directory.
   */
  async putFile(path: string, bytes: Uint8Array, opts: PutFileOptions = {}): Promise<PutFileResult> {
    const names = parsePath(path);
    if (names.length === 0) throw new FileSystemError("IsADirectory", "Is a directory: /");
    const { node: root } = await this.loadRoot();
    const dirNames = names.slice(0, -1);
    const nodes = await this.descend(root, dirNames, true);
    const parent = nodes[nodes.length - 1];
    const existing = findEntry(parent, names[names.length - 1]);
    if (existing?.entryType === "directory") throw new FileSystemError("IsADirectory", `Is a directory: ${path}`);

    const manifestBase = {
      driveId: this.store.bucketId,
      mimeType: encoder.encode(opts.contentType || DEFAULT_CONTENT_TYPE),
      totalSize: BigInt(bytes.length),
      encryptionParams: new Uint8Array(0),
      userMetadata: toMetadata(opts.userMetadata ?? {}),
    };
    // Encode a manifest without chunks first, so a bound error (mime type,
    // metadata) occurs before the content upload.
    encodeFileManifest({ ...manifestBase, chunks: [] });
    if (chunkCount(bytes.length) > FS_LIMITS.maxFileChunks) {
      throw new FileSystemError("FileTooLarge", `${path}: file exceeds ${FS_LIMITS.maxFileChunks} chunks`);
    }

    const content = await this.store.uploadBlob(bytes);
    const manifest: FileManifest = {
      ...manifestBase,
      chunks: content.chunkHashes.map((cid, sequence) => ({ cid, sequence })),
    };
    const { cid: manifestCid } = await this.store.uploadBlob(encodeFileManifest(manifest));
    setEntry(
      parent,
      {
        name: encoder.encode(names[names.length - 1]),
        entryType: "file",
        cid: manifestCid,
        size: BigInt(bytes.length),
        mtime: this.now(),
      },
      joinPath(dirNames),
    );
    const rootCid = await this.writePath(nodes, names, nodes.length - 1, [content.cid, manifestCid]);
    return { rootCid, contentRoot: toHex(content.cid), manifestCid: toHex(manifestCid), size: bytes.length };
  }

  /** Create a directory and any missing parents. Throws `AlreadyExists` when `path` exists. */
  async mkdir(path: string): Promise<TreeWriteResult> {
    const names = parsePath(path);
    if (names.length === 0) throw new FileSystemError("AlreadyExists", "Already exists: /");
    const { node: root } = await this.loadRoot();
    const nodes = await this.descend(root, names.slice(0, -1), true);
    if (findEntry(nodes[nodes.length - 1], names[names.length - 1])) {
      throw new FileSystemError("AlreadyExists", `Already exists: ${path}`);
    }
    nodes.push(this.emptyDirectory());
    return { rootCid: await this.writePath(nodes, names, nodes.length - 1, []) };
  }

  /**
   * Remove a file or an empty directory. Throws `NotFound` when `path` does
   * not exist (unless `missingOk`) and `DirectoryNotEmpty` for a directory
   * with entries. The root cannot be deleted. Returns `null` when nothing
   * was removed.
   */
  async delete(path: string, opts: DeleteOptions = {}): Promise<TreeWriteResult | null> {
    const names = parsePath(path);
    if (names.length === 0) throw new FileSystemError("InvalidPath", "The root directory cannot be deleted");
    const { node: root } = await this.loadRoot();
    let nodes: DirectoryNode[];
    try {
      nodes = await this.descend(root, names.slice(0, -1), false);
    } catch (err) {
      const missing = err instanceof FileSystemError && (err.code === "NotFound" || err.code === "NotADirectory");
      if (missing && opts.missingOk) return null;
      throw err;
    }
    const name = names[names.length - 1];
    const entry = findEntry(nodes[nodes.length - 1], name);
    if (!entry || (opts.filesOnly && entry.entryType === "directory")) {
      if (opts.missingOk) return null;
      throw new FileSystemError("NotFound", `No such file or directory: ${path}`);
    }
    if (entry.entryType === "directory") {
      const dir = await this.readDirectory(entry.cid, path);
      if (dir.children.length > 0) throw new FileSystemError("DirectoryNotEmpty", `Directory not empty: ${path}`);
    }
    let top = nodes.length - 1;
    removeEntry(nodes[top], name);
    if (opts.pruneEmptyParents) {
      while (top > 0 && nodes[top].children.length === 0) {
        removeEntry(nodes[top - 1], names[top - 1]);
        top--;
      }
    }
    return { rootCid: await this.writePath(nodes, names, top, []) };
  }
}

/**
 * Order for one `/commit`: repeated CIDs dropped (first occurrence kept),
 * and the last element of `cids` (the new root) last.
 */
export function commitOrder(cids: Uint8Array[]): Uint8Array[] {
  const root = cids[cids.length - 1];
  const seen = new Set<string>([toHex(root)]);
  const out: Uint8Array[] = [];
  for (const cid of cids.slice(0, -1)) {
    const key = toHex(cid);
    if (seen.has(key)) continue;
    seen.add(key);
    out.push(cid);
  }
  out.push(root);
  return out;
}
