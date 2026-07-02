// SPDX-License-Identifier: GPL-3.0-only

import { describe, expect, it } from "vitest";
import { formatBytes } from "@/lib/utils";

// Byte units use binary (base 1024) — colloquial "GB" == 2^30 bytes,
// matching what most users mean when they type "1 GB".
describe("formatBytes", () => {
  it("formats zero and sub-KB values", () => {
    expect(formatBytes(0)).toBe("0 B");
    expect(formatBytes(1)).toBe("1 B");
    expect(formatBytes(500)).toBe("500 B");
    expect(formatBytes(1023)).toBe("1023 B");
  });

  it("uses base 1024 for unit transitions", () => {
    expect(formatBytes(1024)).toBe("1 KB");
    expect(formatBytes(1024 ** 2)).toBe("1 MB");
    expect(formatBytes(1024 ** 3)).toBe("1 GB");
    expect(formatBytes(1024 ** 4)).toBe("1 TB");
    expect(formatBytes(1024 ** 5)).toBe("1 PB");
    expect(formatBytes(1024 ** 6)).toBe("1 EB");
  });

  it("rejects SI gigabyte (10^9) as still being MB", () => {
    expect(formatBytes(1_000_000_000)).toBe("953.67 MB");
  });

  it("strips trailing zeros and rounds to two decimals", () => {
    expect(formatBytes(1500)).toBe("1.46 KB");
    expect(formatBytes(1024 * 1024 * 1.5)).toBe("1.5 MB");
  });

  it("accepts bigint input", () => {
    expect(formatBytes(1_073_741_824n)).toBe("1 GB");
  });

  it("clamps very large values to EB", () => {
    expect(formatBytes(1024 ** 7)).toBe("1024 EB");
  });
});
