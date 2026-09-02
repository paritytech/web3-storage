// SPDX-License-Identifier: GPL-3.0-only

/**
 * S3 Client — s3-ui's thin adapter over the SDK's S3Client + layer0 pallet
 * wrappers, mirroring drive-ui's DriveClient. Everything chain/HTTP-mechanical
 * (bucket/object ops, tx submission, provider resolution + caching, retry,
 * request signing, the negotiate→establish flow, MultiSignature encoding)
 * lives in @web3-storage/sdk. What stays here is app-shaped: the mutable
 * api/signer lifecycle the RxJS state layer drives, the UI view types, and the
 * reads/subscriptions the SDK's S3Client doesn't model (provider discovery via
 * the `find_matching_providers` runtime API, bucket members, checkpoint/
 * challenge state).
 */

import { Subscription } from "rxjs";
import type { PolkadotSigner } from "polkadot-api";
import { parachain } from "@polkadot-api/descriptors";
import { getSs58AddressInfo } from "@polkadot-api/substrate-bindings";
import {
  buildSignedTermsArgs,
  httpFetch,
  negotiateTerms,
  parseMultiaddrToUrl,
  removeMember as removeMemberTx,
  requireOneEvent,
  setMember as setMemberTx,
  submitTx,
  toSs58,
  type BucketUsage,
  type ChainSigner,
  type Keypair,
  type NegotiateRequest,
  type SignedTerms,
} from "@web3-storage/sdk";
import { S3Client as SdkS3Client } from "@web3-storage/sdk/s3";
import type { PrimaryProviderInfo } from "@web3-storage/sdk/s3";
import type { ParachainApi } from "@/state/chain.state";

export type Signer = PolkadotSigner;

// Re-export the SDK negotiate primitives + types the create-bucket components
// (NewBucketDialog, ProviderPickerPanel) and the state layer import from here.
export { buildSignedTermsArgs, negotiateTerms };
export type { BucketUsage, NegotiateRequest, SignedTerms };

/** `parseMultiaddrToHttp` is the SDK's `parseMultiaddrToUrl` under the old name. */
export const parseMultiaddrToHttp = parseMultiaddrToUrl;

/** SS58 address validity — not in the SDK; a thin substrate-bindings wrapper. */
export function isValidSs58(address: string): boolean {
  return getSs58AddressInfo(address).isValid;
}

// ── View types (UI-shaped; the SDK's S3Client returns its own structs that we
// map onto these so components keep a stable surface) ───────────────────────────

export interface BucketInfo {
  s3BucketId: bigint;
  name: string;
  layer0BucketId: bigint;
  owner: string;
  createdAt: bigint;
  /** Primary providers of the underlying layer-0 bucket. */
  providerInfo: PrimaryProviderInfo[];
}

export interface S3ObjectInfo {
  key: string;
  size: number;
  lastModified: number;
  etag: string;
}

export interface UploadResult {
  cid: string;
  size: number;
}

export interface AvailableProvider {
  account: string;
  multiaddr: string;
  stake: bigint;
  availableCapacity: bigint;
  maxCapacity: bigint;
  pricePerByte: bigint;
  minDuration: number;
  maxDuration: number;
  acceptingPrimary: boolean;
  agreementsTotal: number;
}

export interface MatchingProviders extends AvailableProvider {
  matchScore: number;
  partialReason: string;
  committedBytes: bigint;
  replicaSyncPrice?: bigint;
  acceptingExtensions: boolean;
  registeredAt: number;
  agreementsExtended: number;
  agreementsNotExtended: number;
  agreementsBurned: number;
  challengesReceived: number;
  challengesFailed: number;
}

export type MemberRole = "Admin" | "Writer" | "Reader";

export interface BucketMember {
  account: string;
  role: MemberRole;
}

export interface CheckpointInfo {
  mmrRoot: string;
  startSeq: bigint;
  leafCount: bigint;
  checkpointBlock: number;
}

export interface ChallengeResult {
  challengeId: { deadline: number; index: number };
  respondBy: number;
}

export interface OpenChallenge {
  deadline: number;
  index: number;
  bucketId: bigint;
  provider: string;
  challenger: string;
  leafIndex: bigint;
  chunkIndex: bigint;
  deposit: bigint;
}

export interface ChallengeDefenseResult {
  challengeId: { deadline: number; index: number };
  provider: string;
  responseTimeBlocks: number;
  challengerCost: bigint;
  providerCost: bigint;
  blockNumber: number;
  blockHash: string;
}

