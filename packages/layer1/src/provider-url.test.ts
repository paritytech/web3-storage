// SPDX-License-Identifier: Apache-2.0

import { describe, expect, it, vi } from "vitest";

import { toSs58 } from "@web3-storage/layer0";

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

interface ProviderMatch {
  account: Uint8Array;
  info: { multiaddr: Uint8Array };
  partial_reason: { type: string } | undefined;
}

/** A 32-byte account id filled with `seed`. */
const accountId = (seed: number) => new Uint8Array(32).fill(seed);

/** A `find_matching_providers` result; pass a reason to mark it a partial match. */
function match(seed: number, port: number, reason?: string): ProviderMatch {
  return {
    account: accountId(seed),
    info: { multiaddr: utf8(`/ip4/127.0.0.1/tcp/${port}`) },
    partial_reason: reason === undefined ? undefined : { type: reason },
  };
}

function fakeApi(opts: { matches?: ProviderMatch[]; byAddress?: Record<string, unknown> } = {}) {
  return {
    apis: {
      StorageProviderApi: {
        find_matching_providers: vi.fn(async () => opts.matches ?? []),
      },
    },
    query: {
      StorageProvider: {
        Providers: {
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
    const api = fakeApi({ matches: [match(1, 1, "BelowMinAgreementBytes"), match(2, 2)] });
    const res = await resolveCreationTerms(api, {
      owner: "5own",
      maxBytes: 1024n,
      duration: 100,
      fetchOpts: { fetchImpl: fetchImpl as never },
    });
    expect(res.provider.address).toBe(toSs58(accountId(2)));
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

  it("requests a primary agreement of the given size and duration with no price cap", async () => {
    const api = fakeApi({ matches: [match(1, 3333)] });
    await discoverAcceptingProvider(api, { ...want, readOpts: { at: "best" } });
    const call = (api as { apis: { StorageProviderApi: { find_matching_providers: { mock: { calls: unknown[][] } } } } })
      .apis.StorageProviderApi.find_matching_providers.mock.calls[0];
    expect(call[0]).toEqual({
      bytes_needed: 1024n,
      min_duration: 100,
      max_price_per_byte: (1n << 128n) - 1n,
      primary_only: true,
    });
    expect(call[2]).toEqual({ at: "best" });
  });

  it("skips partial matches and applies the URL override", async () => {
    const matches = [match(1, 1, "NotAccepting"), match(2, 3333)];
    const choice = await discoverAcceptingProvider(fakeApi({ matches }), {
      ...want,
      urlOverride: "http://dev.test",
    });
    expect(choice).toEqual({ address: toSs58(accountId(2)), url: "http://dev.test" });
  });

  it("resolves the URL from the registered multiaddr when no override is given", async () => {
    const choice = await discoverAcceptingProvider(fakeApi({ matches: [match(1, 3333)] }), want);
    expect(choice).toEqual({ address: toSs58(accountId(1)), url: "http://127.0.0.1:3333" });
  });

  it("skips a full match without a resolvable HTTP endpoint", async () => {
    const noHttp = { ...match(1, 1), info: { multiaddr: utf8("/dns4/example/udp/1") } };
    const choice = await discoverAcceptingProvider(fakeApi({ matches: [noHttp, match(2, 2)] }), want);
    expect(choice.address).toBe(toSs58(accountId(2)));
  });

  it("throws naming the requested size and duration when none qualifies", async () => {
    await expect(
      discoverAcceptingProvider(fakeApi({ matches: [match(1, 1, "DurationMismatch")] }), want),
    ).rejects.toThrow(/1024 bytes for 100 blocks/);
  });
});
