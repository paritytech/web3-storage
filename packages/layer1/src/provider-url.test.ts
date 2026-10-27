// SPDX-License-Identifier: Apache-2.0

import { describe, expect, it, vi } from "vitest";

import { discoverAcceptingProvider, resolveCreationTerms } from "./provider-url.js";

const SIGNED = {
  terms: {
    owner: "5owner",
    max_bytes: "1024",
    duration: 100,
    price_per_byte: "1",
    valid_until: 200,
    nonce: "0",
  },
  signature: "0x01" + "ab".repeat(64),
};

const utf8 = (s: string) => new TextEncoder().encode(s);

interface ProviderEntry {
  keyArgs: [string];
  value: {
    settings: {
      accepting_primary: boolean;
      min_bytes: bigint;
      max_capacity: bigint;
      min_duration: number;
      max_duration: number;
    };
    committed_bytes: bigint;
    deregister_at: number | undefined;
    multiaddr: Uint8Array;
  };
}

/** A provider that accepts 1024 bytes for 100 blocks; override fields to make it ineligible. */
function entry(
  address: string,
  port: number,
  over: Partial<ProviderEntry["value"]["settings"]> & {
    committed_bytes?: bigint;
    deregister_at?: number;
  } = {},
): ProviderEntry {
  const { committed_bytes = 0n, deregister_at, ...settings } = over;
  return {
    keyArgs: [address],
    value: {
      settings: {
        accepting_primary: true,
        min_bytes: 0n,
        max_capacity: 0n,
        min_duration: 10,
        max_duration: 1000,
        ...settings,
      },
      committed_bytes,
      deregister_at,
      multiaddr: utf8(`/ip4/127.0.0.1/tcp/${port}`),
    },
  };
}

function fakeApi(opts: { entries?: ProviderEntry[]; byAddress?: Record<string, unknown> } = {}) {
  return {
    query: {
      StorageProvider: {
        Providers: {
          getEntries: vi.fn(async () => opts.entries ?? []),
          getValue: vi.fn(async (addr: string) => opts.byAddress?.[addr]),
        },
        AgreementNonces: {
          getValue: vi.fn(async () => 0n),
        },
      },
    },
  } as never;
}

function jsonFetch() {
  const calls: Array<{ url: string; init?: RequestInit }> = [];
  const fetchImpl = vi.fn(async (url: RequestInfo | URL, init?: RequestInit) => {
    calls.push({ url: String(url), init });
    return new Response(JSON.stringify(SIGNED), { status: 200 });
  });
  return { calls, fetchImpl };
}

