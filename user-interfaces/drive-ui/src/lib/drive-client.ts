/**
 * Drive Client — Browser-compatible wrapper for DriveRegistry pallet + FS HTTP endpoints.
 * Modeled after console-ui's StorageClient but targets the file-system interface.
 */

import { createClient, type PolkadotClient, type TypedApi } from "polkadot-api";
import { getWsProvider } from "polkadot-api/ws-provider/web";
import { getPolkadotSigner } from "polkadot-api/signer";
import { Keyring } from "@polkadot/keyring";
import { cryptoWaitReady } from "@polkadot/util-crypto";
import { parachain } from "@polkadot-api/descriptors";
import { Binary, Enum } from "polkadot-api";

// ─────────────────────────────────────────────────────────────────────────────
// Types
// ─────────────────────────────────────────────────────────────────────────────

interface TxResult {
  blockHash: string;
  blockNumber: number;
  events: any[];
}

export interface DriveInfo {
  driveId: bigint;
  bucketId: bigint;
  owner: string;
  name: string | null;
  maxCapacity: bigint;
  createdAt: bigint;
  storagePeriod: bigint;
  expiresAt: bigint;
  payment: bigint;
}

export interface FsEntry {
  name: string;
  path: string;
  entryType: "file" | "directory";
  size: number;
  mtime: number;
}

export interface CreateDriveOptions {
  name?: string;
  maxCapacity: bigint;
  storagePeriod: number;
  payment: bigint;
  minProviders?: number;
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

export interface CheckpointDuty {
  bucketId: number;
  mmrRoot: string;
  startSeq: number;
  leafCount: number;
  ready: boolean;
}

type ParachainApi = TypedApi<typeof parachain>;

// ─────────────────────────────────────────────────────────────────────────────
// Client
// ─────────────────────────────────────────────────────────────────────────────

export class DriveClient {
  private chainWs: string;
  private client: PolkadotClient | null = null;
  private api: ParachainApi | null = null;
  private signer: ReturnType<typeof getPolkadotSigner> | null = null;
  private signerAddress: string | null = null;
  private providerUrlCache: Map<string, string> = new Map();

  constructor(chainWs: string) {
    this.chainWs = chainWs;
  }

  // ── Connection ────────────────────────────────────────────────────────────

  async connect(): Promise<void> {
    await cryptoWaitReady();
    this.client = createClient(getWsProvider(this.chainWs));
    this.api = this.client.getTypedApi(parachain);
  }

  async setSigner(seed: string): Promise<string> {
    await cryptoWaitReady();
    const keyring = new Keyring({ type: "sr25519" });
    const account = keyring.addFromUri(seed);
    this.signer = getPolkadotSigner(account.publicKey, "Sr25519", (input) =>
      account.sign(input)
    );
    this.signerAddress = account.address;
    return account.address;
  }

  getAddress(): string | null {
    return this.signerAddress;
  }

  disconnect(): void {
    if (this.client) {
      this.client.destroy();
      this.client = null;
      this.api = null;
    }
  }

  isConnected(): boolean {
    return this.client !== null && this.api !== null;
  }

  hasSigner(): boolean {
    return this.signer !== null;
  }

  private ensureConnected(): void {
    if (!this.api) throw new Error("Not connected. Call connect() first.");
    if (!this.signer) throw new Error("Signer not set. Call setSigner() first.");
  }

  // ── Tx submission ─────────────────────────────────────────────────────────

  private submitAndWatchBestBlock(tx: any): Promise<TxResult> {
    return new Promise((resolve, reject) => {
      let resolved = false;

      const handleEvent = (ev: any) => {
        if (ev.type === "txBestBlocksState" && ev.found && !resolved) {
          resolved = true;
          subscription.unsubscribe();

          const events = ev.events ?? [];

          const failedEvent = events.find(
            (e: any) => e.type === "System" && e.value?.type === "ExtrinsicFailed"
          );
          if (failedEvent) {
            const dispatchError = failedEvent.value?.value?.dispatch_error ?? ev.dispatchError;
            const errorStr = dispatchError
              ? JSON.stringify(dispatchError, (_k: string, v: unknown) =>
                  typeof v === "bigint" ? v.toString() : v
                )
              : "unknown dispatch error";
            reject(new Error(`Transaction failed on-chain: ${errorStr}`));
            return;
          }

          resolve({
            blockHash: ev.block.hash,
            blockNumber: ev.block.number,
            events,
          });
        }
      };

      const handleError = (err: any) => {
        if (!resolved) {
          resolved = true;
          subscription?.unsubscribe();
          reject(err);
        }
      };

      const subscription = tx.signSubmitAndWatch(this.signer!).subscribe({
        next: handleEvent,
        error: handleError,
      });

      setTimeout(() => {
        if (!resolved) {
          resolved = true;
          subscription?.unsubscribe();
          reject(new Error("Transaction timed out"));
        }
      }, 120000);
    });
  }

