// SPDX-License-Identifier: Apache-2.0

import { describe, expect, it } from "vitest";
import { blake2b256 } from "@polkadot-labs/hdkd-helpers";
import {
  decodeDirectoryNode,
  DIRECTORY_NODE_MAX_SIZE,
  FILE_MANIFEST_MAX_SIZE,
  isWellFormedString,
  validateDirectoryNode,
  decodeFileManifest,
  directoryNodeCid,
  encodeDirectoryNode,
  encodeFileManifest,
  fileManifestCid,
  FS_LIMITS,
  type DirectoryNode,
  type FileManifest,
} from "./file-system.js";
import { computeDataRoot } from "./merkle.js";
import { ScaleError } from "./scale.js";
import { hexToBytes, toHex } from "./bytes.js";

const te = new TextEncoder();

// Shared with the Rust implementation (spec section 6).
const VECTOR_DIRECTORY: DirectoryNode = {
  driveId: 7n,
  children: [
    { name: te.encode("a.txt"), entryType: "file", cid: blake2b256(te.encode("a")), size: 1n, mtime: 1700000000n },
    { name: te.encode("docs"), entryType: "directory", cid: blake2b256(te.encode("d")), size: 0n, mtime: 1700000001n },
  ],
  metadata: [],
};
const VECTOR_DIRECTORY_HEX =
  "0x07000000000000000814612e747874008928aae63c84d87ea098564d1e03ad813f107add474e56aedd286349c0c03ea4" +
  "010000000000000000f153650000000010646f63730100d116515f37a4c0ac872096c8b7412c80693cc5cee2e99e83a7e7" +
  "60dc1ece91000000000000000001f153650000000000";
const VECTOR_DIRECTORY_CID = "0x2a05cb11a13c3962071e9843a1840b8f2a7948c2b404481f01e252d4f3ce6af9";

const VECTOR_MANIFEST: FileManifest = {
  driveId: 7n,
  mimeType: te.encode("text/plain"),
  totalSize: 1n,
  chunks: [{ cid: blake2b256(te.encode("a")), sequence: 0 }],
  encryptionParams: new Uint8Array(0),
  userMetadata: [{ key: te.encode("origin"), value: te.encode("test") }],
};
const VECTOR_MANIFEST_HEX =
  "0x070000000000000028746578742f706c61696e0100000000000000048928aae63c84d87ea098564d1e03ad813f107add" +
  "474e56aedd286349c0c03ea4000000000004186f726967696e1074657374";
const VECTOR_MANIFEST_CID = "0x4060dec5812cb094e14f3566f175faa0aeda15346f731a971a5c75ad6a345a02";

describe("shared test vectors", () => {
  it("DirectoryNode encoding and CID", () => {
    expect(toHex(encodeDirectoryNode(VECTOR_DIRECTORY))).toBe(VECTOR_DIRECTORY_HEX);
    expect(toHex(directoryNodeCid(VECTOR_DIRECTORY))).toBe(VECTOR_DIRECTORY_CID);
    expect(decodeDirectoryNode(hexToBytes(VECTOR_DIRECTORY_HEX))).toEqual(VECTOR_DIRECTORY);
  });

  it("FileManifest encoding and CID", () => {
    expect(toHex(encodeFileManifest(VECTOR_MANIFEST))).toBe(VECTOR_MANIFEST_HEX);
    expect(toHex(fileManifestCid(VECTOR_MANIFEST))).toBe(VECTOR_MANIFEST_CID);
    expect(decodeFileManifest(hexToBytes(VECTOR_MANIFEST_HEX))).toEqual(VECTOR_MANIFEST);
  });
});

