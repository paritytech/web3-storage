// SPDX-License-Identifier: Apache-2.0

/**
 * Web3Storage: one handle over a chain connection. It creates buckets and
 * manages their members on chain, finds each bucket's provider, and opens a
 * bucket as a file-system tree (`drive`) or as S3-style objects (`objects`).
 * Everything here can also be done with the layer0 and file-system functions
 * directly.
 */

import type { PolkadotClient } from "polkadot-api";
import type { SignedTerms } from "@web3-storage/core";
import {
  connect,
  createBucketWithPrimary,
  getBucketInfos,
  listMemberBuckets,
  ProviderUrlResolver,
  removeMember,
  resolveCreationTerms,
  setMember,
  syncSs58Prefix,
  type BucketInfo,
  type ChainSigner,
  type MemberRole,
  type ParachainApi,
  type ProviderChoice,
  type SubmitOpts,
  type TxStatusListener,
  type Visibility,
  type WaitOpts,
} from "@web3-storage/layer0";
import { FsTree, Objects, providerBlobStore } from "@web3-storage/bucket-tree";

export interface Web3StorageOptions {
  api: ParachainApi;
  signer?: ChainSigner | null;
  /** Provider URL for every bucket instead of the on-chain endpoint (dev, tests). */
  providerUrl?: string;
  /** `fetch` for provider requests (tests). */
  fetch?: typeof fetch;
  /** Tx progress listener. Default null (silent): apps drive their own UI. */
  onStatus?: TxStatusListener | null;
  /** Chain read view. Default "finalized"; tests pass "best". */
  readOpts?: { at: "best" | "finalized" };
  /** When a submitted transaction counts as done. Default "finalized"; tests pass "best". */
  submitMode?: "best" | "finalized";
}

/** Options of {@link Web3Storage.createBucket}. */
export interface CreateBucketOptions {
  /** Bytes the agreement covers: the negotiated terms' `max_bytes`. */
  maxCapacity: bigint;
  /** Agreement duration in blocks: the negotiated terms' `duration`. */
  duration: number;
  /** Provider to negotiate with; the first accepting provider when omitted. */
  provider?: ProviderChoice;
  /** Terms already negotiated: skips `/negotiate`. Requires `provider.address`. */
  signedTerms?: SignedTerms;
  /** Read visibility of the bucket (default Private). */
  visibility?: Visibility;
}

export class Web3Storage {
  readonly api: ParachainApi;
  private currentSigner: ChainSigner | null;
  private readonly providers: ProviderUrlResolver;
  private readonly providerUrl?: string;
  private readonly fetchImpl?: typeof fetch;
  private readonly onStatus: TxStatusListener | null;
  private readonly readOpts: { at: "best" | "finalized" };
  private readonly submitMode: "best" | "finalized";
  private papi?: PolkadotClient;

  constructor(opts: Web3StorageOptions) {
    this.api = opts.api;
    this.currentSigner = opts.signer ?? null;
    this.providerUrl = opts.providerUrl;
    this.fetchImpl = opts.fetch;
    this.onStatus = opts.onStatus ?? null;
    this.readOpts = opts.readOpts ?? { at: "finalized" };
    this.submitMode = opts.submitMode ?? "finalized";
    this.providers = new ProviderUrlResolver(opts.api, opts.providerUrl, this.readOpts);
  }

  /**
   * Connect to `chainWs` and adopt the runtime's SS58 prefix for address
   * rendering (`syncSs58: false` skips it, e.g. before the chain produces
   * blocks). {@link disconnect} closes the connection.
   */
  static async connect(
    chainWs: string,
    opts: Omit<Web3StorageOptions, "api"> & { syncSs58?: boolean } = {},
  ): Promise<Web3Storage> {
    const { papi, api } = connect(chainWs);
    if (opts.syncSs58 !== false) await syncSs58Prefix(api);
    const w3s = new Web3Storage({ ...opts, api });
    w3s.papi = papi;
    return w3s;
  }

  get signer(): ChainSigner | null {
    return this.currentSigner;
  }

  setSigner(signer: ChainSigner | null): void {
    this.currentSigner = signer;
  }

