// SPDX-License-Identifier: Apache-2.0

import { describe, expect, it } from "vitest";
import { blake2b256 } from "@polkadot-labs/hdkd-helpers";
import {
  chunkCount,
  computeDataRoot,
  hashChildren,
  merkleRoot,
  metadataMerkleRoot,
  u64le,
  type MerkleEntry,
} from "./merkle.js";
import { computeCid } from "./verify.js";
import { toHex } from "./bytes.js";

const ZERO32 = new Uint8Array(32);

describe("merkleRoot", () => {
  it("empty → 32 zero bytes", () => {
    expect(toHex(merkleRoot([]))).toBe(toHex(ZERO32));
  });

  it("single leaf → that leaf verbatim", () => {
    const leaf = computeCid(new TextEncoder().encode("leaf"));
    expect(toHex(merkleRoot([leaf]))).toBe(toHex(leaf));
  });

  it("two leaves → hash of the pair; matches hashChildren", () => {
    const a = computeCid(new TextEncoder().encode("a"));
    const b = computeCid(new TextEncoder().encode("b"));
    expect(toHex(merkleRoot([a, b]))).toBe(toHex(hashChildren(a, b)));
  });

  it("three leaves move the odd leaf up a level", () => {
    const a = computeCid(new TextEncoder().encode("a"));
    const b = computeCid(new TextEncoder().encode("b"));
    const c = computeCid(new TextEncoder().encode("c"));
    const expected = hashChildren(hashChildren(a, b), c);
    expect(toHex(merkleRoot([a, b, c]))).toBe(toHex(expected));
  });

  // Roots over blake2b256([i]) for i in 1..=n, from the same independent
  // computation as the Rust golden vectors in crates/primitives/storage/src/merkle.rs.
  it("matches the Rust golden roots for 1 to 9 leaves", () => {
    const golden = [
      "ee155ace9c40292074cb6aff8c9ccdd273c81648ff1149ef36bcea6ebb8a3e25",
      "5a3746f7ad1bece55c75e3cd11d299fe5f3e4f4fb03035e3b8d87d182d705de7",
      "e0d2cb92e481829535abbcab54c8fd72f5a8d9787d37312d3679f1a745b5effb",
      "114d5dc3c70125a2159b283e59b8ac4330014aa80cccdad51d5ee18f02c37a7e",
      "9a8a0313d2a3e6190c5619884c1fb351e52ed99f96bb93253b6dcb77765ebf63",
      "25875908add04dfb7035e7e8ff383fcc92b2586bbe6c64dc0d607902366e7b48",
      "bfd907b4476ee8bda6531809a94c59149108b1881c253746530b10c1354b4843",
      "e0ae36209c0562a4d8075c37a23572e393ab3c3e9e95b6e061f8c203b06f7f27",
      "3a60ffa28abb95e837960000c1e9644b08084c5ac838aadc103a6f5f1f840ced",
    ];
    golden.forEach((expected, i) => {
      const leaves = Array.from({ length: i + 1 }, (_, j) => blake2b256(Uint8Array.of(j + 1)));
      expect(toHex(merkleRoot(leaves)).replace(/^0x/, "")).toBe(expected);
    });
  });
});

describe("chunkCount", () => {
  it("rounds up and has a minimum of one", () => {
    const chunk = 256 * 1024;
    expect(chunkCount(0)).toBe(1);
    expect(chunkCount(1)).toBe(1);
    expect(chunkCount(chunk)).toBe(1);
    expect(chunkCount(chunk + 1)).toBe(2);
    expect(chunkCount(2 * chunk + 10)).toBe(3);
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

  it("multi-chunk file folds chunk hashes via the Merkle tree", () => {
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
    const expected = toHex(merkleRoot([leafFor(a), leafFor(b)]));
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
