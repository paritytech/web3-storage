// SPDX-License-Identifier: GPL-3.0-only

/**
 * S3 Client — s3-ui's thin adapter over the SDK's Web3Storage + layer0 pallet
 * wrappers, mirroring drive-ui's DriveClient. Everything chain/HTTP-mechanical
 * (bucket/object ops, tx submission, provider resolution + caching, retry,
 * request signing, the negotiate→establish flow, MultiSignature encoding)
 * lives in @web3-storage/sdk. What stays here is app-shaped: the mutable
 * api/signer lifecycle the RxJS state layer drives, the UI view types, and the
 * reads/subscriptions the SDK does not model (provider discovery via the
 * `find_matching_providers` runtime API, checkpoint/challenge state).
 */

import { Subscription } from "rxjs";
import { parachain } from "@polkadot-api/descriptors";
import { getSs58AddressInfo } from "@polkadot-api/substrate-bindings";
import {
  buildSignedTermsArgs,
  httpFetch,
  leafChunkCount,
  negotiateTerms,
  parseMultiaddrToUrl,
  requireOneEvent,
  signProviderRequest,
  resolveExpiredChallenge as resolveExpiredChallengeTx,
  getBucketVisibility as getBucketVisibilityQuery,
  setBucketVisibility as setBucketVisibilityTx,
  submitTx,
  toSs58,
  type ChainSigner,
  type Keypair,
  type NegotiateRequest,
  type SignedTerms,
  type Visibility,
  Web3Storage,
  type BucketInfo,
  type BucketMember,
  type MemberRole,
} from "@web3-storage/sdk";
import type { ParachainApi } from "@/state/chain.state";

export type Signer = ChainSigner["signer"];

// Re-export the SDK negotiate primitives + types the create-bucket components
// (NewBucketDialog, ProviderPickerPanel) and the state layer import from here.
export { buildSignedTermsArgs, negotiateTerms };
export type { BucketInfo, BucketMember, MemberRole, NegotiateRequest, SignedTerms, Visibility };

/** `parseMultiaddrToHttp` is the SDK's `parseMultiaddrToUrl` under the old name. */
export const parseMultiaddrToHttp = parseMultiaddrToUrl;

/** SS58 address validity — not in the SDK; a thin substrate-bindings wrapper. */
export function isValidSs58(address: string): boolean {
  return getSs58AddressInfo(address).isValid;
}

// ── View types (UI-shaped; buckets use the SDK's `BucketInfo` as is) ─────────

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
  /** Free capacity per the chain; `undefined` = unlimited, not `0n` ("full"). */
  availableCapacity: bigint | undefined;
  maxCapacity: bigint;
  pricePerByte: bigint;
  minDuration: number;
  maxDuration: number;
  acceptingPrimary: boolean;
  /** Smallest agreement `max_bytes` the provider accepts; `0n` = no minimum. */
  minAgreementBytes: bigint;
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
  /** Challenges resolved in the provider's favor (authorized + public tiers). */
  challengesDefended: number;
  challengesFailed: number;
  /** 0-100, computed on-chain by `ProviderStats::reputation`. */
  reputation: number;
}

/**
 * `StorageProviderApi.providers` limit for the full list. One page for
 * everything: paging over the hash-ordered `Providers` map skips or repeats a
 * provider when a registration or deregistration occurs between two pages,
 * and this client has no block handle to pin the pages to. The storage scan
 * this replaced read the whole map in one call too.
 */
const ALL_PROVIDERS = 0xffff_ffff;

