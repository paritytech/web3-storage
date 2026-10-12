// SPDX-License-Identifier: Apache-2.0

import { describe, expect, it, vi } from "vitest";

import { getBucketInfos, listMemberBuckets } from "./bucket-info.js";

type Bucket = {
  members: { account: string; role: { type: string } }[];
  visibility: { type: string };
  frozen_start_seq?: bigint;
  primary_providers: string[];
};

function makeApi(opts: {
  buckets: Record<string, Bucket>;
  agreements?: Record<string, { max_bytes: bigint; expires_at: number }>;
  memberBuckets?: bigint[];
}) {
  const multiaddr = new TextEncoder().encode("/ip4/127.0.0.1/tcp/3333");
  const getBuckets = vi.fn(async (keys: [bigint][]) =>
    keys.map(([id]) => opts.buckets[id.toString()]),
  );
  return {
    getBuckets,
    api: {
      query: {
        StorageProvider: {
          Buckets: { getValues: getBuckets },
          StorageAgreements: {
            getValues: vi.fn(async (keys: [bigint, string][]) =>
              keys.map(([id, provider]) => opts.agreements?.[`${id}:${provider}`]),
            ),
          },
          Providers: {
            getValues: vi.fn(async (keys: [string][]) => keys.map(() => ({ multiaddr }))),
          },
          MemberBuckets: { getValue: vi.fn(async () => opts.memberBuckets) },
        },
      },
    } as never,
  };
}

const bucket = (overrides: Partial<Bucket> = {}): Bucket => ({
  members: [
    { account: "alice", role: { type: "Admin" } },
    { account: "bob", role: { type: "Writer" } },
    { account: "carol", role: { type: "Reader" } },
  ],
  visibility: { type: "Private" },
  primary_providers: [],
  ...overrides,
});

describe("getBucketInfos", () => {
  it("aligns results with the ids and returns null for a missing bucket", async () => {
    const { api } = makeApi({ buckets: { "1": bucket(), "3": bucket() } });
    const infos = await getBucketInfos(api, [1n, 2n, 3n]);
    expect(infos.map((i) => i?.bucketId ?? null)).toEqual([1n, null, 3n]);
  });

  it("maps roles, visibility and the frozen flag", async () => {
    const { api } = makeApi({
      buckets: { "1": bucket({ visibility: { type: "Public" }, frozen_start_seq: 4n }) },
    });
    const [info] = await getBucketInfos(api, [1n]);
    expect(info!.members.map((m) => m.role)).toEqual(["Admin", "Writer", "Reader"]);
    expect(info!.visibility).toBe("Public");
    expect(info!.frozen).toBe(true);
  });

  it("takes the largest max_bytes and the latest expiry across primaries", async () => {
    const { api } = makeApi({
      buckets: { "1": bucket({ primary_providers: ["p1", "p2"] }) },
      agreements: {
        "1:p1": { max_bytes: 100n, expires_at: 50 },
        "1:p2": { max_bytes: 40n, expires_at: 90 },
      },
    });
    const [info] = await getBucketInfos(api, [1n]);
    expect(info!.maxCapacity).toBe(100n);
    expect(info!.expiresAt).toBe(90);
    expect(info!.providerInfo.map((p) => p.account)).toEqual(["p1", "p2"]);
    expect(info!.providerInfo[0]!.url).toBe("http://127.0.0.1:3333");
  });

  it("returns 0n capacity and null expiry without an agreement", async () => {
    const { api } = makeApi({ buckets: { "1": bucket() } });
    const [info] = await getBucketInfos(api, [1n]);
    expect(info!.frozen).toBe(false);
    expect(info!.maxCapacity).toBe(0n);
    expect(info!.expiresAt).toBeNull();
  });

  it("reads Buckets once", async () => {
    const { api, getBuckets } = makeApi({
      buckets: { "1": bucket({ primary_providers: ["p1"] }) },
    });
    await getBucketInfos(api, [1n]);
    expect(getBuckets).toHaveBeenCalledTimes(1);
  });
});

describe("listMemberBuckets", () => {
  it("returns an empty list when the account is in no bucket", async () => {
    const { api } = makeApi({ buckets: {}, memberBuckets: undefined });
    expect(await listMemberBuckets(api, "alice")).toEqual([]);
  });

  it("skips ids whose bucket no longer exists", async () => {
    const { api } = makeApi({ buckets: { "2": bucket() }, memberBuckets: [1n, 2n] });
    const infos = await listMemberBuckets(api, "alice");
    expect(infos.map((i) => i.bucketId)).toEqual([2n]);
  });
});
