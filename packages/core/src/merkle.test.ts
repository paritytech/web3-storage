// SPDX-License-Identifier: Apache-2.0

import { describe, expect, it } from "vitest";
import { blake2b256 } from "@polkadot-labs/hdkd-helpers";
import {
  computeDataRoot,
  hashChildren,
  metadataMerkleRoot,
  paddedMerkleRoot,
  paddedMerkleTree,
  splitChunks,
  u64le,
  verifyChunkProof,
  verifyLastMmrLeaf,
  verifyMmrProof,
  type MerkleEntry,
} from "./merkle.js";
import { computeCid } from "./verify.js";
import { toHex } from "./bytes.js";

const ZERO32 = new Uint8Array(32);

describe("paddedMerkleRoot", () => {
  it("empty → 32 zero bytes", () => {
    expect(toHex(paddedMerkleRoot([]))).toBe(toHex(ZERO32));
  });

  it("single leaf → that leaf verbatim", () => {
    const leaf = computeCid(new TextEncoder().encode("leaf"));
    expect(toHex(paddedMerkleRoot([leaf]))).toBe(toHex(leaf));
  });

  it("two leaves → hash of the pair; matches hashChildren", () => {
    const a = computeCid(new TextEncoder().encode("a"));
    const b = computeCid(new TextEncoder().encode("b"));
    expect(toHex(paddedMerkleRoot([a, b]))).toBe(toHex(hashChildren(a, b)));
  });

  it("three leaves pad to four with a zero hash", () => {
    const a = computeCid(new TextEncoder().encode("a"));
    const b = computeCid(new TextEncoder().encode("b"));
    const c = computeCid(new TextEncoder().encode("c"));
    const expected = hashChildren(hashChildren(a, b), hashChildren(c, ZERO32));
    expect(toHex(paddedMerkleRoot([a, b, c]))).toBe(toHex(expected));
  });

  // Same vector as `padded_merkle_tree_matches_ts_vector` in
  // crates/primitives/storage, so the Rust and TS trees stay identical.
  it("matches the Rust padded_merkle_tree vector", () => {
    const leaf = (n: number) => computeCid(new Uint8Array([n]));
    expect(toHex(paddedMerkleRoot([leaf(1), leaf(2), leaf(3)]))).toBe(
      "0xb00ba4725e9a6c357dfffcf32d0ea9f2b73418705bdab4ea5a602cac23fbc650",
    );
  });
});

describe("computeDataRoot", () => {
  it("empty file hashes a single empty chunk", () => {
    expect(toHex(computeDataRoot(new Uint8Array(0)))).toBe(toHex(blake2b256(new Uint8Array(0))));
  });

  it("single sub-chunk file → its own chunk hash", () => {
    const bytes = new TextEncoder().encode("hello world");
    expect(toHex(computeDataRoot(bytes))).toBe(toHex(blake2b256(bytes)));
  });

  it("multi-chunk file folds chunk hashes via the padded tree", () => {
    const chunk = 256 * 1024;
    const bytes = new Uint8Array(chunk + 100).fill(7);
    const h0 = blake2b256(bytes.subarray(0, chunk));
    const h1 = blake2b256(bytes.subarray(chunk));
    expect(toHex(computeDataRoot(bytes))).toBe(toHex(hashChildren(h0, h1)));
  });
});

describe("paddedMerkleTree", () => {
  const leaf = (n: number) => computeCid(new Uint8Array([n]));

  it("returns no nodes for zero or one leaf", () => {
    expect(paddedMerkleTree([]).nodes).toEqual([]);
    expect(paddedMerkleTree([leaf(1)])).toEqual({ root: leaf(1), nodes: [] });
  });

  it("returns the internal nodes bottom-up, including zero-padded ones", () => {
    const [a, b, c] = [leaf(1), leaf(2), leaf(3)];
    const ab = hashChildren(a, b);
    const c0 = hashChildren(c, ZERO32);
    const { root, nodes } = paddedMerkleTree([a, b, c]);
    expect(nodes).toEqual([
      { hash: ab, left: a, right: b },
      { hash: c0, left: c, right: ZERO32 },
      { hash: root, left: ab, right: c0 },
    ]);
  });

  it("five leaves produce a node with two zero children", () => {
    const leaves = [1, 2, 3, 4, 5].map(leaf);
    const { nodes } = paddedMerkleTree(leaves);
    expect(nodes).toHaveLength(7);
    expect(nodes[3]).toEqual({ hash: hashChildren(ZERO32, ZERO32), left: ZERO32, right: ZERO32 });
  });
});

describe("splitChunks", () => {
  it("splits at 256 KiB; empty input is one empty chunk", () => {
    expect(splitChunks(new Uint8Array(0))).toEqual([new Uint8Array(0)]);
    expect(splitChunks(new Uint8Array(256 * 1024 * 2 + 1)).map((c) => c.length)).toEqual([262144, 262144, 1]);
  });
});

