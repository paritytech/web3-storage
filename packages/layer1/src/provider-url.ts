// SPDX-License-Identifier: GPL-3.0-only

/**
 * Provider endpoint resolution shared by the layer-1 clients: on-chain
 * multiaddr lookup with a per-bucket cache, plus the watchValue-based wait
 * for a provider to accept a freshly created bucket's agreement.
 */

import {
  negotiateTerms,
  type HttpFetchOpts,
  type NegotiateRequest,
  type SignedTerms,
} from "@web3-storage/core";
import {
  parseMultiaddrToUrl,
  resolveProviderEndpoint,
  waitForPrimaryProvider,
  type ParachainApi,
  type WaitOpts,
} from "@web3-storage/layer0";

export type NegotiateProviderResult =
  | { ok: true; url: string; signed: SignedTerms }
  | { ok: false; error: string };

/**
 * A primary provider backing a bucket/drive's underlying layer-0 storage.
 * Shared by the S3 and file-system interfaces (re-exported from both as
 * `PrimaryProviderInfo`).
 */
export interface PrimaryProviderInfo {
  account: string;
  multiaddr: string;
  /** Resolved HTTP(S) base URL, or `null` if the multiaddr isn't an HTTP endpoint. */
  url: string | null;
}

/**
 * Resolve the primary-provider info for each given layer-0 bucket id, batching
 * every storage read into a single query per pallet map (no per-bucket /
 * per-provider round-trips). Returns an array aligned with `bucketIds` — each
 * entry is that bucket's primary providers (possibly empty).
 */
export async function resolveBucketProviders(
  api: ParachainApi,
  bucketIds: bigint[],
  readOpts: { at: "best" | "finalized" } = { at: "finalized" },
): Promise<PrimaryProviderInfo[][]> {
  if (bucketIds.length === 0) return [];
  const bucketInfo = await api.query.StorageProvider.Buckets.getValues(
    bucketIds.map((id) => [id] as const),
    readOpts,
  );

  const providerAccounts = [
    ...new Set(bucketInfo.flatMap((info) => info?.primary_providers ?? [])),
  ];
  const providerRecords = await api.query.StorageProvider.Providers.getValues(
    providerAccounts.map((account) => [account] as const),
    readOpts,
  );

  const providerMap = new Map<string, PrimaryProviderInfo>();
  providerAccounts.forEach((account, i) => {
    const record = providerRecords[i];
    if (!record) return;
    const multiaddr = new TextDecoder().decode(record.multiaddr);
    providerMap.set(account, { account, multiaddr, url: parseMultiaddrToUrl(multiaddr) });
  });

  return bucketInfo.map((info) =>
    (info?.primary_providers ?? [])
      .map((account) => providerMap.get(account))
      .filter((p): p is PrimaryProviderInfo => p != null),
  );
}

/**
 * Resolve a provider's multiaddr to a base URL and negotiate signed terms in
 * one step — the shared core of the create-bucket / create-drive flows. Returns
 * a discriminated result instead of throwing so UI callers can surface the
 * message inline (a null multiaddr and a failed /negotiate both yield
 * `{ ok: false, error }`).
 */
export async function negotiateProviderTerms(
  provider: { account: string; multiaddr: string },
  request: NegotiateRequest,
): Promise<NegotiateProviderResult> {
  const url = parseMultiaddrToUrl(provider.multiaddr);
  if (!url) {
    return {
      ok: false,
      error: `Provider ${provider.account} has an unparseable multiaddr: ${provider.multiaddr}`,
    };
  }
  try {
    const signed = await negotiateTerms(url, request);
    return { ok: true, url, signed };
  } catch (err) {
    return {
      ok: false,
      error: err instanceof Error ? err.message : "Failed to negotiate with provider",
    };
  }
}

export class ProviderUrlResolver {
  private cache = new Map<string, string>();

  constructor(
    private readonly api: ParachainApi,
    /** Explicit override (dev/tests) — skips on-chain resolution entirely. */
    private readonly override?: string,
    /** Read view for plain lookups. Finalized = UI-grade default. */
    private readonly readOpts: { at: "best" | "finalized" } = { at: "finalized" },
  ) {}

  async get(bucketId: bigint): Promise<string> {
    if (this.override) return this.override;
    const key = bucketId.toString();
    const cached = this.cache.get(key);
    if (cached) return cached;
    const url = await resolveProviderEndpoint(this.api, bucketId, this.readOpts);
    this.cache.set(key, url);
    return url;
  }

  invalidate(bucketId?: bigint): void {
    if (bucketId !== undefined) this.cache.delete(bucketId.toString());
    else this.cache.clear();
  }

