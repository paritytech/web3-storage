// SPDX-License-Identifier: Apache-2.0

import { describe, expect, it } from "vitest";
import { ScaleError, ScaleReader, ScaleWriter } from "./scale.js";
import { hexToBytes, toHex } from "./bytes.js";

describe("SCALE compact", () => {
  const cases: [number, string][] = [
    [0, "0x00"],
    [1, "0x04"],
    [63, "0xfc"],
    [64, "0x0101"],
    [16383, "0xfdff"],
    [16384, "0x02000100"],
    [(1 << 30) - 1, "0xfeffffff"],
    [1 << 30, "0x0300000040"],
    [0xffff_ffff, "0x03ffffffff"],
  ];
  it.each(cases)("%i encodes to %s and back", (value, hex) => {
    expect(toHex(new ScaleWriter().compact(value).finish())).toBe(hex);
    const r = new ScaleReader(hexToBytes(hex));
    expect(r.compact()).toBe(value);
    r.end();
  });

  it("rejects non-canonical and over-u32 encodings", () => {
    expect(() => new ScaleReader(hexToBytes("0x0100")).compact()).toThrow(ScaleError);
    expect(() => new ScaleReader(hexToBytes("0x02000000")).compact()).toThrow(ScaleError);
    expect(() => new ScaleReader(hexToBytes("0x0300000000")).compact()).toThrow(ScaleError);
    expect(() => new ScaleReader(hexToBytes("0x070000000001")).compact()).toThrow(ScaleError);
  });
});

describe("SCALE fixed-width values", () => {
  it("encodes u32/u64 little-endian and H256 verbatim", () => {
    const h = new Uint8Array(32).fill(7);
    const bytes = new ScaleWriter().u8(1).u32(0x01020304).u64(0x0102030405060708n).h256(h).finish();
    expect(toHex(bytes.subarray(0, 13))).toBe("0x01040302010807060504030201");
    const r = new ScaleReader(bytes);
    expect([r.u8(), r.u32(), r.u64()]).toEqual([1, 0x01020304, 0x0102030405060708n]);
    expect(r.h256()).toEqual(h);
    r.end();
  });

  it("rejects out-of-range values", () => {
    expect(() => new ScaleWriter().u64(-1n)).toThrow(ScaleError);
    expect(() => new ScaleWriter().u64(1n << 64n)).toThrow(ScaleError);
    expect(() => new ScaleWriter().u32(2 ** 32)).toThrow(ScaleError);
    expect(() => new ScaleWriter().h256(new Uint8Array(31))).toThrow(ScaleError);
  });
});

describe("SCALE bounded vectors", () => {
  it("round-trips bytes and enforces the bound on encode and decode", () => {
    const bytes = new ScaleWriter().bytes(Uint8Array.of(1, 2, 3), 3, "f").finish();
    expect(toHex(bytes)).toBe("0x0c010203");
    expect(new ScaleReader(bytes).bytes(3, "f")).toEqual(Uint8Array.of(1, 2, 3));
    expect(() => new ScaleWriter().bytes(Uint8Array.of(1, 2, 3), 2, "f")).toThrow(/f is 3 bytes, max 2/);
    expect(() => new ScaleReader(bytes).bytes(2, "f")).toThrow(/f is 3 bytes, max 2/);
  });

  it("enforces item bounds and detects truncation and trailing bytes", () => {
    expect(() => new ScaleWriter().vec([1, 2], 1, "v", (w, x) => w.u8(x))).toThrow(/v has 2 items, max 1/);
    expect(() => new ScaleReader(hexToBytes("0x0801")).vec(4, "v", (r) => r.u8())).toThrow(/unexpected end/);
    const r = new ScaleReader(hexToBytes("0x0001"));
    r.vec(4, "v", (r) => r.u8());
    expect(() => r.end()).toThrow(/1 trailing bytes/);
  });
});
