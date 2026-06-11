/**
 * Drive Client — wraps DriveRegistry pallet + StorageProvider pallet + provider
 * HTTP endpoints. Stateless w.r.t. connection: takes a `ParachainApi` and a
 * `Signer` (both supplied by the state layer) per-instance, and exposes one
 * method per documented operation.
 */

import { Binary, Enum, type PolkadotSigner, type Transaction, type TxFinalizedPayload } from "polkadot-api";
import { parachain } from "@polkadot-api/descriptors";
import {
  READ_OPTS,
  resolveProviderEndpoint,
  submitTx,
  waitForPrimaryProvider,
} from "@web3-storage/sdk";
import type { ParachainApi } from "@/state/chain.state";

export type Signer = PolkadotSigner;

const HTTP_RETRY_ATTEMPTS = 3;
const HTTP_RETRY_BASE_MS = 250;

export interface DriveInfo {
  driveId: bigint;
  bucketId: bigint;
  owner: string;
  name: string | null;
  maxCapacity: bigint;
  // block-number fields are u32 on chain → number in PAPI's typed API
  createdAt: number;
  storagePeriod: number;
  expiresAt: number;
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

export interface UploadOptions {
  contentType?: string;
  signal?: AbortSignal;
}

function sleep(ms: number): Promise<void> {
  return new Promise((r) => setTimeout(r, ms));
}

function isAbortError(err: unknown): boolean {
  return (
    err instanceof DOMException &&
    (err.name === "AbortError" || err.code === DOMException.ABORT_ERR)
  );
}

function isRetryableHttpError(status: number | null): boolean {
  if (status === null) return true;
  return status >= 500 && status < 600;
}

async function httpFetch(
  url: string,
  init: RequestInit & { signal?: AbortSignal } = {},
): Promise<Response> {
  let lastError: unknown = null;
  for (let attempt = 0; attempt < HTTP_RETRY_ATTEMPTS; attempt++) {
    try {
      const res = await fetch(url, init);
      if (res.ok || !isRetryableHttpError(res.status)) return res;
      lastError = new Error(`HTTP ${res.status}: ${await res.text().catch(() => "")}`);
    } catch (err) {
      if (isAbortError(err)) throw err;
      lastError = err;
    }
    if (attempt < HTTP_RETRY_ATTEMPTS - 1) {
      await sleep(HTTP_RETRY_BASE_MS * Math.pow(2, attempt));
    }
  }
  throw lastError instanceof Error ? lastError : new Error("HTTP request failed");
}

function decodeName(name: unknown): string | null {
  if (name == null) return null;
  try {
    if (typeof name === "string") return name;
    // polkadot-api Binary has asText(); fall back to TextDecoder
    if (typeof (name as any).asText === "function") return (name as any).asText();
    return new TextDecoder().decode(name as Uint8Array);
  } catch {
    return null;
  }
}

export class DriveClient {
  private api: ParachainApi | null = null;
  private signer: Signer | null = null;
  private signerAddress: string | null = null;
  private providerUrlCache = new Map<string, string>();

  setApi(api: ParachainApi | null): void {
    if (api !== this.api) {
      this.providerUrlCache.clear();
    }
    this.api = api;
  }