  // ── Provider resolution (same as StorageClient) ───────────────────────────

  private parseMultiaddrToHttp(multiaddr: string): string | null {
    const parts = multiaddr.split("/").filter(Boolean);
    let host: string | null = null;
    let port: string | null = null;

    for (let i = 0; i < parts.length; i++) {
      if (
        (parts[i] === "ip4" || parts[i] === "ip6" || parts[i] === "dns4" || parts[i] === "dns6") &&
        parts[i + 1]
      ) {
        host = parts[i + 1];
      }
      if (parts[i] === "tcp" && parts[i + 1]) {
        port = parts[i + 1];
      }
    }

    if (host && port) return `http://${host}:${port}`;
    return null;
  }

  private async resolveProviderEndpoint(bucketId: bigint): Promise<string> {
    if (!this.api) throw new Error("Not connected");

    const bucket = await this.api.query.StorageProvider.Buckets.getValue(bucketId);
    if (!bucket) throw new Error(`Bucket ${bucketId} not found on chain`);

    const providers: string[] = bucket.primary_providers ?? [];
    if (providers.length === 0) {
      throw new Error(`Bucket ${bucketId} has no primary providers`);
    }

    for (const providerAccount of providers) {
      const provider = await this.api.query.StorageProvider.Providers.getValue(providerAccount);
      if (!provider) continue;

      let multiaddrStr: string;
      if (provider.multiaddr instanceof Uint8Array) {
        multiaddrStr = new TextDecoder().decode(provider.multiaddr);
      } else if (typeof provider.multiaddr.asBytes === "function") {
        multiaddrStr = new TextDecoder().decode(provider.multiaddr.asBytes());
      } else {
        multiaddrStr = String(provider.multiaddr);
      }

      const url = this.parseMultiaddrToHttp(multiaddrStr);
      if (url) return url;
    }

    throw new Error(`Could not resolve HTTP endpoint for bucket ${bucketId} providers`);
  }

  async getProviderUrl(bucketId: bigint): Promise<string> {
    const key = bucketId.toString();
    const cached = this.providerUrlCache.get(key);
    if (cached) return cached;

    const url = await this.resolveProviderEndpoint(bucketId);
    this.providerUrlCache.set(key, url);
    return url;
  }

  async waitForProvider(
    bucketId: bigint,
    onProgress?: (status: string, elapsedMs: number) => void,
  ): Promise<string> {
    this.providerUrlCache.delete(bucketId.toString());

    const intervals = [
      0, 3000, 3000, 3000, 3000, 3000,
      6000, 6000, 6000, 6000, 6000,
      10000, 10000, 10000, 10000, 10000,
      10000, 10000, 10000, 10000, 10000,
    ];
    const startTime = Date.now();

    for (let i = 0; i < intervals.length; i++) {
      if (intervals[i] > 0) {
        await new Promise((r) => setTimeout(r, intervals[i]));
      }

      const elapsedMs = Date.now() - startTime;
      const elapsedSec = Math.round(elapsedMs / 1000);

      let status: string;
      if (elapsedSec < 15) {
        status = "Checking if provider has accepted the agreement...";
      } else if (elapsedSec < 45) {
        status = "Provider is reviewing the agreement...";
      } else if (elapsedSec < 90) {
        status = "Still waiting for provider to accept...";
      } else {
        status = "Taking longer than usual — provider may be busy or offline...";
      }

      onProgress?.(status, elapsedMs);

      try {
        const url = await this.resolveProviderEndpoint(bucketId);
        this.providerUrlCache.set(bucketId.toString(), url);
        onProgress?.("Provider accepted — ready to use", Date.now() - startTime);
        return url;
      } catch (err) {
        // Retry on transient states: bucket not yet visible, or no providers assigned yet
        const retryable =
          err instanceof Error &&
          (err.message.includes("no primary providers") || err.message.includes("not found on chain"));
        if (!retryable) throw err;
        if (i === intervals.length - 1) {
          throw new Error(
            `Provider did not accept the agreement after ${Math.round((Date.now() - startTime) / 1000)}s. ` +
              `The provider may be offline or not accepting new agreements.`
          );
        }
      }
    }
    throw new Error("Provider did not accept the agreement");
  }

  // ── Account ───────────────────────────────────────────────────────────────

  async getBalance(address: string): Promise<{ free: bigint; reserved: bigint }> {
    if (!this.api) throw new Error("Not connected");

    const account = await this.api.query.System.Account.getValue(address);
    return {
      free: BigInt(account.data.free),
      reserved: BigInt(account.data.reserved),
    };
  }

