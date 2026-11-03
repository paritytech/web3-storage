// SPDX-License-Identifier: Apache-2.0

import { describe, expect, it } from "vitest";
import { blake2b256 } from "@polkadot-labs/hdkd-helpers";
import {
  bagPeaks,
  computeDataRoot,
  DEFAULT_CHUNK_SIZE,
  hashChildren,
  hashLeaf,
  LEAF_VERSION,
  leafChunkCount,
  merkleRoot,
  metadataMerkleRoot,
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

  it("moves the odd leaf of a 3-chunk root up a level", () => {
    expect(toHex(merkleRoot([leafA, leafB, leafC]))).toBe(
      "0x17321db51c1ef3ec1f77e271aa300b4e5c6091708bcba37e46025774a26142ee",
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
      "5632fced358a72ed6fd53ada497c9980a010c7e82c8386a10c12f2f27f9195f3",
      "56d7f37e08c1fac178a24ff1a5cc3413e35e4816ad2ce2a79120f3072bc9c618",
      "226e43e73fc2eb5cb50dc8f74106275685bf54f1aabd6e07e67c285f6f1032f9",
      "96c211dab54f3b6abd7a0f240df556dc810fc28d216c3b60dde7f491bbb5805e",
      "ea39f530b256e22bbff141464469eca0da922437cbab3e08fc706ac147ddc002",
      "470b3383b4658c30ba2466cc75dba6f0316fbe66e0e711708b47a85650f803b1",
      "734ed4bd23af21a41d23be13e369e592f49499fd2ef89e1b33b96a4540daa0f7",
      "c487902011078c0f151b8986afe18f487d7c69f46d58fd4ce293ade5b2a40ce5",
    ];
    golden.forEach((expected, i) => {
      const leaves = Array.from({ length: i + 1 }, (_, j) => blake2b256(Uint8Array.of(j + 1)));
      expect(toHex(merkleRoot(leaves)).replace(/^0x/, "")).toBe(expected);
    });
  });
});

describe("leafChunkCount", () => {
  it("rounds up with the chunk size of the leaf version and is zero for empty data", () => {
    const chunk = DEFAULT_CHUNK_SIZE;
    const count = (data_size: number | bigint) =>
      leafChunkCount({ leaf_version: LEAF_VERSION, data_size });
    expect(count(0)).toBe(0);
    expect(count(1)).toBe(1);
    expect(count(chunk)).toBe(1);
    expect(count(BigInt(chunk + 1))).toBe(2);
    expect(count(2 * chunk + 10)).toBe(3);
  });

  it("counts a u64::MAX data size exactly", () => {
    const max = (1n << 64n) - 1n;
    const chunk = BigInt(DEFAULT_CHUNK_SIZE);
    expect(leafChunkCount({ leaf_version: LEAF_VERSION, data_size: max })).toBe(
      Number((max + chunk - 1n) / chunk),
    );
  });

  it("throws for an unknown leaf version", () => {
    expect(() => leafChunkCount({ leaf_version: LEAF_VERSION + 1, data_size: 1 })).toThrow(
      "unknown MMR leaf version",
    );
  });
});

describe("computeDataRoot", () => {
  it("empty data has no chunk tree", () => {
    expect(() => computeDataRoot(new Uint8Array(0))).toThrow("empty data has no chunk tree");
  });

  it("single sub-chunk file → its own chunk hash", () => {
    const bytes = new TextEncoder().encode("hello world");
    expect(toHex(computeDataRoot(bytes))).toBe(toHex(hashLeaf(bytes)));
  });

  it("multi-chunk file folds chunk hashes via the Merkle tree", () => {
    const chunk = DEFAULT_CHUNK_SIZE;
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
