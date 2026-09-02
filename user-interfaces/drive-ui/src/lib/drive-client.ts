// SPDX-License-Identifier: GPL-3.0-only

/**
 * Drive Client — drive-ui's thin adapter over the sdk's FileSystemClient.
 * What stays here is app-shaped: stateless signer swapping driven by the
 * state layer, Blob conversion for the browser download path, and the
 * progress-status strings. Everything chain/HTTP-mechanical (tx submission,
 * provider resolution + caching, retry/backoff, request signing, watch-based
 * acceptance waits) lives in @web3-storage/sdk.
 */

import type { PolkadotSigner } from "polkadot-api";
import { parachain } from "@polkadot-api/descriptors";
import {
  buildSignedTermsArgs,
  createDrive as createDriveTx,
  negotiateTerms,
  parseMultiaddrToUrl,
  toSs58,
  type BucketUsage,
  type ChainSigner,
  type Keypair,
  type NegotiateRequest,
  type SignedTerms,
} from "@web3-storage/sdk";
import { FileSystemClient } from "@web3-storage/sdk/fs";
import type { ParachainApi } from "@/state/chain.state";

export type Signer = PolkadotSigner;

// Re-export the SDK negotiate primitives + types the create-drive components
// (NewDriveDialog, ProviderPickerPanel) and the state layer import from here.
export { buildSignedTermsArgs, negotiateTerms };
export type { BucketUsage, NegotiateRequest, SignedTerms };

/** `parseMultiaddrToHttp` is the SDK's `parseMultiaddrToUrl` under the old name. */
export const parseMultiaddrToHttp = parseMultiaddrToUrl;

export type {
  BucketMember,
  CreateDriveOptions,
  DriveInfo,
  FsEntry,
  MemberRole,
  UploadOptions,
} from "@web3-storage/sdk/fs";
import type {
  BucketMember,
  CreateDriveOptions,
  DriveInfo,
  FsEntry,
  MemberRole,
  UploadOptions,
} from "@web3-storage/sdk/fs";

export interface CheckpointInfo {
  mmrRoot: string;
  startSeq: bigint;
  leafCount: bigint;
  checkpointBlock: number;
}

/**
 * A registered provider as surfaced by the provider picker. UI-side discovery
 * type (raw `StorageProvider.Providers` sweep) — stays in the UI, not the SDK.
 */
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

/**
 * A provider scored against storage requirements via the
 * `find_matching_providers` runtime API. Carries a `matchScore` and
 * `partialReason` so the picker can rank "best fit" vs near-misses.
 */
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

export interface QueryMatchingProvidersParams {
  query: {
    bytesNeeded: bigint;
    minDuration: number;
    maxPricePerByte: bigint;
    primaryOnly: boolean;
  };
  limit: number;
}

export class DriveClient {
  private api: ParachainApi | null = null;
  private signer: Signer | null = null;
  private signerAddress: string | null = null;
  private keypair: Keypair | null = null;
  private fsc: FileSystemClient | null = null;