  // ── Drive on-chain operations ─────────────────────────────────────────────

  async createDrive(options: CreateDriveOptions): Promise<DriveInfo> {
    this.ensureConnected();

    const nameArg = options.name ? Binary.fromText(options.name) : undefined;

    const tx = this.api!.tx.DriveRegistry.create_drive({
      name: nameArg,
      max_capacity: options.maxCapacity,
      storage_period: options.storagePeriod,
      payment: options.payment,
      min_providers: options.minProviders ?? undefined,
    });

    const result = await this.submitAndWatchBestBlock(tx);

    // Extract drive info from events
    let driveId: bigint | null = null;
    let bucketId: bigint | null = null;
    for (const event of result.events) {
      if (event.type === "DriveRegistry" && event.value.type === "DriveCreated") {
        driveId = BigInt(event.value.value.drive_id);
        bucketId = BigInt(event.value.value.bucket_id);
        break;
      }
    }

    if (driveId === null || bucketId === null) {
      // Fallback: query chain for user drives
      const drives = await this.listDrives();
      if (drives.length > 0) {
        return drives[drives.length - 1];
      }
      throw new Error(
        "DriveCreated event not found. The runtime descriptor may be stale — run: npx papi update"
      );
    }

    return {
      driveId,
      bucketId,
      owner: this.signerAddress!,
      name: options.name ?? null,
      maxCapacity: options.maxCapacity,
      createdAt: BigInt(Date.now()),
      storagePeriod: BigInt(options.storagePeriod),
      expiresAt: 0n,
      payment: options.payment,
    };
  }

  async listDrives(): Promise<DriveInfo[]> {
    this.ensureConnected();

    const driveIds = await this.api!.query.DriveRegistry.UserDrives.getValue(
      this.signerAddress!
    );

    if (!driveIds || driveIds.length === 0) return [];

    const drives: DriveInfo[] = [];
    for (const driveId of driveIds) {
      const drive = await this.api!.query.DriveRegistry.Drives.getValue(driveId);
      if (drive) {
        let driveName: string | null = null;
        if (drive.name) {
          try {
            if (drive.name instanceof Uint8Array) {
              driveName = new TextDecoder().decode(drive.name);
            } else if (typeof drive.name.asText === "function") {
              driveName = drive.name.asText();
            } else if (typeof drive.name.asBytes === "function") {
              driveName = new TextDecoder().decode(drive.name.asBytes());
            } else {
              driveName = String(drive.name);
            }
          } catch {
            driveName = null;
          }
        }

        drives.push({
          driveId: BigInt(driveId),
          bucketId: BigInt(drive.bucket_id),
          owner: String(drive.owner),
          name: driveName,
          maxCapacity: BigInt(drive.max_capacity),
          createdAt: BigInt(drive.created_at),
          storagePeriod: BigInt(drive.storage_period),
          expiresAt: BigInt(drive.expires_at),
          payment: BigInt(drive.payment),
        });
      }
    }
    return drives;
  }

  async deleteDrive(driveId: bigint): Promise<void> {
    this.ensureConnected();

    const tx = this.api!.tx.DriveRegistry.delete_drive({
      drive_id: driveId,
    });

    await this.submitAndWatchBestBlock(tx);
  }

  // ── FS HTTP operations ────────────────────────────────────────────────────

  async listDirectory(bucketId: bigint, path: string): Promise<FsEntry[]> {
    const providerUrl = await this.getProviderUrl(bucketId);
    const params = new URLSearchParams({ path });

    const response = await fetch(
      `${providerUrl}/fs/${Number(bucketId)}/ls?${params.toString()}`
    );

    if (!response.ok) {
      throw new Error(`List directory failed: ${response.status}`);
    }

    const result = await response.json();
    return (result.entries || []).map((e: any) => ({
      name: e.name,
      path: e.path,
      entryType: e.entry_type as "file" | "directory",
      size: e.size ?? 0,
      mtime: (e.mtime ?? 0) * 1000, // Unix seconds -> JS millis
    }));
  }

  async uploadFile(
    bucketId: bigint,
    path: string,
    data: Uint8Array,
    contentType: string = "application/octet-stream"
  ): Promise<void> {
    const providerUrl = await this.getProviderUrl(bucketId);
    const response = await fetch(
      `${providerUrl}/fs/${Number(bucketId)}/file?path=${encodeURIComponent(path)}`,
      {
        method: "PUT",
        headers: { "Content-Type": contentType },
        body: data,
      }
    );

    if (!response.ok) {
      throw new Error(`Upload failed: ${response.status} ${await response.text()}`);
    }
  }