/** Most free capacity first; unlimited (`undefined`) sorts ahead of metered. */
function byFreeCapacityDesc(a: AvailableProvider, b: AvailableProvider): number {
  if (a.availableCapacity === b.availableCapacity) return 0;
  if (a.availableCapacity === undefined) return -1;
  if (b.availableCapacity === undefined) return 1;
  return b.availableCapacity > a.availableCapacity ? 1 : -1;
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
  private w3s: Web3Storage | null = null;

  private rebuild(): void {
    if (!this.api) {
      this.w3s = null;
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
    this.w3s = new Web3Storage({ api: this.api, signer: chainSigner });
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

  private requireW3s(): Web3Storage {
    if (!this.w3s) throw new Error("Not connected to chain");
    return this.w3s;
  }

  private requireOwner(): ChainSigner {
    if (!this.owner) throw new Error("Signer not set");
    return this.owner;
  }

  // ── Provider resolution (delegated to the SDK's resolver + cache) ───────────

  getProviderUrl(bucketId: bigint): Promise<string> {
    return this.requireW3s().getProviderUrl(bucketId);
  }

  invalidateProviderUrl(bucketId: bigint): void {
    this.w3s?.invalidateProviderUrl(bucketId);
  }

  // ── S3 Bucket operations (on-chain, via the SDK's Web3Storage) ─────────────

  /**
   * Redeem provider-signed terms in `create_bucket_with_primary`
   * (negotiate→establish). The terms are pre-negotiated by the create-bucket
   * UI. Returns the new Layer 0 bucket id.
   */
  async createBucket(
    providerAccount: string,
    providerUrl: string,
    signed: SignedTerms,
    visibility?: Visibility,
  ): Promise<bigint> {
    const { bucketId } = await this.requireW3s().createBucket({
      maxCapacity: BigInt(signed.terms.max_bytes),
      duration: signed.terms.duration,
      provider: { address: providerAccount, url: providerUrl },
      signedTerms: signed,
      visibility,
    });
    return bucketId;
  }

  /** Every bucket the signer is a member of (`StorageProvider.MemberBuckets`). */
  async listBuckets(): Promise<BucketInfo[]> {
    return this.requireW3s().listBuckets();
  }

  // ── S3 Object operations (the bucket's own tree, via the SDK's Objects) ────

  async putObject(
    bucketId: bigint,
    key: string,
    data: Uint8Array,
    options?: { signal?: AbortSignal },
  ): Promise<UploadResult> {
    const objects = await this.requireW3s().objects(bucketId, { signal: options?.signal });
    const result = await objects.putObject(key, data);
    return { cid: result.cid, size: result.size };
  }

  /** Download an object by key. The SDK checks the content against the bucket's tree. */
  async getObject(bucketId: bigint, key: string): Promise<Uint8Array> {
    const bucketObjects = await this.requireW3s().objects(bucketId);
    const { data } = await bucketObjects.getObject(key);
    return data;
  }

  /**
   * Every object whose key starts with `prefix`, in key order, from one
   * read of the bucket's tree (paging would read the tree once per page
   * and could mix two bucket roots).
   */
  async listObjects(bucketId: bigint, prefix?: string): Promise<S3ObjectInfo[]> {
    const bucketObjects = await this.requireW3s().objects(bucketId);
    const { objects } = await bucketObjects.listAllObjects({ prefix });
    return objects.map((o) => ({
      key: o.key,
      size: o.size,
      lastModified: o.lastModified ?? 0,
      etag: o.etag ?? "",
    }));
  }

  async deleteObject(bucketId: bigint, key: string): Promise<void> {
    const bucketObjects = await this.requireW3s().objects(bucketId);
    await bucketObjects.deleteObject(key);
  }

  // ── Members ─────────────────────────────────────────────────────────────────

  async getBucketMembers(bucketId: bigint): Promise<BucketMember[]> {
    const info = await this.requireW3s().getBucket(bucketId);
    if (!info) throw new Error(`Bucket ${bucketId} not found`);
    return info.members;
  }

  addMember(bucketId: bigint, account: string, role: MemberRole): Promise<void> {
    return this.requireW3s().setMember(bucketId, account, role);
  }

  removeMember(bucketId: bigint, account: string): Promise<void> {
    return this.requireW3s().removeMember(bucketId, account);
  }

  async getBucketVisibility(bucketId: bigint): Promise<Visibility> {
    return getBucketVisibilityQuery(this.requireApi(), bucketId);
  }

  async setBucketVisibility(bucketId: bigint, visibility: Visibility): Promise<void> {
    await setBucketVisibilityTx(this.requireApi(), this.requireOwner(), bucketId, visibility);
  }

  // ── Provider discovery (UI-side; the `providers` and
  // `find_matching_providers` runtime APIs — not modelled by the SDK) ─────────

  /** Every registered provider, sorted by free capacity descending. */
  async listAvailableProviders(): Promise<AvailableProvider[]> {
    const api = this.requireApi();
    const page = await api.apis.StorageProviderApi.providers(0, ALL_PROVIDERS);

    const providers: AvailableProvider[] = page.map(([account, info]) => ({
      account,
      multiaddr: new TextDecoder().decode(info.multiaddr),
      stake: info.stake,
      // The chain already computed this; `null` is its "unlimited".
      availableCapacity:
        info.available_capacity == null ? undefined : BigInt(info.available_capacity),
      maxCapacity: BigInt(info.max_capacity),
      pricePerByte: info.price_per_byte,
      minDuration: info.min_duration,
      maxDuration: info.max_duration,
      acceptingPrimary: info.accepting_primary,
      minAgreementBytes: info.min_agreement_bytes,
      agreementsTotal: info.stats.agreements_total,
    }));

    providers.sort(byFreeCapacityDesc);

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
        // The chain already computed this; `null` is its "unlimited".
        const availableCapacity =
          info.available_capacity == null ? undefined : BigInt(info.available_capacity);

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
          minAgreementBytes: BigInt(info.min_agreement_bytes ?? 0),
          replicaSyncPrice:
            info.replica_sync_price != null ? BigInt(info.replica_sync_price) : undefined,
          acceptingExtensions: info.accepting_extensions ?? false,
          registeredAt: Number(info.stats.registered_at ?? 0),
          agreementsTotal: info.stats.agreements_total ?? 0,
          agreementsExtended: info.stats.agreements_extended ?? 0,
          agreementsNotExtended: info.stats.agreements_not_extended ?? 0,
          agreementsBurned: info.stats.agreements_burned ?? 0,
          challengesDefended:
            (info.stats.challenges_received_authorized ?? 0) +
            (info.stats.challenges_received_public ?? 0),
          challengesFailed: info.stats.challenges_failed ?? 0,
          reputation: info.stats.reputation,
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
    // Reader-gated on private buckets — sign as the connected owner.
    const headers = await signProviderRequest(this.requireOwner().signer, "GET", bucketId);
    const response = await httpFetch(`${providerUrl}/mmr_proof?${params.toString()}`, { headers });
    if (!response.ok) {
      throw new Error(`MMR proof request failed: ${response.status}`);
    }
    const result = await response.json();
    if (!result.leaf) throw new Error("MMR proof response has no leaf");
    return leafChunkCount(result.leaf);
  }

  /** Slash the provider of a challenge that expired unanswered; anyone may call it. */
  async resolveExpiredChallenge(
    challengeId: { deadline: number; index: number },
  ): Promise<ChallengeSlashResult> {
    const api = this.requireApi();
    const result = await resolveExpiredChallengeTx(api, this.requireOwner(), challengeId, {
      mode: "finalized",
    });
    if (result.type !== "finalized") {
      throw new Error("resolve_expired_challenge did not finalize");
    }
    const slashed = requireOneEvent(
      result.events,
      api.event.StorageProvider.ChallengeSlashed,
      "ChallengeSlashed",
    );
    return {
      challengeId: { deadline: slashed.challenge_id.deadline, index: slashed.challenge_id.index },
      provider: slashed.provider,
      slashedAmount: slashed.slashed_amount,
      blockNumber: result.block.number,
      blockHash: result.block.hash,
    };
  }

  async isChallengeActive(challengeId: { deadline: number; index: number }): Promise<boolean> {
    const api = this.requireApi();
    const entry = await api.query.StorageProvider.Challenges.getValue(
      challengeId.deadline,
      challengeId.index,
    );
    return entry !== undefined;
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
