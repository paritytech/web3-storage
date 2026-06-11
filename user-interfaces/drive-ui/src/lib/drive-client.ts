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
import { ss58Decode } from "@polkadot-labs/hdkd-helpers";
import { READ_OPTS, type ChainSigner } from "@web3-storage/sdk";
import { FileSystemClient } from "@web3-storage/sdk/fs";
import type { ParachainApi } from "@/state/chain.state";

export type Signer = PolkadotSigner;

export type {
  BucketMember,
  CheckpointDuty,
  CreateDriveOptions,
  DriveInfo,
  FsEntry,
  MemberRole,
  UploadOptions,
} from "@web3-storage/sdk/fs";
import type {
  BucketMember,
  CheckpointDuty,
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

export class DriveClient {
  private api: ParachainApi | null = null;
  private signer: Signer | null = null;
  private signerAddress: string | null = null;
  private fsc: FileSystemClient | null = null;

  private rebuild(): void {
    if (!this.api) {
      this.fsc = null;
      return;
    }
    let chainSigner: ChainSigner | null = null;
    if (this.signer && this.signerAddress) {
      // Wallet flows hand us a PolkadotSigner + address; recover the public
      // key from the address. No raw keypair here, so provider requests go
      // unsigned (same as this app always behaved).
      const [publicKey] = ss58Decode(this.signerAddress);
      chainSigner = {
        signer: this.signer,
        address: this.signerAddress,
        publicKey,
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

  setSigner(signer: Signer | null, address: string | null): void {
    this.signer = signer;
    this.signerAddress = address;
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
    const account = await api.query.System.Account.getValue(address, READ_OPTS);
    return { free: account.data.free, reserved: account.data.reserved };
  }

  // ── Drive on-chain operations ─────────────────────────────────────────────

  async createDrive(options: CreateDriveOptions): Promise<DriveInfo> {
    const { driveId, bucketId } = await this.requireFs().createDrive(options);
    return {
      driveId,
      bucketId,
      owner: this.signerAddress ?? "",
      name: options.name ?? null,
      maxCapacity: options.maxCapacity,
      createdAt: 0,
      storagePeriod: options.storagePeriod,
      expiresAt: 0,
      payment: options.payment,
    };
  }

  listDrives(): Promise<DriveInfo[]> {
    return this.requireFs().listDrives(this.signerAddress ?? undefined);
  }

  getDrive(driveId: bigint): Promise<DriveInfo | null> {
    return this.requireFs().getDrive(driveId);
  }

  async deleteDrive(driveId: bigint): Promise<void> {
    await this.requireFs().deleteDrive(driveId);
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
    const bucket = await api.query.StorageProvider.Buckets.getValue(bucketId, READ_OPTS);
    if (!bucket) throw new Error(`Bucket ${bucketId} not found`);

    const snapshot = bucket.snapshot;
    if (!snapshot) return null;

    return {
      mmrRoot: snapshot.mmr_root,
      startSeq: snapshot.start_seq,
      leafCount: snapshot.leaf_count,
      checkpointBlock: snapshot.checkpoint_block,
    };
  }

  getCheckpointDuty(bucketId: bigint): Promise<CheckpointDuty | null> {
    return this.requireFs().getCheckpointDuty(bucketId);
  }

  triggerCheckpoint(bucketId: bigint): Promise<void> {
    return this.requireFs().triggerCheckpoint(bucketId);
  }
}

// Re-export descriptor for tests / consumers that need event types.
export { parachain }
