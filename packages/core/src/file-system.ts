// SPDX-License-Identifier: Apache-2.0

/**
 * The file-system format of `crates/primitives/file-system`: `DirectoryNode`
 * and `FileManifest`, SCALE-encoded byte for byte like the Rust types. The
 * FS and S3 clients store these as blobs in a Layer 0 bucket. A blob's CID is
 * its `data_root` ({@link computeDataRoot}).
 */

import { computeDataRoot } from "./merkle.js";
import { ScaleError, ScaleReader, ScaleWriter } from "./scale.js";

/** Bounds of the Rust `BoundedVec`s (`Max*` types in the primitives crate). */
export const FS_LIMITS = {
  maxEntryNameLength: 256,
  maxDirectoryChildren: 1024,
  maxMetadataEntries: 64,
  maxMetadataKeyLength: 64,
  maxMetadataValueLength: 256,
  maxFileChunks: 65536,
  maxMimeTypeLength: 128,
  maxEncryptionParamsLength: 512,
} as const;

/** Length of the SCALE compact encoding of `n` (n < 2^30). */
function compactLength(n: number): number {
  return n < 1 << 6 ? 1 : n < 1 << 14 ? 2 : 4;
}

/** Largest SCALE encoding of a `BoundedVec` of `max` items of at most `itemSize` bytes each. */
function boundedVecMaxSize(max: number, itemSize: number): number {
  return compactLength(max) + max * itemSize;
}

const METADATA_ENTRY_MAX_SIZE =
  boundedVecMaxSize(FS_LIMITS.maxMetadataKeyLength, 1) + boundedVecMaxSize(FS_LIMITS.maxMetadataValueLength, 1);
const METADATA_MAX_SIZE = boundedVecMaxSize(FS_LIMITS.maxMetadataEntries, METADATA_ENTRY_MAX_SIZE);

/** Name + entry type + cid + size + mtime. */
const DIRECTORY_ENTRY_MAX_SIZE = boundedVecMaxSize(FS_LIMITS.maxEntryNameLength, 1) + 1 + 32 + 8 + 8;

/**
 * Largest SCALE encoding of a `DirectoryNode` (Rust `MaxEncodedLen`):
 * drive id + children + metadata. Reads of directory blobs stop at this size.
 */
export const DIRECTORY_NODE_MAX_SIZE =
  8 + boundedVecMaxSize(FS_LIMITS.maxDirectoryChildren, DIRECTORY_ENTRY_MAX_SIZE) + METADATA_MAX_SIZE;

/**
 * Largest SCALE encoding of a `FileManifest` (Rust `MaxEncodedLen`): drive
 * id + mime type + total size + chunks (cid + u32) + encryption params +
 * user metadata. Reads of manifest blobs stop at this size.
 */
export const FILE_MANIFEST_MAX_SIZE =
  8 +
  boundedVecMaxSize(FS_LIMITS.maxMimeTypeLength, 1) +
  8 +
  boundedVecMaxSize(FS_LIMITS.maxFileChunks, 32 + 4) +
  boundedVecMaxSize(FS_LIMITS.maxEncryptionParamsLength, 1) +
  METADATA_MAX_SIZE;

/**
 * True when `s` is well-formed UTF-16 (no lone surrogate), so it encodes
 * to UTF-8 without replacement characters. `String.prototype.isWellFormed`
 * is ES2024; this works on ES2022.
 */
export function isWellFormedString(s: string): boolean {
  return !/\p{Surrogate}/u.test(s);
}

const strictUtf8 = new TextDecoder("utf-8", { fatal: true });

/** True when `name` is a valid entry name: 1..=256 bytes of UTF-8, no `/`, not `.` or `..`. */
export function isValidEntryName(name: Uint8Array): boolean {
  if (name.length < 1 || name.length > FS_LIMITS.maxEntryNameLength) return false;
  if (name.includes(0x2f)) return false;
  if ((name.length === 1 && name[0] === 0x2e) || (name.length === 2 && name[0] === 0x2e && name[1] === 0x2e)) {
    return false;
  }
  try {
    strictUtf8.decode(name);
    return true;
  } catch {
    return false;
  }
}

function compareNames(a: Uint8Array, b: Uint8Array): number {
  const len = Math.min(a.length, b.length);
  for (let i = 0; i < len; i++) if (a[i] !== b[i]) return a[i] - b[i];
  return a.length - b.length;
}

/**
 * Check the rules a decoder does not, like Rust `DirectoryNode::validate`:
 * `metadata` is empty, every child name is valid ({@link isValidEntryName}),
 * directory entries have size 0, and children are sorted by name bytes with
 * no duplicates. Throws an `Error` that says which rule failed.
 */
export function validateDirectoryNode(node: DirectoryNode): void {
  if (node.metadata.length > 0) throw new Error("directory metadata must be empty");
  if (node.children.length > FS_LIMITS.maxDirectoryChildren) {
    throw new Error(`directory has more than ${FS_LIMITS.maxDirectoryChildren} children`);
  }
  node.children.forEach((e, i) => {
    if (!isValidEntryName(e.name)) throw new Error(`child ${i} has an invalid name`);
    if (e.entryType === "directory" && e.size !== 0n) throw new Error(`directory child ${i} has size ${e.size}`);
    if (i > 0 && compareNames(node.children[i - 1].name, e.name) >= 0) {
      throw new Error(`children are not sorted by name with unique names at ${i}`);
    }
  });
}

