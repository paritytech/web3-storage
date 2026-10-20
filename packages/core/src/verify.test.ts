// SPDX-License-Identifier: Apache-2.0

import { describe, expect, it } from "vitest";
import { DEFAULT_CHUNK_SIZE, hashChildren, hashLeaf } from "./merkle.js";
import { CidMismatchError, computeCid, nodeHash, verifyCid } from "./verify.js";
import { toHex } from "./bytes.js";

describe("verifyCid", () => {
  const data = new TextEncoder().encode("content addressed");

  it("passes when the data hashes to the expected cid (hex or bytes)", () => {
    const cid = computeCid(data);
    expect(() => verifyCid(data, cid)).not.toThrow();
    expect(() => verifyCid(data, toHex(cid))).not.toThrow();
    expect(() => verifyCid(data, toHex(cid).slice(2))).not.toThrow();
  });

  it("throws CidMismatchError with both hashes on mismatch", () => {
    const wrong = computeCid(new TextEncoder().encode("other"));
    try {
      verifyCid(data, wrong);
      expect.unreachable("should have thrown");
    } catch (err) {
      const e = err as CidMismatchError;
      expect(e).toBeInstanceOf(CidMismatchError);
      expect(e.expected).toBe(toHex(wrong));
      expect(e.actual).toBe(toHex(computeCid(data)));
    }
  });

  it("mirrors the runtime chunk size", () => {
    expect(DEFAULT_CHUNK_SIZE).toBe(256 * 1024);
  });
});

describe("nodeHash", () => {
  const enc = new TextEncoder();
  const left = hashLeaf(enc.encode("left"));
  const right = hashLeaf(enc.encode("right"));
  const children = [toHex(left), toHex(right)];

  it("hashes data without children as a leaf", () => {
    expect(nodeHash(enc.encode("chunk"), null)).toEqual(hashLeaf(enc.encode("chunk")));
  });

  it("hashes two children without data as a node", () => {
    expect(nodeHash(null, children)).toEqual(hashChildren(left, right));
  });

  it("returns null for a wrong child count, both data and children, or neither", () => {
    expect(nodeHash(null, [toHex(left)])).toBeNull();
    expect(nodeHash(enc.encode("other"), children)).toBeNull();
    expect(nodeHash(null, null)).toBeNull();
  });
});
