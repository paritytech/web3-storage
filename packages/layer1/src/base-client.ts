// SPDX-License-Identifier: Apache-2.0

/**
 * Shared plumbing for the layer-1 clients (FileSystemClient, S3Client):
 * signer state, provider-URL resolution, fetch injection, and the chain
 * read/submit defaults. The subclasses add only their pallet + HTTP surface.
 */

import { signProviderRequest, type HttpFetchOpts } from "@web3-storage/core";
import type {
  ChainSigner,
  ParachainApi,
  SubmitOpts,
  TxStatusListener,
} from "@web3-storage/layer0";

import { ProviderUrlResolver } from "./provider-url.js";

export interface Layer1ClientOptions {
  api: ParachainApi;
  signer?: ChainSigner | null;
  /** Explicit provider URL (dev/tests) — skips on-chain resolution. */
  providerUrl?: string;
  /** Injection point for unit tests. */
  fetch?: typeof fetch;
  /** Tx progress listener. Default null (silent) — apps drive their own UI. */
  onStatus?: TxStatusListener | null;
  /**
   * Read view for chain lookups. Defaults to "finalized" (UI-grade,
   * reorg-safe). Tests/examples pass READ_OPTS ({at: "best"}) to match their
   * in-block submission semantics.
   */
  readOpts?: { at: "best" | "finalized" };
  /**
   * Submission doneness. Defaults to "finalized" (UI-grade). Tests/examples
   * pass "best" for speed.
   */
  submitMode?: "best" | "finalized";
}

export abstract class Layer1Client {
  protected readonly api: ParachainApi;
  private signer: ChainSigner | null;
  protected readonly providers: ProviderUrlResolver;
  protected readonly fetchOpts: HttpFetchOpts;
  protected readonly onStatus: TxStatusListener | null;
  protected readonly readOpts: { at: "best" | "finalized" };
  protected readonly submitMode: "best" | "finalized";
  protected readonly creationUrlOverride?: string;

  constructor(opts: Layer1ClientOptions) {
    this.api = opts.api;
    this.signer = opts.signer ?? null;
    this.readOpts = opts.readOpts ?? { at: "finalized" };
    this.submitMode = opts.submitMode ?? "finalized";
    this.providers = new ProviderUrlResolver(opts.api, opts.providerUrl, this.readOpts);
    this.creationUrlOverride = opts.providerUrl;
    this.fetchOpts = opts.fetch ? { fetchImpl: opts.fetch } : {};
    this.onStatus = opts.onStatus ?? null;
  }

  setSigner(signer: ChainSigner | null): void {
    this.signer = signer;
  }

  protected requireSigner(): ChainSigner {
    if (!this.signer) throw new Error("Signer not set");
    return this.signer;
  }

  protected submitOpts(): SubmitOpts {
    return { mode: this.submitMode, retryStale: 0, onStatus: this.onStatus };
  }

  protected authHeaders(method: string, bucketId: bigint): Promise<Record<string, string>> {
    return signProviderRequest(this.requireSigner().signer, method, bucketId);
  }
}
