import { describe, expect, it } from "vitest";
import { CidMismatchError, computeCid, DEFAULT_CHUNK_SIZE, verifyCid } from "./verify.js";
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
