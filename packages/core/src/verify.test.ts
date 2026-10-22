// SPDX-License-Identifier: Apache-2.0

import { describe, expect, it } from "vitest";
import { DEFAULT_CHUNK_SIZE, hashChildren, hashLeaf } from "./merkle.js";
import { CidMismatchError, computeCid, nodeHash, parseNode, verifyCid } from "./verify.js";
import { bytesToBase64, toHex } from "./bytes.js";

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

describe("parseNode and nodeHash", () => {
  const enc = new TextEncoder();
  const left = hashLeaf(enc.encode("left"));
  const right = hashLeaf(enc.encode("right"));
  const children = [toHex(left), toHex(right)];
  const data = bytesToBase64(enc.encode("chunk"));

  it("parses data without children as a chunk and hashes it as a leaf", () => {
    const node = parseNode({ data, children: null });
    expect(node).toEqual({ data: enc.encode("chunk") });
    expect(nodeHash(node!)).toEqual(hashLeaf(enc.encode("chunk")));
  });

  it("parses two children without data as an internal node and hashes it as a node", () => {
    const node = parseNode({ children });
    expect(node).toEqual({ children: [left, right] });
    expect(nodeHash(node!)).toEqual(hashChildren(left, right));
  });

  it("returns null for a wrong child count, both data and children, or neither", () => {
    expect(parseNode({ children: [toHex(left)] })).toBeNull();
    expect(parseNode({ data, children })).toBeNull();
    expect(parseNode({})).toBeNull();
  });

  it("returns null for children that are not 32-byte hex hashes", () => {
    // Same bytes as [left, right], boundary shifted by one byte.
    const shifted = [toHex(new Uint8Array([...left, right[0]])), toHex(right.slice(1))];
    expect(parseNode({ children: shifted })).toBeNull();
    expect(parseNode({ children: [toHex(left), "0x" + "zz".repeat(32)] })).toBeNull();
  });
});
