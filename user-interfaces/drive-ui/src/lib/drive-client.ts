// SPDX-License-Identifier: GPL-3.0-only

/**
 * Drive Client — drive-ui's thin adapter over the sdk's Web3Storage.
 * What stays here is app-shaped: stateless signer swapping driven by the
 * state layer, Blob conversion for the browser download path, and the
 * progress-status strings. Everything chain/HTTP-mechanical (tx submission,
 * provider resolution + caching, retry/backoff, request signing, watch-based
 * acceptance waits) lives in @web3-storage/sdk.
 */

import { parachain } from "@polkadot-api/descriptors";
import {
  buildSignedTermsArgs,
  createBucketWithPrimary,
  negotiateTerms,
  parseMultiaddrToUrl,
  getBucketVisibility as getBucketVisibilityQuery,
  setBucketVisibility as setBucketVisibilityTx,
  fetchBucketUsage,
  type BucketUsage,
  type ChainSigner,
  type Keypair,
  type NegotiateRequest,
  type SignedTerms,
  type Visibility,
  Web3Storage,
  type BucketInfo,
  type BucketMember,
  type MemberRole,
  findMatchingProviders,
  getCheckpointInfo,
  listProviders,
  type AvailableProvider,
  type CheckpointInfo,
  type MatchingProviders,
  type QueryMatchingProvidersParams,
} from "@web3-storage/sdk";

export type { AvailableProvider, CheckpointInfo, MatchingProviders, QueryMatchingProvidersParams };
import type { ParachainApi } from "@/state/chain.state";

export type Signer = ChainSigner["signer"];

// Re-export the SDK negotiate primitives + types the create-drive components
// (NewDriveDialog, ProviderPickerPanel) and the state layer import from here.
export { buildSignedTermsArgs, negotiateTerms };
export type { BucketUsage, NegotiateRequest, SignedTerms, Visibility };

/** `parseMultiaddrToHttp` is the SDK's `parseMultiaddrToUrl` under the old name. */
export const parseMultiaddrToHttp = parseMultiaddrToUrl;

export type { BucketMember, MemberRole };

/** A drive is a bucket; the chain stores nothing else about it. */
export type DriveInfo = BucketInfo;

/** A directory entry as the file browser shows it. */
export interface FsEntry {
  name: string;
  path: string;
  entryType: "file" | "directory";
  size: number;
  /** Milliseconds since the epoch (stored with second precision). */
  mtime: number;
  /** File: CID of its manifest. Directory: CID of its directory node. 0x-hex. */
  cid: string;
}

export interface UploadOptions {
  contentType?: string;
  signal?: AbortSignal;
}

export class DriveClient {
  private api: ParachainApi | null = null;
  private signer: Signer | null = null;
  private signerAddress: string | null = null;
  private keypair: Keypair | null = null;
  private w3s: Web3Storage | null = null;

  private rebuild(): void {
    if (!this.api) {
      this.w3s = null;
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
    if (!this.signer || !this.signerAddress) throw new Error("Signer not set");
    return {
      signer: this.signer,
      address: this.signerAddress,
      publicKey: this.signer.publicKey,
      keypair: this.keypair ?? undefined,
    };
  }

  // ── Provider resolution ───────────────────────────────────────────────────

  getProviderUrl(bucketId: bigint): Promise<string> {
    return this.requireW3s().getProviderUrl(bucketId);
  }

  invalidateProviderUrl(bucketId: bigint): void {
    this.w3s?.invalidateProviderUrl(bucketId);
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
      const url = await this.requireW3s().waitForProvider(bucketId, {
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

  /** Every registered provider, most free capacity first. */
  listAvailableProviders(): Promise<AvailableProvider[]> {
    return listProviders(this.requireApi());
  }

  /** Providers scored against storage requirements, best match first. */
  queryMatchingProviders(
    query: QueryMatchingProvidersParams["query"],
    limit: QueryMatchingProvidersParams["limit"],
  ): Promise<MatchingProviders[]> {
    return findMatchingProviders(this.requireApi(), query, limit);
  }

  /**
   * Redeem provider-signed agreement terms on chain in
   * `create_bucket_with_primary`: opens a Layer 0 bucket with one primary
   * agreement. The bucket is the drive.
   *
   * **Step 2 of drive creation.** Step 1 is the HTTP `negotiateTerms` call
   * against the chosen provider; splitting the two lets a failed on-chain
   * submit be retried without re-negotiating (terms valid until
   * `terms.valid_until`).
   */
  async submitCreateDrive(
    providerAccount: string,
    signed: SignedTerms,
    visibility?: Visibility,
  ): Promise<{ bucketId: bigint }> {
    const api = this.requireApi();
    const owner = this.requireOwner();

    // Finalize: the state layer reads the drive list back at the finalized
    // head right after this resolves, so an in-block ("best") submit would
    // race a reorg and the just-created drive could be missing from the
    // refreshed list. `retryStale: 0` — a user retry is the right UX for a UI.
    const { bucketId } = await createBucketWithPrimary(
      api,
      owner,
      { address: providerAccount },
      signed,
      { mode: "finalized", retryStale: 0, visibility },
    );
    return { bucketId };
  }

  listDrives(): Promise<DriveInfo[]> {
    return this.requireW3s().listBuckets(this.signerAddress ?? undefined);
  }

  getDrive(bucketId: bigint): Promise<DriveInfo | null> {
    return this.requireW3s().getBucket(bucketId);
  }

  /** Physical usage vs paid quota, read from the drive's provider node. */
  async getDriveUsage(bucketId: bigint): Promise<BucketUsage> {
    return fetchBucketUsage(await this.getProviderUrl(bucketId), bucketId);
  }

  // ── FS HTTP operations ────────────────────────────────────────────────────

  async listDirectory(bucketId: bigint, path: string): Promise<FsEntry[]> {
    const drive = await this.requireW3s().drive(bucketId);
    const entries = await drive.list(path);
    return entries.map((e) => ({
      name: e.name,
      path: e.path,
      entryType: e.entryType,
      size: Number(e.size),
      mtime: Number(e.mtime) * 1000,
      cid: e.cid,
    }));
  }

  async uploadFile(
    bucketId: bigint,
    path: string,
    data: Uint8Array,
    options: UploadOptions = {},
  ): Promise<void> {
    const drive = await this.requireW3s().drive(bucketId, { signal: options.signal });
    await drive.putFile(path, data, { contentType: options.contentType });
  }

  async downloadFile(bucketId: bigint, path: string): Promise<Blob> {
    const drive = await this.requireW3s().drive(bucketId);
    const file = await drive.getFile(path);
    return new Blob([file.bytes as BlobPart]);
  }

  async deleteFile(bucketId: bigint, path: string): Promise<void> {
    const drive = await this.requireW3s().drive(bucketId);
    await drive.delete(path);
  }

  async createDirectory(bucketId: bigint, path: string): Promise<void> {
    const drive = await this.requireW3s().drive(bucketId);
    await drive.mkdir(path);
  }

  // ── Members ───────────────────────────────────────────────────────────────

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

  // ── Checkpoint ────────────────────────────────────────────────────────────

  getCheckpointInfo(bucketId: bigint): Promise<CheckpointInfo | null> {
    return getCheckpointInfo(this.requireApi(), bucketId);
  }
}

// Re-export descriptor for tests / consumers that need event types.
export { parachain }
