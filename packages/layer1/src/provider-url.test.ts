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
  value: { settings: { accepting_primary: boolean }; multiaddr: Uint8Array };
}

function fakeApi(opts: { entries?: ProviderEntry[]; byAddress?: Record<string, unknown> } = {}) {
  return {
    query: {
      StorageProvider: {
        Providers: {
          getEntries: vi.fn(async () => opts.entries ?? []),
          getValue: vi.fn(async (addr: string) => opts.byAddress?.[addr]),
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
    });
    expect(res.provider.address).toBe("5prov");
    expect(res.signedTerms).toEqual(SIGNED);
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
  it("skips non-accepting providers and applies the URL override", async () => {
    const entries: ProviderEntry[] = [
      { keyArgs: ["5no"], value: { settings: { accepting_primary: false }, multiaddr: utf8("/ip4/127.0.0.1/tcp/1") } },
      { keyArgs: ["5yes"], value: { settings: { accepting_primary: true }, multiaddr: utf8("/ip4/127.0.0.1/tcp/3333") } },
    ];
    const choice = await discoverAcceptingProvider(fakeApi({ entries }), { urlOverride: "http://dev.test" });
    expect(choice).toEqual({ address: "5yes", url: "http://dev.test" });
  });

  it("resolves the URL from the registered multiaddr when no override is given", async () => {
    const entries: ProviderEntry[] = [
      { keyArgs: ["5yes"], value: { settings: { accepting_primary: true }, multiaddr: utf8("/ip4/127.0.0.1/tcp/3333") } },
    ];
    const choice = await discoverAcceptingProvider(fakeApi({ entries }));
    expect(choice).toEqual({ address: "5yes", url: "http://127.0.0.1:3333" });
  });

  it("throws when no provider is accepting primary agreements", async () => {
    await expect(discoverAcceptingProvider(fakeApi({ entries: [] }))).rejects.toThrow(
      /accepting primary/,
    );
  });
});