export interface ChallengeSlashResult {
  challengeId: { deadline: number; index: number };
  provider: string;
  slashedAmount: bigint;
  challengerReward: bigint;
  blockNumber: number;
  blockHash: string;
}

export interface QueryMatchingProvidersParams {
  query: {
    bytesNeeded: bigint;
    minDuration: number;
    maxPricePerByte: bigint;
    primaryOnly: boolean;
  };
  limit: number;
}

// ── S3Client class ────────────────────────────────────────────────────────────

export class S3Client {
  private api: ParachainApi | null = null;
  private signer: Signer | null = null;
  private signerAddress: string | null = null;
  private keypair: Keypair | null = null;
  private owner: ChainSigner | null = null;
  private s3: SdkS3Client | null = null;

  private rebuild(): void {
    if (!this.api) {
      this.s3 = null;
      this.owner = null;
      return;
    }
    let chainSigner: ChainSigner | null = null;
    if (this.signer && this.signerAddress) {
      chainSigner = {
        signer: this.signer,
        address: this.signerAddress,
        publicKey: this.signer.publicKey,
        keypair: this.keypair ?? undefined,
      };
    }
    this.owner = chainSigner;
    this.s3 = new SdkS3Client({ api: this.api, signer: chainSigner });
  }

  setApi(api: ParachainApi | null): void {
    if (api !== this.api) {
      this.api = api;
      this.rebuild();
    }
  }

  setSigner(
    signer: Signer | null,
    address: string | null,
    keypair: Keypair | null,
  ): void {
    this.signer = signer;
    this.signerAddress = address;
    this.keypair = keypair;
    this.rebuild();
  }

  hasApi(): boolean {
    return this.api !== null;
  }

  hasSigner(): boolean {
    return this.signer !== null && this.signerAddress !== null;
  }

  getSignerAddress(): string | null {
    return this.signerAddress;
  }

  private requireApi(): ParachainApi {
    if (!this.api) throw new Error("Not connected to chain");
    return this.api;
  }

  private requireS3(): SdkS3Client {
    if (!this.s3) throw new Error("Not connected to chain");
    return this.s3;
  }

  private requireOwner(): ChainSigner {
    if (!this.owner) throw new Error("Signer not set");
    return this.owner;
  }

  // ── Provider resolution (delegated to the SDK's resolver + cache) ───────────

  getProviderUrl(bucketId: bigint): Promise<string> {
    return this.requireS3().getProviderUrl(bucketId);
  }

  invalidateProviderUrl(bucketId: bigint): void {
    this.s3?.invalidateProviderUrl(bucketId);
  }

  // ── S3 Bucket operations (on-chain, via the SDK's S3Client) ─────────────────

  /**
   * Redeem provider-signed terms in `create_s3_bucket` (negotiate→establish).
   * The terms are pre-negotiated by the create-bucket UI; `providerUrl` primes
   * nothing here — the SDK resolves the provider from chain on first use.
   */
  async createBucket(
    name: string,
    providerAccount: string,
    providerUrl: string,
    signed: SignedTerms,
  ): Promise<BucketInfo> {
    const info = await this.requireS3().createBucket(name, {
      maxCapacity: BigInt(signed.terms.max_bytes),
      duration: signed.terms.duration,
      provider: { address: providerAccount, url: providerUrl },
      signedTerms: signed,
    });
    return {
      s3BucketId: info.s3BucketId,
      name: info.name,
      layer0BucketId: info.layer0BucketId,
      owner: info.owner,
      createdAt: BigInt(info.createdAt),
      providerInfo: [{ account: providerAccount, multiaddr: "", url: providerUrl }],
    };
  }

  async listBuckets(): Promise<BucketInfo[]> {
    const list = await this.requireS3().listBuckets();
    return list.map((b) => ({
      s3BucketId: b.s3BucketId,
      name: b.name,
      layer0BucketId: b.layer0BucketId,
      owner: b.owner,
      createdAt: BigInt(b.createdAt),
      providerInfo: b.providerInfo,
    }));
  }

