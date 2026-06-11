/**
 * Provider endpoint resolution shared by the layer-1 clients: on-chain
 * multiaddr lookup with a per-bucket cache, plus the watchValue-based wait
 * for a provider to accept a freshly created bucket's agreement.
 */

import {
  resolveProviderEndpoint,
  waitForPrimaryProvider,
  type ParachainApi,
  type WaitOpts,
} from "@web3-storage/layer0";

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