describe("resolveCreationTerms", () => {
  it("negotiates against an explicit provider URL and returns provider + signed terms", async () => {
    const { calls, fetchImpl } = jsonFetch();
    const res = await resolveCreationTerms(fakeApi(), {
      owner: "5own",
      maxBytes: 1024n,
      duration: 100,
      provider: { address: "5prov", url: "http://prov.test" },
      fetchOpts: { fetchImpl: fetchImpl as never },
    });
    expect(calls[0].url).toBe("http://prov.test/negotiate");
    // bigint terms are serialized as decimal strings on the wire.
    expect(JSON.parse(String(calls[0].init!.body))).toMatchObject({
      owner: "5own",
      max_bytes: "1024",
      duration: 100,
      nonce: "0",
    });
    expect(res.provider.address).toBe("5prov");
    expect(res.signedTerms).toEqual(SIGNED);
  });

  it("reads an address-only provider's entry once for both its URL and price", async () => {
    const { calls, fetchImpl } = jsonFetch();
    const api = fakeApi({
      byAddress: {
        "5prov": {
          settings: { accepting_primary: true, price_per_byte: 7n },
          multiaddr: utf8("/ip4/127.0.0.1/tcp/3333"),
        },
      },
    });
    await resolveCreationTerms(api, {
      owner: "5own",
      maxBytes: 1024n,
      duration: 100,
      provider: { address: "5prov" },
      fetchOpts: { fetchImpl: fetchImpl as never },
    });
    expect(calls[0].url).toBe("http://127.0.0.1:3333/negotiate");
    expect(JSON.parse(String(calls[0].init!.body))).toMatchObject({ price_per_byte: "7" });
    expect(
      (api as { query: { StorageProvider: { Providers: { getValue: { mock: { calls: unknown[] } } } } } })
        .query.StorageProvider.Providers.getValue.mock.calls,
    ).toHaveLength(1);
  });

  it("discovers a provider that fits the requested size and duration", async () => {
    const { calls, fetchImpl } = jsonFetch();
    const api = fakeApi({ entries: [entry("5small", 1, { min_bytes: 4096n }), entry("5fits", 2)] });
    const res = await resolveCreationTerms(api, {
      owner: "5own",
      maxBytes: 1024n,
      duration: 100,
      fetchOpts: { fetchImpl: fetchImpl as never },
    });
    expect(res.provider.address).toBe("5fits");
    expect(calls[0].url).toBe("http://127.0.0.1:2/negotiate");
  });

  it("uses pre-negotiated signedTerms without an HTTP round-trip", async () => {
    const { calls, fetchImpl } = jsonFetch();
    const res = await resolveCreationTerms(fakeApi(), {
      owner: "5own",
      maxBytes: 1n,
      duration: 1,
      provider: { address: "5prov" },
      signedTerms: SIGNED as never,
      fetchOpts: { fetchImpl: fetchImpl as never },
    });
    expect(calls).toHaveLength(0);
    expect(res).toEqual({ provider: { address: "5prov" }, signedTerms: SIGNED });
  });

  it("rejects signedTerms without a provider address", async () => {
    await expect(
      resolveCreationTerms(fakeApi(), {
        owner: "5own",
        maxBytes: 1n,
        duration: 1,
        signedTerms: SIGNED as never,
      }),
    ).rejects.toThrow(/provider\.address/);
  });
});

describe("discoverAcceptingProvider", () => {
  const want = { maxBytes: 1024n, duration: 100 };

  it("skips non-accepting providers and applies the URL override", async () => {
    const entries = [entry("5no", 1, { accepting_primary: false }), entry("5yes", 3333)];
    const choice = await discoverAcceptingProvider(fakeApi({ entries }), {
      ...want,
      urlOverride: "http://dev.test",
    });
    expect(choice).toEqual({ address: "5yes", url: "http://dev.test" });
  });

  it("resolves the URL from the registered multiaddr when no override is given", async () => {
    const choice = await discoverAcceptingProvider(fakeApi({ entries: [entry("5yes", 3333)] }), want);
    expect(choice).toEqual({ address: "5yes", url: "http://127.0.0.1:3333" });
  });

  it.each([
    ["deregistering", { deregister_at: 500 }],
    ["min_bytes above the request", { min_bytes: 1025n }],
    ["not enough free capacity", { max_capacity: 2000n, committed_bytes: 1000n }],
    ["duration below min_duration", { min_duration: 101 }],
    ["duration above max_duration", { max_duration: 99 }],
  ])("skips a provider with %s", async (_name, over) => {
    const entries = [entry("5bad", 1, over), entry("5good", 2)];
    const choice = await discoverAcceptingProvider(fakeApi({ entries }), want);
    expect(choice.address).toBe("5good");
  });

  it("accepts a provider at the exact boundaries", async () => {
    const entries = [
      entry("5edge", 1, {
        min_bytes: 1024n,
        max_capacity: 2048n,
        committed_bytes: 1024n,
        min_duration: 100,
        max_duration: 100,
      }),
    ];
    const choice = await discoverAcceptingProvider(fakeApi({ entries }), want);
    expect(choice.address).toBe("5edge");
  });

  it("throws naming the requested size and duration when none qualifies", async () => {
    await expect(discoverAcceptingProvider(fakeApi({ entries: [] }), want)).rejects.toThrow(
      /1024 bytes for 100 blocks/,
    );
  });
});