  /**
   * Wait until `bucketId` has a primary provider, then resolve and cache its
   * HTTP endpoint. `opts.onTick` drives caller-side progress text.
   */
  async waitForProvider(bucketId: bigint, opts: WaitOpts = {}): Promise<string> {
    if (this.override) return this.override;
    this.invalidate(bucketId);
    await waitForPrimaryProvider(this.api, bucketId, { timeoutMs: 150_000, ...opts });
    // The acceptance was just observed at the best head — resolve there too,
    // or a finalized read could lag the very event that unblocked us.
    const url = await resolveProviderEndpoint(this.api, bucketId, { at: "best" });
    this.cache.set(bucketId.toString(), url);
    return url;
  }
}

/** A provider to negotiate with: chain account + its HTTP(S) base URL. */
export interface ProviderChoice {
  address: string;
  /** /negotiate base URL. Omitted means "resolve from the registered multiaddr". */
  url?: string;
}

/**
 * Pick an on-chain provider that is accepting primary agreements and exposes a
 * resolvable HTTP endpoint. `urlOverride` (a dev/test provider URL) wins over
 * the registered multiaddr — the local dev provider registers as the accepting
 * provider, and all UIs/examples point at one URL. Throws when none qualifies.
 */
export async function discoverAcceptingProvider(
  api: ParachainApi,
  {
    readOpts = { at: "finalized" },
    urlOverride,
  }: { readOpts?: { at: "best" | "finalized" }; urlOverride?: string } = {},
): Promise<{ address: string; url: string }> {
  const entries = await api.query.StorageProvider.Providers.getEntries(readOpts);
  for (const entry of entries) {
    const address = entry.keyArgs[0];
    const provider = entry.value;
    if (!provider?.settings?.accepting_primary) continue;
    const url =
      urlOverride ?? parseMultiaddrToUrl(new TextDecoder().decode(provider.multiaddr));
    if (url) return { address, url };
  }
  throw new Error(
    "no provider is accepting primary agreements (register one, or pass an explicit provider)",
  );
}

export interface ResolveCreationTermsOpts {
  owner: string;
  maxBytes: bigint;
  duration: number;
  /** Explicit provider; auto-discovered when omitted. */
  provider?: ProviderChoice;
  /** Pre-negotiated terms (skips /negotiate); requires `provider.address`. */
  signedTerms?: SignedTerms;
  /** Dev/test provider URL — used for discovery and address-only providers. */
  urlOverride?: string;
  /**
   * Price the request quotes. The provider's /negotiate requires this field
   * and validates it against its listed price; defaults to the chosen
   * provider's on-chain `price_per_byte`.
   */
  pricePerByte?: bigint;
  readOpts?: { at: "best" | "finalized" };
  fetchOpts?: HttpFetchOpts;
}

/**
 * The negotiate half of the negotiate -> establish flow for bucket/drive
 * creation: pick a provider (explicit or discovered), POST /negotiate, and
 * return the provider account + provider-signed terms ready for the establish_*
 * / create_drive / create_s3_bucket extrinsics. Pass `signedTerms` to skip the
 * HTTP round-trip entirely (the caller pre-negotiated).
 */
export async function resolveCreationTerms(
  api: ParachainApi,
  opts: ResolveCreationTermsOpts,
): Promise<{ provider: { address: string }; signedTerms: SignedTerms }> {
  if (opts.signedTerms) {
    const address = opts.provider?.address;
    if (!address) {
      throw new Error("signedTerms requires provider.address (who signed the terms)");
    }
    return { provider: { address }, signedTerms: opts.signedTerms };
  }

  let choice: { address: string; url: string };
  if (opts.provider?.address) {
    const url = opts.provider.url ?? opts.urlOverride;
    if (url) {
      choice = { address: opts.provider.address, url };
    } else {
      // Address only: resolve its /negotiate URL from the registered multiaddr.
      const info = await api.query.StorageProvider.Providers.getValue(
        opts.provider.address,
        opts.readOpts ?? { at: "finalized" },
      );
      const resolved = info ? parseMultiaddrToUrl(new TextDecoder().decode(info.multiaddr)) : null;
      if (!resolved) {
        throw new Error(`provider ${opts.provider.address} has no resolvable HTTP endpoint`);
      }
      choice = { address: opts.provider.address, url: resolved };
    }
  } else {
    choice = await discoverAcceptingProvider(api, {
      readOpts: opts.readOpts,
      urlOverride: opts.urlOverride,
    });
  }

  // /negotiate requires price_per_byte and validates it against the provider's
  // listed price; default to the provider's current on-chain setting.
  let pricePerByte = opts.pricePerByte;
  if (pricePerByte == null) {
    const info = await api.query.StorageProvider.Providers.getValue(
      choice.address,
      opts.readOpts ?? { at: "finalized" },
    );
    pricePerByte = info?.settings?.price_per_byte ?? 1n;
  }

  const request: NegotiateRequest = {
    owner: opts.owner,
    max_bytes: opts.maxBytes,
    duration: opts.duration,
    price_per_byte: pricePerByte,
    bucket_id: null,
    replica_params: null,
  };
  const signedTerms = await negotiateTerms(choice.url, request, opts.fetchOpts);
  return { provider: { address: choice.address }, signedTerms };
}