  async downloadFile(bucketId: bigint, path: string): Promise<Blob> {
    const providerUrl = await this.getProviderUrl(bucketId);
    const response = await fetch(
      `${providerUrl}/fs/${Number(bucketId)}/file?path=${encodeURIComponent(path)}`
    );

    if (!response.ok) {
      throw new Error(`Download failed: ${response.status}`);
    }

    return response.blob();
  }

  async deleteFile(bucketId: bigint, path: string): Promise<void> {
    const providerUrl = await this.getProviderUrl(bucketId);
    const response = await fetch(
      `${providerUrl}/fs/${Number(bucketId)}/file?path=${encodeURIComponent(path)}`,
      { method: "DELETE" }
    );

    if (!response.ok) {
      throw new Error(`Delete failed: ${response.status} ${await response.text()}`);
    }
  }

  async createDirectory(bucketId: bigint, path: string): Promise<void> {
    const providerUrl = await this.getProviderUrl(bucketId);
    const response = await fetch(
      `${providerUrl}/fs/${Number(bucketId)}/mkdir?path=${encodeURIComponent(path)}`,
      { method: "POST" }
    );

    if (!response.ok) {
      throw new Error(`Create directory failed: ${response.status} ${await response.text()}`);
    }
  }

  // ── Bucket Members & Permissions ──────────────────────────────────────────

  async getBucketMembers(bucketId: bigint): Promise<BucketMember[]> {
    if (!this.api) throw new Error("Not connected");

    const bucket = await this.api.query.StorageProvider.Buckets.getValue(bucketId);
    if (!bucket) throw new Error(`Bucket ${bucketId} not found`);

    const roleMap: Record<string, MemberRole> = {
      Admin: "Admin",
      Writer: "Writer",
      Reader: "Reader",
    };

    return (bucket.members ?? []).map((m: any) => ({
      account: m.account,
      role: roleMap[m.role?.type ?? m.role] ?? "Reader",
    }));
  }

  async addMember(bucketId: bigint, account: string, role: MemberRole): Promise<void> {
    this.ensureConnected();

    const tx = this.api!.tx.StorageProvider.set_member({
      bucket_id: bucketId,
      member: account,
      role: Enum(role),
    });

    await this.submitAndWatchBestBlock(tx);
  }

  async removeMember(bucketId: bigint, account: string): Promise<void> {
    this.ensureConnected();

    const tx = this.api!.tx.StorageProvider.remove_member({
      bucket_id: bucketId,
      member: account,
    });

    await this.submitAndWatchBestBlock(tx);
  }

  // ── Checkpoint ────────────────────────────────────────────────────────────

  async getCheckpointInfo(bucketId: bigint): Promise<CheckpointInfo | null> {
    if (!this.api) throw new Error("Not connected");

    const bucket = await this.api.query.StorageProvider.Buckets.getValue(bucketId);
    if (!bucket) throw new Error(`Bucket ${bucketId} not found`);

    const snapshot = bucket.snapshot;
    if (!snapshot) return null;

    let mmrRoot: string;
    const raw = snapshot.mmr_root;
    if (raw instanceof Uint8Array) {
      mmrRoot = "0x" + Array.from(raw).map((b) => b.toString(16).padStart(2, "0")).join("");
    } else if (typeof raw?.asBytes === "function") {
      const bytes = raw.asBytes();
      mmrRoot = "0x" + Array.from(bytes).map((b: number) => b.toString(16).padStart(2, "0")).join("");
    } else {
      mmrRoot = String(raw);
    }

    return {
      mmrRoot,
      startSeq: BigInt(snapshot.start_seq ?? 0),
      leafCount: BigInt(snapshot.leaf_count ?? 0),
      checkpointBlock: Number(snapshot.checkpoint_block ?? 0),
    };
  }

  async getCheckpointDuty(bucketId: bigint): Promise<CheckpointDuty | null> {
    const providerUrl = await this.getProviderUrl(bucketId);
    const response = await fetch(
      `${providerUrl}/checkpoint/duty?bucket_id=${Number(bucketId)}`
    );

    if (!response.ok) {
      if (response.status === 404) return null;
      throw new Error(`Checkpoint duty failed: ${response.status}`);
    }

    return response.json();
  }

  async triggerCheckpoint(bucketId: bigint): Promise<void> {
    const providerUrl = await this.getProviderUrl(bucketId);
    const response = await fetch(
      `${providerUrl}/checkpoint/trigger?bucket_id=${Number(bucketId)}`,
      { method: "POST" }
    );

    if (!response.ok) {
      throw new Error(`Checkpoint trigger failed: ${response.status} ${await response.text()}`);
    }
  }
}

// Singleton
let driveClient: DriveClient | null = null;

export function getDriveClient(chainWs: string): DriveClient {
  if (!driveClient || driveClient["chainWs"] !== chainWs) {
    driveClient = new DriveClient(chainWs);
  }
  return driveClient;
}
