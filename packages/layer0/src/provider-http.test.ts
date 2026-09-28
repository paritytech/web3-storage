// SPDX-License-Identifier: Apache-2.0

import { afterEach, describe, expect, it, vi } from "vitest";
import { fetchBucketUsage } from "./provider-http.js";

const U64_MAX = "18446744073709551615";

function stubBucketsResponse(buckets: unknown[]) {
  vi.stubGlobal(
    "fetch",
    vi.fn(async () => ({
      ok: true,
      json: async () => ({ buckets }),
    })),
  );
}

afterEach(() => {
  vi.unstubAllGlobals();
});

describe("fetchBucketUsage", () => {
  it("maps used/max bytes and detects a synced quota", async () => {
    stubBucketsResponse([
      { bucket_id: 7, used_bytes: 4096, max_bytes: 10485760 },
    ]);
    const usage = await fetchBucketUsage("http://provider", 7n);
    expect(usage.usedBytes).toBe(4096n);
    expect(usage.maxBytes).toBe(10485760n);
    expect(usage.quotaSynced).toBe(true);
  });

  it("treats the u64::MAX sentinel as quota-not-synced", async () => {
    // JSON.parse turns u64::MAX into an imprecise float; the helper must
    // still classify it as the unlimited sentinel.
    stubBucketsResponse([
      { bucket_id: 1, used_bytes: 0, max_bytes: JSON.parse(U64_MAX) },
    ]);
    const usage = await fetchBucketUsage("http://provider", 1);
    expect(usage.quotaSynced).toBe(false);
    expect(usage.usedBytes).toBe(0n);
  });

  it("throws when the provider does not know the bucket", async () => {
    stubBucketsResponse([]);
    await expect(fetchBucketUsage("http://provider", 9n)).rejects.toThrow(
      "bucket 9 not found",
    );
  });
});