  private rebuild(): void {
    if (!this.api) {
      this.fsc = null;
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
    this.fsc = new FileSystemClient({ api: this.api, signer: chainSigner });
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

  private requireFs(): FileSystemClient {
    if (!this.fsc) throw new Error("Not connected to chain");
    return this.fsc;
  }

  // ── Provider resolution ───────────────────────────────────────────────────

  getProviderUrl(bucketId: bigint): Promise<string> {
    return this.requireFs().getProviderUrl(bucketId);
  }

  invalidateProviderUrl(bucketId: bigint): void {
    this.fsc?.invalidateProviderUrl(bucketId);
  }

  async waitForProvider(
    bucketId: bigint,
    onProgress?: (status: string, elapsedMs: number) => void,
  ): Promise<string> {
    const statusFor = (elapsedSec: number): string => {
      if (elapsedSec < 15) return "Checking if provider has accepted the agreement...";
      if (elapsedSec < 45) return "Provider is reviewing the agreement...";
      if (elapsedSec < 90) return "Still waiting for provider to accept...";
      return "Taking longer than usual — provider may be busy or offline...";
    };

    onProgress?.(statusFor(0), 0);
    const startTime = Date.now();
    try {
      const url = await this.requireFs().waitForProvider(bucketId, {
        timeoutMs: 150_000,
        tickMs: 3_000,
        onTick: (elapsedMs) => onProgress?.(statusFor(Math.round(elapsedMs / 1000)), elapsedMs),
      });
      onProgress?.("Provider accepted — ready to use", Date.now() - startTime);
      return url;
    } catch {
      throw new Error(
        `Provider did not accept the agreement after ${Math.round(
          (Date.now() - startTime) / 1000,
        )}s. The provider may be offline or not accepting new agreements.`,
      );
    }
  }

  // ── Account ───────────────────────────────────────────────────────────────

  async getBalance(address: string): Promise<{ free: bigint; reserved: bigint }> {
    const api = this.requireApi();
    const account = await api.query.System.Account.getValue(address);
    return { free: account.data.free, reserved: account.data.reserved };
  }

  // ── Drive on-chain operations ─────────────────────────────────────────────

  async createDrive(options: CreateDriveOptions): Promise<DriveInfo> {
    const { driveId, bucketId, provider } = await this.requireFs().createDrive(options);
    return {
      driveId,
      bucketId,
      owner: this.signerAddress ?? "",
      name: options.name ?? null,
      maxCapacity: options.maxCapacity,
      createdAt: 0,
      storagePeriod: options.storagePeriod,
      expiresAt: 0,
      providerInfo: [{ account: provider, multiaddr: "", url: options.provider?.url ?? null }],
    };
  }

  /**
   * Walk `StorageProvider.Providers` storage and return all registered
   * providers, sorted by free capacity descending. Used by the provider
   * picker to surface candidates before negotiation. UI-side discovery — the
   * SDK's `discoverAcceptingProvider` picks one; this lists them all.
   */
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
        agreementsTotal: (provider.stats as { agreements_total?: number })?.agreements_total ?? 0,
      });
    }

    providers.sort((a, b) => {
      if (b.availableCapacity > a.availableCapacity) return 1;
      if (b.availableCapacity < a.availableCapacity) return -1;
      return 0;
    });

    return providers;
  }

  /**
   * Score registered providers against the given storage requirements via the
   * `find_matching_providers` runtime API. Returns a `matchScore` and
   * `partialReason` per provider so the picker can rank candidates.
   */
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

  /**
   * Redeem provider-signed agreement terms on chain to open a Layer-0 bucket
   * + primary agreement and register the drive on top — atomically in one
   * extrinsic (via the layer-0 `createDrive` wrapper, submitted finalized).
   *
   * **Step 2 of drive creation.** Step 1 is the HTTP `negotiateTerms` call
   * against the chosen provider; splitting the two lets a failed on-chain
   * submit be retried without re-negotiating (terms valid until
   * `terms.valid_until`). `providerUrl` is used only to prime the provider
   * URL cache for the first upload.
   */
  async submitCreateDrive(
    name: string | undefined,
    providerAccount: string,
    providerUrl: string,
    signed: SignedTerms,
  ): Promise<DriveInfo> {
    const api = this.requireApi();
    if (!this.signer || !this.signerAddress) {
      throw new Error("Signer not set");
    }
    const owner: ChainSigner = {
      signer: this.signer,
      address: this.signerAddress,
      publicKey: this.signer.publicKey,
      keypair: this.keypair ?? undefined,
    };

    // Finalize: the state layer reads the drive list back at the finalized
    // head right after this resolves, so an in-block ("best") submit would
    // race a reorg and the just-created drive could be missing from the
    // refreshed list. `retryStale: 0` — a user retry is the right UX for a UI.
    const { driveId, bucketId } = await createDriveTx(
      api,
      owner,
      name ?? "",
      { address: providerAccount },
      signed,
      { mode: "finalized", retryStale: 0 },
    );

    return {
      driveId,
      bucketId,
      owner: this.signerAddress,
      name: name ?? null,
      maxCapacity: BigInt(signed.terms.max_bytes),
      createdAt: 0,
      storagePeriod: signed.terms.duration,
      expiresAt: 0,
      providerInfo: [{ account: providerAccount, multiaddr: "", url: providerUrl }],
    };
  }

  listDrives(): Promise<DriveInfo[]> {
    return this.requireFs().listDrives(this.signerAddress ?? undefined);
  }

  getDrive(driveId: bigint): Promise<DriveInfo | null> {
    return this.requireFs().getDrive(driveId);
  }

  /** Delete the drive; resolves with the prorated refund from the event. */
  deleteDrive(driveId: bigint): Promise<{ refunded: bigint }> {
    return this.requireFs().deleteDrive(driveId);
  }

  /** Physical usage vs paid quota, read from the drive's provider node. */
  getDriveUsage(driveId: bigint): Promise<BucketUsage> {
    return this.requireFs().getDriveUsage(driveId);
  }

  // ── FS HTTP operations ────────────────────────────────────────────────────

  listDirectory(bucketId: bigint, path: string): Promise<FsEntry[]> {
    return this.requireFs().listDirectory(bucketId, path);
  }

  async uploadFile(
    bucketId: bigint,
    path: string,
    data: Uint8Array,
    options: UploadOptions = {},
  ): Promise<void> {
    await this.requireFs().uploadFile(bucketId, path, data, options);
  }

  async downloadFile(bucketId: bigint, path: string): Promise<Blob> {
    const bytes = await this.requireFs().downloadFile(bucketId, path);
    return new Blob([bytes as BlobPart]);
  }

  deleteFile(bucketId: bigint, path: string): Promise<void> {
    return this.requireFs().deleteFile(bucketId, path);
  }

  createDirectory(bucketId: bigint, path: string): Promise<void> {
    return this.requireFs().createDirectory(bucketId, path);
  }

  // ── Members ───────────────────────────────────────────────────────────────

  getBucketMembers(bucketId: bigint): Promise<BucketMember[]> {
    return this.requireFs().getBucketMembers(bucketId);
  }

  addMember(bucketId: bigint, account: string, role: MemberRole): Promise<void> {
    return this.requireFs().addMember(bucketId, account, role);
  }

  removeMember(bucketId: bigint, account: string): Promise<void> {
    return this.requireFs().removeMember(bucketId, account);
  }

  // ── Checkpoint ────────────────────────────────────────────────────────────

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
}

// Re-export descriptor for tests / consumers that need event types.
export { parachain }
