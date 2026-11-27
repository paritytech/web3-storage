// SPDX-License-Identifier: Apache-2.0

import { describe, expect, it } from "vitest";
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
import { computeDataRoot, hashLeaf } from "./merkle.js";
import { ScaleError } from "./scale.js";
import { hexToBytes, toHex } from "./bytes.js";

const te = new TextEncoder();

// Shared with the Rust implementation (spec section 6).
const VECTOR_DIRECTORY: DirectoryNode = {
  bucketId: 7n,
  children: [
    { name: te.encode("a.txt"), entryType: "file", cid: hashLeaf(te.encode("a")), size: 1n, mtime: 1700000000n },
    { name: te.encode("docs"), entryType: "directory", cid: hashLeaf(te.encode("d")), size: 0n, mtime: 1700000001n },
  ],
  metadata: [],
};
const VECTOR_DIRECTORY_HEX =
  "0x07000000000000000814612e747874007234082e1dd0b5ec0acd71875d61c9f374af30c100bc4de7aa4eb3f15bbed686" +
  "010000000000000000f153650000000010646f6373019fa96117cda5187dd7699a57a16cbf37bb3353c1385e7854ab6e13" +
  "1df63cc659000000000000000001f153650000000000";
const VECTOR_DIRECTORY_CID = "0xdc2e9cfcc2612da5c6090a9d8f1ff953ef6f14fbb19a9e32a65bdffb990e50a5";

const VECTOR_MANIFEST: FileManifest = {
  bucketId: 7n,
  mimeType: te.encode("text/plain"),
  totalSize: 1n,
  chunks: [{ cid: hashLeaf(te.encode("a")), sequence: 0 }],
  encryptionParams: new Uint8Array(0),
  userMetadata: [{ key: te.encode("origin"), value: te.encode("test") }],
};
const VECTOR_MANIFEST_HEX =
  "0x070000000000000028746578742f706c61696e0100000000000000047234082e1dd0b5ec0acd71875d61c9f374af30c1" +
  "00bc4de7aa4eb3f15bbed686000000000004186f726967696e1074657374";
const VECTOR_MANIFEST_CID = "0xf7ea0f16d55dedab276a539b951c87b065a0d90c4f8c99510545bd2afc04e62e";

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
    bucketId: 1n,
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