/** `EntryType`: SCALE index 0 = File, 1 = Directory. */
export type EntryType = "file" | "directory";

export interface MetadataEntry {
  key: Uint8Array;
  value: Uint8Array;
}

export interface DirectoryEntry {
  /** UTF-8 name bytes. */
  name: Uint8Array;
  entryType: EntryType;
  /** File: CID of its `FileManifest`. Directory: CID of its `DirectoryNode`. */
  cid: Uint8Array;
  /** File: content bytes. Directory: 0. */
  size: bigint;
  /** Unix seconds. */
  mtime: bigint;
}

export interface DirectoryNode {
  /** The Layer 0 bucket id. */
  driveId: bigint;
  /** Sorted by name bytes ascending; names are unique. */
  children: DirectoryEntry[];
  /** Reserved; always empty. */
  metadata: MetadataEntry[];
}

export interface FileChunk {
  cid: Uint8Array;
  /** Position of the chunk in the content (u32). */
  sequence: number;
}

export interface FileManifest {
  driveId: bigint;
  mimeType: Uint8Array;
  totalSize: bigint;
  /** Content chunk hashes in order. Empty content has one chunk: the hash of the empty chunk. */
  chunks: FileChunk[];
  /** Always empty. */
  encryptionParams: Uint8Array;
  userMetadata: MetadataEntry[];
}

function writeMetadata(w: ScaleWriter, entries: MetadataEntry[], field: string): void {
  w.vec(entries, FS_LIMITS.maxMetadataEntries, field, (w, e) => {
    w.bytes(e.key, FS_LIMITS.maxMetadataKeyLength, `${field} key`);
    w.bytes(e.value, FS_LIMITS.maxMetadataValueLength, `${field} value`);
  });
}

function readMetadata(r: ScaleReader, field: string): MetadataEntry[] {
  return r.vec(FS_LIMITS.maxMetadataEntries, field, (r) => ({
    key: r.bytes(FS_LIMITS.maxMetadataKeyLength, `${field} key`),
    value: r.bytes(FS_LIMITS.maxMetadataValueLength, `${field} value`),
  }));
}

export function encodeDirectoryNode(node: DirectoryNode): Uint8Array {
  const w = new ScaleWriter();
  w.u64(node.driveId);
  w.vec(node.children, FS_LIMITS.maxDirectoryChildren, "directory children", (w, e) => {
    w.bytes(e.name, FS_LIMITS.maxEntryNameLength, "entry name");
    w.u8(e.entryType === "file" ? 0 : 1);
    w.h256(e.cid);
    w.u64(e.size);
    w.u64(e.mtime);
  });
  writeMetadata(w, node.metadata, "directory metadata");
  return w.finish();
}

export function decodeDirectoryNode(bytes: Uint8Array): DirectoryNode {
  const r = new ScaleReader(bytes);
  const driveId = r.u64();
  const children = r.vec(FS_LIMITS.maxDirectoryChildren, "directory children", (r) => {
    const name = r.bytes(FS_LIMITS.maxEntryNameLength, "entry name");
    const tag = r.u8();
    if (tag > 1) throw new ScaleError(`invalid EntryType index ${tag}`);
    return {
      name,
      entryType: (tag === 0 ? "file" : "directory") as EntryType,
      cid: r.h256(),
      size: r.u64(),
      mtime: r.u64(),
    };
  });
  const metadata = readMetadata(r, "directory metadata");
  r.end();
  return { driveId, children, metadata };
}

export function encodeFileManifest(manifest: FileManifest): Uint8Array {
  const w = new ScaleWriter();
  w.u64(manifest.driveId);
  w.bytes(manifest.mimeType, FS_LIMITS.maxMimeTypeLength, "mime type");
  w.u64(manifest.totalSize);
  w.vec(manifest.chunks, FS_LIMITS.maxFileChunks, "file chunks", (w, c) => {
    w.h256(c.cid);
    w.u32(c.sequence);
  });
  w.bytes(manifest.encryptionParams, FS_LIMITS.maxEncryptionParamsLength, "encryption params");
  writeMetadata(w, manifest.userMetadata, "user metadata");
  return w.finish();
}

export function decodeFileManifest(bytes: Uint8Array): FileManifest {
  const r = new ScaleReader(bytes);
  const driveId = r.u64();
  const mimeType = r.bytes(FS_LIMITS.maxMimeTypeLength, "mime type");
  const totalSize = r.u64();
  const chunks = r.vec(FS_LIMITS.maxFileChunks, "file chunks", (r) => ({ cid: r.h256(), sequence: r.u32() }));
  const encryptionParams = r.bytes(FS_LIMITS.maxEncryptionParamsLength, "encryption params");
  const userMetadata = readMetadata(r, "user metadata");
  r.end();
  return { driveId, mimeType, totalSize, chunks, encryptionParams, userMetadata };
}

/** CID of an encoded `DirectoryNode`: the `data_root` of its encoding. */
export function directoryNodeCid(node: DirectoryNode): Uint8Array {
  return computeDataRoot(encodeDirectoryNode(node));
}

/** CID of an encoded `FileManifest`: the `data_root` of its encoding. */
export function fileManifestCid(manifest: FileManifest): Uint8Array {
  return computeDataRoot(encodeFileManifest(manifest));
}