  /**
   * Empty the bucket, then delete it. The chain refuses deleting a bucket
   * whose `object_count` is non-zero, so every on-chain metadata row is
   * cleared first (the authoritative set — the provider's index entries are
   * removed best-effort; the Layer 0 teardown erases provider data anyway).
   * Resolves with the prorated refund for the remaining paid period
   * (payment buys bytes for a number of blocks; teardown refunds the
   * blocks not yet elapsed).
   */
  // Sequential one-transaction-per-object metadata deletes — fine for the
  // bucket sizes the UI handles; switch to utility.batch if bulk deletion
  // of large buckets ever matters.
  async deleteBucket(s3BucketId: bigint): Promise<{ refunded: bigint }> {
    const api = this.requireApi();
    const s3 = this.requireS3();

    const info = await api.query.S3Registry.S3Buckets.getValue(s3BucketId);
    const entries = await api.query.S3Registry.Objects.getEntries(s3BucketId);
    for (const entry of entries) {
      const key = new TextDecoder().decode(entry.keyArgs[1]);
      if (info) {
        try {
          await s3.deleteObject({ layer0BucketId: info.layer0_bucket_id }, key);
        } catch {
          // Provider unreachable or index already gone — only the chain
          // rows block the deletion, and the L0 teardown reclaims provider
          // data anyway.
        }
      }
      await s3.deleteObjectMetadata(s3BucketId, key);
    }

    return this.requireS3().deleteBucket(s3BucketId);
  }

  /** Physical usage vs paid quota, read from the bucket's provider node. */
  getBucketUsage(layer0BucketId: bigint): Promise<BucketUsage> {
    return this.requireS3().getBucketUsage(layer0BucketId);
  }

  // ── S3 Object operations (HTTP, via the SDK's S3Client) ─────────────────────

  async putObject(
    bucketId: bigint,
    key: string,
    data: Uint8Array,
    options?: { signal?: AbortSignal },
  ): Promise<UploadResult> {
    const result = await this.requireS3().putObject({ layer0BucketId: bucketId }, key, data, {
      signal: options?.signal,
    });
    return { cid: result.cid ?? "", size: result.size };
  }

  async getObject(bucketId: bigint, key: string): Promise<Uint8Array> {
    const { data } = await this.requireS3().getObject({ layer0BucketId: bucketId }, key);
    return data;
  }

  async listObjects(bucketId: bigint, prefix?: string): Promise<S3ObjectInfo[]> {
    const summaries = await this.requireS3().listObjects({ layer0BucketId: bucketId }, prefix);
    return summaries.map((o) => ({
      key: o.key,
      size: o.size,
      lastModified: o.lastModified ?? 0,
      etag: o.etag ?? "",
    }));
  }

  async deleteObject(bucketId: bigint, key: string): Promise<void> {
    await this.requireS3().deleteObject({ layer0BucketId: bucketId }, key);
  }

  // ── Members (read is a chain query; writes go through layer0 wrappers) ───────

  async getBucketMembers(bucketId: bigint): Promise<BucketMember[]> {
    const api = this.requireApi();
    const bucket = await api.query.StorageProvider.Buckets.getValue(bucketId);
    if (!bucket) throw new Error(`Bucket ${bucketId} not found`);

    const roleMap: Record<string, MemberRole> = {
      Admin: "Admin",
      Writer: "Writer",
      Reader: "Reader",
    };

    return (bucket.members ?? []).map((m: { account: string; role: { type?: string } | string }) => {
      const roleType = typeof m.role === "string" ? m.role : m.role?.type ?? "Reader";
      return { account: m.account, role: roleMap[roleType] ?? "Reader" };
    });
  }

  async addMember(bucketId: bigint, account: string, role: MemberRole): Promise<void> {
    await setMemberTx(this.requireApi(), this.requireOwner(), bucketId, { address: account }, role);
  }

  async removeMember(bucketId: bigint, account: string): Promise<void> {
    await removeMemberTx(this.requireApi(), this.requireOwner(), bucketId, { address: account });
  }

  // ── Provider discovery (UI-side; raw `StorageProvider.Providers` + the
  // `find_matching_providers` runtime API — not modelled by the SDK) ──────────