  // ── Buckets (chain) ─────────────────────────────────────────────────────

  /**
   * Create a bucket with one primary agreement: pick a provider
   * (`opts.provider` or the first accepting one), POST `/negotiate` for signed
   * terms unless `opts.signedTerms` is given, then submit
   * `create_bucket_with_primary`. The signer becomes the bucket's admin.
   */
  async createBucket(opts: CreateBucketOptions): Promise<{ bucketId: bigint; provider: string }> {
    const signer = this.requireSigner();
    const { provider, signedTerms } = await resolveCreationTerms(this.api, {
      owner: signer.address,
      maxBytes: opts.maxCapacity,
      duration: opts.duration,
      provider: opts.provider,
      signedTerms: opts.signedTerms,
      urlOverride: this.providerUrl,
      readOpts: this.readOpts,
      fetchOpts: this.fetchImpl ? { fetchImpl: this.fetchImpl } : {},
    });
    const { bucketId } = await createBucketWithPrimary(this.api, signer, provider, signedTerms, {
      ...this.submitOpts(),
      visibility: opts.visibility,
    });
    return { bucketId, provider: provider.address };
  }

  async getBucket(bucketId: bigint): Promise<BucketInfo | null> {
    const [info] = await getBucketInfos(this.api, [bucketId], this.readOpts);
    return info ?? null;
  }

  /**
   * Every bucket `account` (default: the signer) is a member of. The chain
   * does not record which buckets contain a file system or objects, so this
   * lists all of them.
   */
  listBuckets(account?: string): Promise<BucketInfo[]> {
    return listMemberBuckets(this.api, account ?? this.requireSigner().address, this.readOpts);
  }

  /** Add `account` to the bucket with `role`, or change its role. Needs Admin. */
  async setMember(bucketId: bigint, account: string, role: MemberRole): Promise<void> {
    await setMember(this.api, this.requireSigner(), bucketId, { address: account }, role, this.submitOpts());
  }

  /** Remove `account` from the bucket. Needs Admin. */
  async removeMember(bucketId: bigint, account: string): Promise<void> {
    await removeMember(this.api, this.requireSigner(), bucketId, { address: account }, this.submitOpts());
  }

  // ── Providers ───────────────────────────────────────────────────────────

  /** HTTP URL of the bucket's primary provider. Cached per bucket. */
  getProviderUrl(bucketId: bigint): Promise<string> {
    return this.providers.get(bucketId);
  }

  invalidateProviderUrl(bucketId?: bigint): void {
    this.providers.invalidate(bucketId);
  }

  /** Wait until the bucket has a primary provider, then return its URL. */
  waitForProvider(bucketId: bigint, opts?: WaitOpts): Promise<string> {
    return this.providers.waitForProvider(bucketId, opts);
  }

  // ── Data (provider) ─────────────────────────────────────────────────────

  /**
   * The bucket's file-system tree on its primary provider. The tree uses the
   * signer set when it was opened: writes need one with Writer or Admin, and
   * reads of a private bucket need Reader. Rejects if the bucket is not on
   * chain or has no primary provider.
   */
  async drive(bucketId: bigint, opts: { signal?: AbortSignal } = {}): Promise<FsTree> {
    const providerUrl = await this.providers.get(bucketId);
    return new FsTree(
      providerBlobStore(providerUrl, bucketId, this.currentSigner, { fetch: this.fetchImpl, signal: opts.signal }),
    );
  }

  /** The bucket's S3-style objects, stored in its file-system tree. */
  async objects(bucketId: bigint, opts: { signal?: AbortSignal } = {}): Promise<Objects> {
    return new Objects(await this.drive(bucketId, opts));
  }

  /** Close the chain connection opened by {@link connect}. */
  disconnect(): void {
    this.papi?.destroy();
  }

  private requireSigner(): ChainSigner {
    if (!this.currentSigner) throw new Error("Signer not set");
    return this.currentSigner;
  }

  private submitOpts(): SubmitOpts {
    return { mode: this.submitMode, retryStale: 0, onStatus: this.onStatus };
  }
}
