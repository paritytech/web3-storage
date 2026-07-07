// SPDX-License-Identifier: Apache-2.0

import { describe, expect, it } from "vitest";
import { asHex, base64ToBytes, bytesEq, bytesToBase64, hexToBytes, toHex } from "./bytes.js";

describe("hex", () => {
  it("round-trips bytes", () => {
    const bytes = new Uint8Array([0, 1, 0xab, 0xff]);
    expect(toHex(bytes)).toBe("0x0001abff");
    expect(hexToBytes("0x0001abff")).toEqual(bytes);
    expect(hexToBytes("0001abff")).toEqual(bytes);
  });

  it("asHex normalizes strings and bytes", () => {
    expect(asHex("abcd")).toBe("0xabcd");
    expect(asHex("0xabcd")).toBe("0xabcd");
    expect(asHex(new Uint8Array([0xab, 0xcd]))).toBe("0xabcd");
  });

  it("bytesEq compares by content and rejects null/length mismatch", () => {
    expect(bytesEq(new Uint8Array([1, 2]), new Uint8Array([1, 2]))).toBe(true);
    expect(bytesEq(new Uint8Array([1, 2]), new Uint8Array([1, 3]))).toBe(false);
    expect(bytesEq(new Uint8Array([1]), new Uint8Array([1, 2]))).toBe(false);
    expect(bytesEq(null, new Uint8Array([1]))).toBe(false);
  });
});

describe("base64", () => {
  it("round-trips small payloads", () => {
    const bytes = new TextEncoder().encode("hello base64");
    expect(base64ToBytes(bytesToBase64(bytes))).toEqual(bytes);
  });

  it("round-trips payloads beyond the fromCharCode chunk size (>64KiB)", () => {
    const big = new Uint8Array(100_000);
    for (let i = 0; i < big.length; i++) big[i] = i % 251;
    expect(base64ToBytes(bytesToBase64(big))).toEqual(big);
  });
});
