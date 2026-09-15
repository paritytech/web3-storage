// SPDX-License-Identifier: Apache-2.0

import { describe, expect, it } from "vitest";
import { blake2b256 } from "@polkadot-labs/hdkd-helpers";
import {
  computeDataRoot,
  hashChildren,
  metadataMerkleRoot,
  paddedMerkleRoot,
  u64le,
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