describe("file-system format", () => {
  it("CID is the data_root of the encoding, also above one chunk", () => {
    const big: FileManifest = {
      ...VECTOR_MANIFEST,
      chunks: Array.from({ length: 10_000 }, (_, i) => ({ cid: new Uint8Array(32).fill(i % 256), sequence: i })),
    };
    const encoded = encodeFileManifest(big);
    expect(encoded.length).toBeGreaterThan(256 * 1024);
    expect(fileManifestCid(big)).toEqual(computeDataRoot(encoded));
    expect(decodeFileManifest(encoded)).toEqual(big);
  });

  it("enforces bounds on encode and decode", () => {
    const longName = { ...VECTOR_DIRECTORY.children[0], name: new Uint8Array(FS_LIMITS.maxEntryNameLength + 1) };
    expect(() => encodeDirectoryNode({ ...VECTOR_DIRECTORY, children: [longName] })).toThrow(ScaleError);
    const longMime = { ...VECTOR_MANIFEST, mimeType: new Uint8Array(FS_LIMITS.maxMimeTypeLength + 1) };
    expect(() => encodeFileManifest(longMime)).toThrow(/mime type/);
    // Patch the name length prefix of the vector to 257 (compact 0x0504).
    const bytes = hexToBytes(VECTOR_DIRECTORY_HEX);
    const patched = new Uint8Array([...bytes.subarray(0, 9), 0x05, 0x04, ...bytes.subarray(10)]);
    expect(() => decodeDirectoryNode(patched)).toThrow(/entry name is 257 bytes/);
  });

  it("rejects an invalid entry type and trailing bytes", () => {
    const bytes = hexToBytes(VECTOR_DIRECTORY_HEX);
    const badType = bytes.slice();
    badType[15] = 2;
    expect(() => decodeDirectoryNode(badType)).toThrow(/EntryType/);
    expect(() => decodeDirectoryNode(new Uint8Array([...bytes, 0]))).toThrow(/trailing/);
  });

  it("does not decode a manifest as a directory node", () => {
    expect(() => decodeDirectoryNode(hexToBytes(VECTOR_MANIFEST_HEX))).toThrow(ScaleError);
  });
});

describe("validateDirectoryNode", () => {
  const entry = (name: string | Uint8Array, entryType: "file" | "directory", size: bigint) => ({
    name: typeof name === "string" ? te.encode(name) : name,
    entryType,
    cid: new Uint8Array(32),
    size,
    mtime: 0n,
  });
  const node = (children: ReturnType<typeof entry>[], metadata: DirectoryNode["metadata"] = []): DirectoryNode => ({
    driveId: 1n,
    children,
    metadata,
  });

  // The cases of Rust `validate_rejects_bad_directories`.
  it("rejects bad directories", () => {
    expect(() => validateDirectoryNode(node([entry("b", "file", 0n), entry("a", "file", 0n)]))).toThrow(/sorted/);
    expect(() => validateDirectoryNode(node([entry("a", "file", 0n), entry("a", "directory", 0n)]))).toThrow(
      /sorted/,
    );
    expect(() => validateDirectoryNode(node([entry("d", "directory", 5n)]))).toThrow(/size 5/);
    expect(() => validateDirectoryNode(node([entry("..", "file", 0n)]))).toThrow(/invalid name/);
    expect(() => validateDirectoryNode(node([], [{ key: te.encode("k"), value: te.encode("v") }]))).toThrow(
      /metadata/,
    );
  });

  it("rejects invalid names", () => {
    const names = [new Uint8Array(0), te.encode("."), te.encode("a/b"), new Uint8Array([0xff]), te.encode("x".repeat(257))];
    for (const name of names) {
      expect(() => validateDirectoryNode(node([entry(name, "file", 0n)]))).toThrow(/invalid name/);
    }
  });

  it("accepts a valid directory", () => {
    validateDirectoryNode(VECTOR_DIRECTORY);
    validateDirectoryNode(node([entry("B", "file", 3n), entry("a", "directory", 0n), entry("é", "file", 0n)]));
  });
});

describe("format limits", () => {
  it("maximum encoded sizes match Rust MaxEncodedLen", () => {
    expect(DIRECTORY_NODE_MAX_SIZE).toBe(335116);
    expect(FILE_MANIFEST_MAX_SIZE).toBe(2380698);
  });

  it("isWellFormedString rejects lone surrogates", () => {
    expect(isWellFormedString("a😀é")).toBe(true);
    expect(isWellFormedString("a\ud800")).toBe(false);
    expect(isWellFormedString("\udc00b")).toBe(false);
  });
});