  async listAvailableProviders(): Promise<AvailableProvider[]> {
    const api = this.requireApi();
    const entries = await api.query.StorageProvider.Providers.getEntries();
    const providers: AvailableProvider[] = [];

    for (const entry of entries) {
      const provider = entry.value;
      const account = entry.keyArgs[0] as string;
      const settings = provider.settings;

      const multiaddrStr = new TextDecoder().decode(provider.multiaddr);
      const maxCapacity = BigInt(settings.max_capacity ?? 0);
      const committedBytes = BigInt(provider.committed_bytes ?? 0);
      const availableCapacity =
        maxCapacity > committedBytes ? maxCapacity - committedBytes : 0n;

      providers.push({
        account,
        multiaddr: multiaddrStr,
        stake: BigInt(provider.stake ?? 0),
        availableCapacity,
        maxCapacity,
        pricePerByte: BigInt(settings.price_per_byte ?? 0),
        minDuration: settings.min_duration ?? 0,
        maxDuration: settings.max_duration ?? 0,
        acceptingPrimary: settings.accepting_primary ?? false,
        // eslint-disable-next-line @typescript-eslint/no-explicit-any
        agreementsTotal: (provider.stats as any)?.agreements_total ?? 0,
      });
    }

    providers.sort((a, b) => {
      if (b.availableCapacity > a.availableCapacity) return 1;
      if (b.availableCapacity < a.availableCapacity) return -1;
      return 0;
    });

    return providers;
  }

  async queryMatchingProviders(
    query: QueryMatchingProvidersParams["query"],
    limit: QueryMatchingProvidersParams["limit"],
  ): Promise<MatchingProviders[]> {
    const api = this.requireApi();
    const matches = await api.apis.StorageProviderApi.find_matching_providers(
      {
        bytes_needed: query.bytesNeeded,
        min_duration: query.minDuration,
        max_price_per_byte: query.maxPricePerByte,
        primary_only: query.primaryOnly,
      },
      limit,
    );

    return matches
      .map((match) => {
        const info = match.info;
        const maxCapacity = BigInt(info.max_capacity ?? 0);
        const committedBytes = BigInt(info.committed_bytes ?? 0);
        const availableCapacity =
          maxCapacity > committedBytes ? maxCapacity - committedBytes : 0n;

        return {
          account: toSs58(match.account),
          multiaddr: new TextDecoder().decode(info.multiaddr),
          stake: BigInt(info.stake ?? 0),
          availableCapacity,
          maxCapacity,
          committedBytes,
          pricePerByte: BigInt(info.price_per_byte ?? 0),
          minDuration: info.min_duration ?? 0,
          maxDuration: info.max_duration ?? 0,
          acceptingPrimary: info.accepting_primary ?? false,
          replicaSyncPrice:
            info.replica_sync_price != null ? BigInt(info.replica_sync_price) : undefined,
          acceptingExtensions: info.accepting_extensions ?? false,
          registeredAt: Number(info.registered_at ?? 0),
          agreementsTotal: info.agreements_total ?? 0,
          agreementsExtended: info.agreements_extended ?? 0,
          agreementsNotExtended: info.agreements_not_extended ?? 0,
          agreementsBurned: info.agreements_burned ?? 0,
          challengesReceived: info.challenges_received ?? 0,
          challengesFailed: info.challenges_failed ?? 0,
          matchScore: match.match_score,
          partialReason: match.partial_reason?.type ?? "",
        };
      })
      .sort((a, b) => b.matchScore - a.matchScore);
  }

  // ── Checkpoint (chain-state read) ────────────────────────────────────────

  async getCheckpointInfo(bucketId: bigint): Promise<CheckpointInfo | null> {
    const api = this.requireApi();
    const bucket = await api.query.StorageProvider.Buckets.getValue(bucketId);
    if (!bucket) throw new Error(`Bucket ${bucketId} not found`);

    const snapshot = bucket.snapshot;
    if (!snapshot) return null;

    return {
      mmrRoot: snapshot.commitment.mmr_root,
      startSeq: snapshot.commitment.start_seq,
      leafCount: snapshot.commitment.leaf_count,
      checkpointBlock: snapshot.checkpoint_block,
    };
  }

  // ── Challenge (write via SDK submitTx; reads/subscriptions are chain queries) ─

  async getBucketProviders(bucketId: bigint): Promise<string[]> {
    const api = this.requireApi();
    const bucket = await api.query.StorageProvider.Buckets.getValue(bucketId);
    if (!bucket) throw new Error(`Bucket ${bucketId} not found`);
    const providers: string[] = bucket.primary_providers ?? [];
    if (providers.length === 0) {
      throw new Error(`Bucket ${bucketId} has no primary providers`);
    }
    return providers;
  }

