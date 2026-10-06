// SPDX-License-Identifier: Apache-2.0

import { describe, expect, it } from "vitest";
import { blake2b256 } from "@polkadot-labs/hdkd-helpers";
import {
  bagPeaks,
  computeDataRoot,
  hashChildren,
  hashLeaf,
  metadataMerkleRoot,
  paddedMerkleRoot,
  u64le,
  type MerkleEntry,
} from "./merkle.js";
import { computeCid } from "./verify.js";
import { concatBytes, toHex } from "./bytes.js";

const ZERO32 = new Uint8Array(32);

// Expected values come from an independent blake2b-256 computation over the
// documented preimages; the Rust tests in crates/primitives/storage assert the
// same leaf, node and root values.
describe("golden vectors", () => {
  const enc = new TextEncoder();
  const leafA = hashLeaf(enc.encode("a"));
  const leafB = hashLeaf(enc.encode("b"));
  const leafC = hashLeaf(enc.encode("c"));

  it("hashes a leaf with the 0x00 prefix", () => {
    expect(toHex(hashLeaf(new Uint8Array(0)))).toBe(
      "0x03170a2e7597b7b7e3d84c05391d139a62b157e78786d8c082f29dcf4c111314",
    );
    expect(toHex(hashLeaf(enc.encode("abc")))).toBe(
      "0x4b44b5a5f9e6fafead231e4d609a8e88053a6053c087b68e24e31faf0fb8dfe7",
    );
    expect(toHex(computeCid(enc.encode("abc")))).toBe(toHex(hashLeaf(enc.encode("abc"))));
    expect(toHex(hashLeaf(enc.encode("abc")))).not.toBe(toHex(blake2b256(enc.encode("abc"))));
    expect(toHex(leafA)).toBe("0x7234082e1dd0b5ec0acd71875d61c9f374af30c100bc4de7aa4eb3f15bbed686");
    expect(toHex(leafB)).toBe("0xb3d5dedf654e9fc853bdc5daf79330c5a1eaf2b910f2a36c72ef8ea999ccf953");
    expect(toHex(leafC)).toBe("0x960259f5c0885e7b7967cc25158bc9069db1ca8222e7beffdfffe5dea0297966");
  });

  it("hashes an internal node with the 0x01 prefix", () => {
    expect(toHex(hashChildren(leafA, leafB))).toBe(
      "0xee616625a590167bc4b3dc703ab4f3f2ddecbee6b9d05fee9281f02046e6082e",
    );
  });

  it("pads a 3-chunk root with an untagged zero leaf", () => {
    expect(toHex(paddedMerkleRoot([leafA, leafB, leafC]))).toBe(
      "0xa3dd32d607debce875c8dcfb1417d07c9bc4c5cccd0bacefd0a4a9473d958e37",
    );
  });

  it("tags metadata leaves and nodes", () => {
    const entries: MerkleEntry[] = [
      { path: "/a.jpg", dataRoot: leafA, size: 1n },
      { path: "/b.jpg", dataRoot: leafB, size: 2n },
    ];
    expect(toHex(metadataMerkleRoot(entries))).toBe(
      "0xa179f8563e699338a92ca5e7836273a048160ed61ce8bd4eb9af12b5b275da94",
    );
  });

  it("does not let node bytes verify as a leaf", () => {
    const nodeBytes = concatBytes(leafA, leafB);
    expect(toHex(hashLeaf(nodeBytes))).not.toBe(toHex(hashChildren(leafA, leafB)));
  });

  it("bags MMR peaks right to left with the 0x02 prefix", () => {
    const peaks = [1, 2, 3].map((b) => new Uint8Array(32).fill(b));
    expect(toHex(bagPeaks(peaks.slice(0, 2)))).toBe(
      "0x39ffd1718a7a2d0f61de8f7902cb47d9661392d004d5b91e835ccd3d475cd035",
    );
    expect(toHex(bagPeaks(peaks))).toBe(
      "0xde6b5def5927714c7e957d45b39ba6b3cb1b24c08d3d72e038387f46268eb23f",
    );
  });

  it("returns one peak as the root and zero for no peaks", () => {
    const peak = new Uint8Array(32).fill(7);
    expect(bagPeaks([peak])).toEqual(peak);
    expect(bagPeaks([])).toEqual(ZERO32);
  });
});

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
    expect(toHex(computeDataRoot(new Uint8Array(0)))).toBe(toHex(hashLeaf(new Uint8Array(0))));
  });

  it("single sub-chunk file → its own chunk hash", () => {
    const bytes = new TextEncoder().encode("hello world");
    expect(toHex(computeDataRoot(bytes))).toBe(toHex(hashLeaf(bytes)));
  });

  it("multi-chunk file folds chunk hashes via the padded tree", () => {
    const chunk = 256 * 1024;
    const bytes = new Uint8Array(chunk + 100).fill(7);
    const h0 = hashLeaf(bytes.subarray(0, chunk));
    const h1 = hashLeaf(bytes.subarray(chunk));
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
      hashLeaf(concatBytes(enc.encode(e.path), e.dataRoot, u64le(e.size)));
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