describe("verifyChunkProof", () => {
  const leaves = [1, 2, 3].map((n) => computeCid(new Uint8Array([n])));
  const root = paddedMerkleRoot(leaves);
  const ab = hashChildren(leaves[0], leaves[1]);

  it("accepts the padded-tree proof and rejects a wrong index or depth", () => {
    expect(verifyChunkProof(leaves[2], 2, [ZERO32, ab], root, 3)).toBe(true);
    expect(verifyChunkProof(leaves[2], 3, [ZERO32, ab], root, 3)).toBe(false);
    expect(verifyChunkProof(leaves[2], 2, [ZERO32, ab], root, 5)).toBe(false);
    expect(verifyChunkProof(leaves[0], 0, [], leaves[0], 1)).toBe(true);
  });
});

describe("verifyMmrProof", () => {
  const leafHash = (l: { dataRoot: Uint8Array; dataSize: bigint; totalSize: bigint }) =>
    blake2b256(concat(l.dataRoot, u64le(l.dataSize), u64le(l.totalSize)));
  const leaves = [1, 2, 3].map((n) => ({
    dataRoot: computeCid(new Uint8Array([n])),
    dataSize: 1n,
    totalSize: BigInt(n),
  }));
  const hashes = leaves.map(leafHash);
  const peak0 = hashChildren(hashes[0], hashes[1]);
  const mmrRoot = hashChildren(peak0, hashes[2]);

  it("accepts a leaf under the first and the last peak", () => {
    expect(verifyMmrProof({ peaks: [peak0, hashes[2]], leaf: leaves[1], siblings: [hashes[0]], path: [true] }, mmrRoot)).toBe(true);
    expect(verifyMmrProof({ peaks: [peak0, hashes[2]], leaf: leaves[2], siblings: [], path: [] }, mmrRoot)).toBe(true);
  });

  it("verifyLastMmrLeaf accepts only the last leaf", () => {
    const peaks = [peak0, hashes[2]];
    expect(verifyLastMmrLeaf({ peaks, leaf: leaves[2], siblings: [], path: [] }, mmrRoot, 3n)).toBe(true);
    expect(verifyLastMmrLeaf({ peaks, leaf: leaves[1], siblings: [hashes[0]], path: [true] }, mmrRoot, 3n)).toBe(false);
    expect(verifyLastMmrLeaf({ peaks, leaf: leaves[2], siblings: [], path: [] }, mmrRoot, 4n)).toBe(false);
    // Two leaves: the last leaf is the right child under the single peak.
    const root2 = hashChildren(hashes[0], hashes[1]);
    expect(verifyLastMmrLeaf({ peaks: [root2], leaf: leaves[1], siblings: [hashes[0]], path: [true] }, root2, 2n)).toBe(true);
    expect(verifyLastMmrLeaf({ peaks: [root2], leaf: leaves[0], siblings: [hashes[1]], path: [false] }, root2, 2n)).toBe(false);
  });

  it("rejects a changed leaf or root", () => {
    const changed = { ...leaves[2], dataSize: 2n };
    expect(verifyMmrProof({ peaks: [peak0, hashes[2]], leaf: changed, siblings: [], path: [] }, mmrRoot)).toBe(false);
    expect(verifyMmrProof({ peaks: [peak0, hashes[2]], leaf: leaves[2], siblings: [], path: [] }, ZERO32)).toBe(false);
  });
});

describe("metadataMerkleRoot", () => {
  it("empty drive → 32 zero bytes", () => {
    expect(toHex(metadataMerkleRoot([]))).toBe(toHex(ZERO32));
  });

  it("orders entries by UTF-8 path bytes regardless of input order", () => {
    const enc = new TextEncoder();
    const leafFor = (e: MerkleEntry) =>
      blake2b256(concat(enc.encode(e.path), e.dataRoot, u64le(e.size)));
    const a: MerkleEntry = { path: "/a.jpg", dataRoot: ZERO32, size: 1n };
    const b: MerkleEntry = { path: "/b.jpg", dataRoot: ZERO32, size: 2n };
    const expected = toHex(paddedMerkleRoot([leafFor(a), leafFor(b)]));
    expect(toHex(metadataMerkleRoot([a, b]))).toBe(expected);
    expect(toHex(metadataMerkleRoot([b, a]))).toBe(expected);
  });
});

describe("u64le", () => {
  it("encodes little-endian", () => {
    expect(toHex(u64le(1n))).toBe("0x0100000000000000");
    expect(toHex(u64le(256n))).toBe("0x0001000000000000");
  });
});

function concat(...arrays: Uint8Array[]): Uint8Array {
  let total = 0;
  for (const a of arrays) total += a.length;
  const out = new Uint8Array(total);
  let off = 0;
  for (const a of arrays) {
    out.set(a, off);
    off += a.length;
  }
  return out;
}