  /**
   * Open a checkpoint challenge against a provider. The chain submission
   * (finalized — the challenge id must survive to the response) goes through
   * the SDK's `submitTx`; we extract `respond_by` ourselves because layer0's
   * `challengeCheckpoint` wrapper returns only the id and forces chunk 0.
   */
  async challengeCheckpoint(
    bucketId: bigint,
    provider: string,
    leafIndex: bigint,
    chunkIndex: bigint,
  ): Promise<ChallengeResult> {
    const api = this.requireApi();
    const result = await submitTx(
      api.tx.StorageProvider.challenge_checkpoint({
        bucket_id: bucketId,
        provider,
        target: { leaf_index: leafIndex, chunk_index: chunkIndex },
      }),
      this.requireOwner().signer,
      { label: "challenge_checkpoint", mode: "finalized" },
    );

    const created = requireOneEvent(
      result.events,
      api.event.StorageProvider.ChallengeCreated,
      "ChallengeCreated",
    );
    return {
      challengeId: { deadline: created.challenge_id.deadline, index: created.challenge_id.index },
      respondBy: created.respond_by,
    };
  }

  async getLeafChunkCount(bucketId: bigint, leafIndex: bigint): Promise<number> {
    const providerUrl = await this.getProviderUrl(bucketId);
    const params = new URLSearchParams({
      bucket_id: Number(bucketId).toString(),
      leaf_index: leafIndex.toString(),
    });
    const response = await httpFetch(`${providerUrl}/mmr_proof?${params.toString()}`);
    if (!response.ok) {
      throw new Error(`MMR proof request failed: ${response.status}`);
    }
    const result = await response.json();
    const dataSize: number = result.leaf?.data_size ?? 0;
    const chunkSize = 262144; // 256 KiB — DEFAULT_CHUNK_SIZE
    return dataSize === 0 ? 1 : Math.ceil(dataSize / chunkSize);
  }

  async isChallengeActive(deadline: number): Promise<boolean> {
    const api = this.requireApi();
    // Challenges is a StorageDoubleMap keyed by (deadline, index); an active
    // deadline has at least one entry under its prefix.
    const entries = await api.query.StorageProvider.Challenges.getEntries(deadline);
    return entries.length > 0;
  }

  async getOpenChallenges(bucketId: bigint): Promise<OpenChallenge[]> {
    const api = this.requireApi();
    // Challenges is a StorageDoubleMap keyed by (deadline, index): each entry
    // is a single challenge with keyArgs = [deadline, index] and value = the
    // Challenge (there is no per-deadline Vec to iterate).
    const entries = await api.query.StorageProvider.Challenges.getEntries();
    const result: OpenChallenge[] = [];

    for (const entry of entries) {
      const [deadline, index] = entry.keyArgs as [number, number];
      const c = entry.value;
      if (c.bucket_id === bucketId) {
        result.push({
          deadline,
          index,
          bucketId: c.bucket_id,
          provider: c.provider,
          challenger: c.challenger,
          leafIndex: c.target.leaf_index,
          chunkIndex: c.target.chunk_index,
          deposit: c.deposit,
        });
      }
    }

    return result.sort((a, b) => a.deadline - b.deadline);
  }

  watchChallengeOutcome(
    deadline: number,
    providerAddress: string,
    onDefended: (result: ChallengeDefenseResult) => void,
    onSlashed: (result: ChallengeSlashResult) => void,
  ): () => void {
    const api = this.requireApi();
    const sub = new Subscription();

    sub.add(
      api.event.StorageProvider.ChallengeDefended.watch().subscribe({
        next: ({ block, events }) => {
          for (const ev of events) {
            const p = ev.payload;
            if (p.challenge_id.deadline !== deadline) continue;
            if (p.provider !== providerAddress) continue;
            onDefended({
              challengeId: { deadline: p.challenge_id.deadline, index: p.challenge_id.index },
              provider: p.provider,
              responseTimeBlocks: p.response_time_blocks,
              challengerCost: p.challenger_cost,
              providerCost: p.provider_cost,
              blockNumber: block.number,
              blockHash: block.hash,
            });
          }
        },
        error: () => {},
      }),
    );

    sub.add(
      api.event.StorageProvider.ChallengeSlashed.watch().subscribe({
        next: ({ block, events }) => {
          for (const ev of events) {
            const p = ev.payload;
            if (p.challenge_id.deadline !== deadline) continue;
            if (p.provider !== providerAddress) continue;
            onSlashed({
              challengeId: { deadline: p.challenge_id.deadline, index: p.challenge_id.index },
              provider: p.provider,
              slashedAmount: p.slashed_amount,
              challengerReward: p.challenger_reward,
              blockNumber: block.number,
              blockHash: block.hash,
            });
          }
        },
        error: () => {},
      }),
    );

    return () => sub.unsubscribe();
  }
}

export { parachain };