  setSigner(signer: Signer | null, address: string | null): void {
    this.signer = signer;
    this.signerAddress = address;
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

  private requireSigner(): { signer: Signer; address: string } {
    if (!this.signer || !this.signerAddress) throw new Error("Signer not set");
    return { signer: this.signer, address: this.signerAddress };
  }

  // ── Tx submission ─────────────────────────────────────────────────────────

  /**
   * Sign + submit via the sdk, resolve at best-block inclusion (~2-6s vs
   * ~12-24s finalized — none of this client's follow-up operations reference
   * the tx by a block-height-derived id, and all reads in this file target
   * the best head, so read-your-writes holds). Throws on chain-side failure
   * or signing/connection error. No stale-nonce auto-retry: a user-visible
   * retry is the right UX in a wallet-driven app.
   */
  private async submit(tx: Transaction): Promise<TxFinalizedPayload> {
    const { signer } = this.requireSigner();
    const result = await submitTx(tx, signer, {
      mode: "best",
      retryStale: 0,
      onStatus: null,
      label: "drive-ui tx",
    });
    return result as TxFinalizedPayload;
  }

  // ── Provider resolution ───────────────────────────────────────────────────

  async getProviderUrl(bucketId: bigint): Promise<string> {
    const key = bucketId.toString();
    const cached = this.providerUrlCache.get(key);
    if (cached) return cached;
    const url = await resolveProviderEndpoint(this.requireApi(), bucketId);
    this.providerUrlCache.set(key, url);
    return url;
  }

  invalidateProviderUrl(bucketId: bigint): void {
    this.providerUrlCache.delete(bucketId.toString());
  }

  async waitForProvider(
    bucketId: bigint,
    onProgress?: (status: string, elapsedMs: number) => void,
  ): Promise<string> {
    this.invalidateProviderUrl(bucketId);
    const api = this.requireApi();

    const statusFor = (elapsedSec: number): string => {
      if (elapsedSec < 15) return "Checking if provider has accepted the agreement...";
      if (elapsedSec < 45) return "Provider is reviewing the agreement...";
      if (elapsedSec < 90) return "Still waiting for provider to accept...";
      return "Taking longer than usual — provider may be busy or offline...";
    };

    onProgress?.(statusFor(0), 0);
    const startTime = Date.now();
    try {
      // watchValue replays the current bucket state and emits on change — no
      // poll interval, no missed-acceptance window.
      await waitForPrimaryProvider(api, bucketId, {
        timeoutMs: 150_000,
        tickMs: 3_000,
        onTick: (elapsedMs) => onProgress?.(statusFor(Math.round(elapsedMs / 1000)), elapsedMs),
      });
    } catch {
      throw new Error(
        `Provider did not accept the agreement after ${Math.round(
          (Date.now() - startTime) / 1000,
        )}s. The provider may be offline or not accepting new agreements.`,
      );
    }

    const url = await resolveProviderEndpoint(api, bucketId);
    this.providerUrlCache.set(bucketId.toString(), url);
    onProgress?.("Provider accepted — ready to use", Date.now() - startTime);
    return url;
  }

  // ── Account ───────────────────────────────────────────────────────────────

  async getBalance(address: string): Promise<{ free: bigint; reserved: bigint }> {
    const api = this.requireApi();
    const account = await api.query.System.Account.getValue(address, READ_OPTS);
    return { free: account.data.free, reserved: account.data.reserved };
  }

  // ── Drive on-chain operations ─────────────────────────────────────────────

  async createDrive(options: CreateDriveOptions): Promise<DriveInfo> {
    const api = this.requireApi();
    const { address } = this.requireSigner();

    const nameArg = options.name ? Binary.fromText(options.name) : undefined;

    const tx = api.tx.DriveRegistry.create_drive({
      name: nameArg,
      max_capacity: options.maxCapacity,
      storage_period: options.storagePeriod,
      payment: options.payment,
      min_providers: options.minProviders ?? undefined,
    });

    const result = await this.submit(tx);

    const created = api.event.DriveRegistry.DriveCreated.filter(result.events);
    if (created.length === 0) {
      const drives = await this.listDrives();
      if (drives.length > 0) return drives[drives.length - 1];
      throw new Error(
        "DriveCreated event not found. The runtime descriptor may be stale — run: pnpm papi:generate",
      );
    }
    const { drive_id, bucket_id } = created[0].payload;

    return {
      driveId: drive_id,
      bucketId: bucket_id,
      owner: address,
      name: options.name ?? null,
      maxCapacity: options.maxCapacity,
      createdAt: 0,
      storagePeriod: options.storagePeriod,
      expiresAt: 0,
      payment: options.payment,
    };
  }

  async listDrives(): Promise<DriveInfo[]> {
    const api = this.requireApi();
    const { address } = this.requireSigner();

    const driveIds = await api.query.DriveRegistry.UserDrives.getValue(address, READ_OPTS);
    if (driveIds.length === 0) return [];

    const drives: DriveInfo[] = [];
    for (const driveId of driveIds) {
      const drive = await api.query.DriveRegistry.Drives.getValue(driveId, READ_OPTS);
      if (!drive) continue;
      drives.push({
        driveId,
        bucketId: drive.bucket_id,
        owner: drive.owner,
        name: decodeName(drive.name),
        maxCapacity: drive.max_capacity,
        createdAt: drive.created_at,
        storagePeriod: drive.storage_period,
        expiresAt: drive.expires_at,
        payment: drive.payment,
      });
    }
    return drives;
  }

  async getDrive(driveId: bigint): Promise<DriveInfo | null> {
    const api = this.requireApi();
    const drive = await api.query.DriveRegistry.Drives.getValue(driveId, READ_OPTS);
    if (!drive) return null;
    return {
      driveId,
      bucketId: drive.bucket_id,
      owner: drive.owner,
      name: decodeName(drive.name),
      maxCapacity: drive.max_capacity,
      createdAt: drive.created_at,
      storagePeriod: drive.storage_period,
      expiresAt: drive.expires_at,
      payment: drive.payment,
    };
  }

  async deleteDrive(driveId: bigint): Promise<void> {
    const api = this.requireApi();
    const tx = api.tx.DriveRegistry.delete_drive({ drive_id: driveId });
    await this.submit(tx);
  }

  // ── FS HTTP operations ────────────────────────────────────────────────────

  async listDirectory(bucketId: bigint, path: string): Promise<FsEntry[]> {
    const providerUrl = await this.getProviderUrl(bucketId);
    const params = new URLSearchParams({ path });
    const response = await httpFetch(
      `${providerUrl}/fs/${Number(bucketId)}/ls?${params.toString()}`,
    );

    if (!response.ok) {
      throw new Error(`List directory failed: ${response.status}`);
    }

    const result = await response.json();
    return (result.entries || []).map((e: { name: string; path: string; entry_type: string; size?: number; mtime?: number }) => ({
      name: e.name,
      path: e.path,
      entryType: e.entry_type as "file" | "directory",
      size: e.size ?? 0,
      mtime: (e.mtime ?? 0) * 1000,
    }));
  }

  async uploadFile(
    bucketId: bigint,
    path: string,
    data: Uint8Array,
    options: UploadOptions = {},
  ): Promise<void> {
    const providerUrl = await this.getProviderUrl(bucketId);
    const response = await httpFetch(
      `${providerUrl}/fs/${Number(bucketId)}/file?path=${encodeURIComponent(path)}`,
      {
        method: "PUT",
        headers: { "Content-Type": options.contentType || "application/octet-stream" },
        body: data,
        signal: options.signal,
      },
    );

    if (!response.ok) {
      throw new Error(`Upload failed: ${response.status} ${await response.text().catch(() => "")}`);
    }
  }

  async downloadFile(bucketId: bigint, path: string): Promise<Blob> {
    const providerUrl = await this.getProviderUrl(bucketId);
    const response = await httpFetch(
      `${providerUrl}/fs/${Number(bucketId)}/file?path=${encodeURIComponent(path)}`,
    );

    if (!response.ok) {
      throw new Error(`Download failed: ${response.status}`);
    }

    return response.blob();
  }

  async deleteFile(bucketId: bigint, path: string): Promise<void> {
    const providerUrl = await this.getProviderUrl(bucketId);
    const response = await httpFetch(
      `${providerUrl}/fs/${Number(bucketId)}/file?path=${encodeURIComponent(path)}`,
      { method: "DELETE" },
    );

    if (!response.ok) {
      throw new Error(`Delete failed: ${response.status} ${await response.text().catch(() => "")}`);
    }
  }

  async createDirectory(bucketId: bigint, path: string): Promise<void> {
    const providerUrl = await this.getProviderUrl(bucketId);
    const response = await httpFetch(
      `${providerUrl}/fs/${Number(bucketId)}/mkdir?path=${encodeURIComponent(path)}`,
      { method: "POST" },
    );

    if (!response.ok) {
      throw new Error(`Create directory failed: ${response.status} ${await response.text().catch(() => "")}`);
    }
  }

  // ── Members ───────────────────────────────────────────────────────────────

  async getBucketMembers(bucketId: bigint): Promise<BucketMember[]> {
    const api = this.requireApi();
    const bucket = await api.query.StorageProvider.Buckets.getValue(bucketId, READ_OPTS);
    if (!bucket) throw new Error(`Bucket ${bucketId} not found`);

    const roleMap: Record<string, MemberRole> = {
      Admin: "Admin",
      Writer: "Writer",
      Reader: "Reader",
    };

    return (bucket.members ?? []).map((m: { account: string; role: { type?: string } | string }) => {
      const roleType = typeof m.role === "string" ? m.role : m.role?.type ?? "Reader";
      return {
        account: m.account,
        role: roleMap[roleType] ?? "Reader",
      };
    });
  }

  async addMember(bucketId: bigint, account: string, role: MemberRole): Promise<void> {
    const api = this.requireApi();
    const tx = api.tx.StorageProvider.set_member({
      bucket_id: bucketId,
      member: account,
      role: Enum(role),
    });
    await this.submit(tx);
  }

  async removeMember(bucketId: bigint, account: string): Promise<void> {
    const api = this.requireApi();
    const tx = api.tx.StorageProvider.remove_member({
      bucket_id: bucketId,
      member: account,
    });
    await this.submit(tx);
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

  async getCheckpointDuty(bucketId: bigint): Promise<CheckpointDuty | null> {
    const providerUrl = await this.getProviderUrl(bucketId);
    const response = await httpFetch(
      `${providerUrl}/checkpoint/duty?bucket_id=${Number(bucketId)}`,
    );

    if (!response.ok) {
      if (response.status === 404) return null;
      throw new Error(`Checkpoint duty failed: ${response.status}`);
    }

    return response.json();
  }

  async triggerCheckpoint(bucketId: bigint): Promise<void> {
    const providerUrl = await this.getProviderUrl(bucketId);
    const response = await httpFetch(
      `${providerUrl}/checkpoint/trigger?bucket_id=${Number(bucketId)}`,
      { method: "POST" },
    );

    if (!response.ok) {
      throw new Error(`Checkpoint trigger failed: ${response.status} ${await response.text().catch(() => "")}`);
    }
  }
}

// Re-export descriptor for tests / consumers that need event types.
export { parachain };
