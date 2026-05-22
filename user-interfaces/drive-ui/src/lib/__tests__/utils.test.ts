import { describe, expect, it } from "vitest";
import { formatBytes } from "@/lib/utils";

// Byte units use binary (base 1024) — colloquial "GB" == 2^30 bytes,
// matching what most users mean when they type "1 GB".
describe("formatBytes", () => {
  it("returns 0 Bytes for zero", () => {
    expect(formatBytes(0)).toBe("0 Bytes");
  });

  it("formats sub-KB values in Bytes", () => {
    expect(formatBytes(1)).toBe("1 Bytes");
    expect(formatBytes(500)).toBe("500 Bytes");
    expect(formatBytes(1023)).toBe("1023 Bytes");
  });

  it("uses base 1024 for unit transitions", () => {
    expect(formatBytes(1024)).toBe("1 KB");
    expect(formatBytes(1024 ** 2)).toBe("1 MB");
    expect(formatBytes(1024 ** 3)).toBe("1 GB");
    expect(formatBytes(1024 ** 4)).toBe("1 TB");
    expect(formatBytes(1024 ** 5)).toBe("1 PB");
  });

  it("rejects SI gigabyte (10^9) as still being MB", () => {
    expect(formatBytes(1_000_000_000)).toBe("953.67 MB");
  });

  it("rounds fractional values per the requested decimals", () => {
    expect(formatBytes(1500)).toBe("1.46 KB");
    expect(formatBytes(1024 * 1024 * 1.5)).toBe("1.5 MB");
    expect(formatBytes(1500, 0)).toBe("1 KB");
    expect(formatBytes(1024 ** 3 * 2.5, 1)).toBe("2.5 GB");
  });

  it("treats the documented '1 GB' default (2^30) as 1 GB", () => {
    expect(formatBytes(1_073_741_824)).toBe("1 GB");
  });
});
