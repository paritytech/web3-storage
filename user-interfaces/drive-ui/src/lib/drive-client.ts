/**
 * Drive Client — wraps DriveRegistry pallet + StorageProvider pallet + provider
 * HTTP endpoints. Stateless w.r.t. connection: takes a `ParachainApi` and a
 * `Signer` (both supplied by the state layer) per-instance, and exposes one
 * method per documented operation.
 */

import { Binary, Enum, type PolkadotSigner, type Transaction, type TxFinalizedPayload } from "polkadot-api";
import { parachain } from "@polkadot-api/descriptors";
import { type ParachainApi, getClient } from "@/state/chain.state";

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

/**
 * Parse a libp2p multiaddr string (e.g. `/ip4/127.0.0.1/tcp/3333`) into an
 * HTTP URL. Picks the FIRST matching host/port pair so multi-`/tcp/` addrs
 * with multiple host candidates resolve deterministically.
 */
function decodeName(name: Uint8Array | undefined): string | null {
  if (!name) return null;
  try {
    return new TextDecoder().decode(name);
  } catch {
    return null;
  }
}

export function parseMultiaddrToHttp(multiaddr: string): string | null {
  const parts = multiaddr.split("/").filter(Boolean);
  let host: string | null = null;
  let port: string | null = null;

  for (let i = 0; i < parts.length; i++) {
    const seg = parts[i];
    const next = parts[i + 1];
    if (!next) continue;

    if ((seg === "ip4" || seg === "ip6" || seg === "dns4" || seg === "dns6") && host === null) {
      host = seg.startsWith("ip6") ? `[${next}]` : next;
    }
    if (seg === "tcp" && port === null) {
      port = next;
    }
    if (host !== null && port !== null) break;
  }

  if (host && port) return `http://${host}:${port}`;
  return null;
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
   * Sign + submit + wait for finalization. Throws on chain-side failure or
   * signing/connection error.
   *
   * Resolves the next nonce via the legacy `system_accountNextIndex` JSON-RPC
   * method directly against the node, bypassing PAPI's chainHead. PAPI's
   * default and the runtime-API/storage variants both compute nonce from a
   * specific block on the client's locally-observed chainHead, which can lag
   * behind the chain's actual state when the page has just reloaded and a
   * different connection (e.g. tests' api setup) has been submitting
   * same-signer txs. `system_accountNextIndex` queries the node directly and
   * accounts for pending pool state → always returns the correct next nonce.
   */
  private async submit(tx: Transaction): Promise<TxFinalizedPayload> {
    const { signer, address } = this.requireSigner();
    const client = getClient();
    if (!client) throw new Error("Not connected to chain");
    // Resolve next nonce via the legacy `system_accountNextIndex` JSON-RPC
    // method, bypassing PAPI's chainHead. PAPI's defaults (and its
    // typed-API/storage variants) compute nonce from a block on the local
    // chainHead, which can lag behind the chain's actual state when the
    // page just reloaded and a different connection (e.g. test setup via
    // api helpers) has been submitting same-signer txs. This RPC queries
    // the node directly and accounts for pool state.
    const nonce = await client._request<number>("system_accountNextIndex", [address]);
    const result = await tx.signAndSubmit(signer, { nonce });
    if (!result.ok) {
      const err = JSON.stringify(result.dispatchError, (_k, v) =>
        typeof v === "bigint" ? v.toString() : v,
      );
      throw new Error(`Transaction failed on-chain: ${err}`);
    }
    return result;
  }

  // ── Provider resolution ───────────────────────────────────────────────────

  private async resolveProviderEndpoint(bucketId: bigint): Promise<string> {
    const api = this.requireApi();
    const bucket = await api.query.StorageProvider.Buckets.getValue(bucketId);
    if (!bucket) throw new Error(`Bucket ${bucketId} not found on chain`);

    const providers = bucket.primary_providers;
    if (providers.length === 0) {
      throw new Error(`Bucket ${bucketId} has no primary providers`);
    }

    for (const providerAccount of providers) {
      const provider = await api.query.StorageProvider.Providers.getValue(providerAccount);
      if (!provider) continue;

      const multiaddrStr = new TextDecoder().decode(provider.multiaddr);
      const url = parseMultiaddrToHttp(multiaddrStr);
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

  invalidateProviderUrl(bucketId: bigint): void {
    this.providerUrlCache.delete(bucketId.toString());
  }

  async waitForProvider(
    bucketId: bigint,
    onProgress?: (status: string, elapsedMs: number) => void,
  ): Promise<string> {
    this.invalidateProviderUrl(bucketId);

    const intervals = [
      0, 3000, 3000, 3000, 3000, 3000, 6000, 6000, 6000, 6000, 6000,
      10000, 10000, 10000, 10000, 10000, 10000, 10000, 10000, 10000, 10000,
    ];
    const startTime = Date.now();

    for (let i = 0; i < intervals.length; i++) {
      if (intervals[i] > 0) await sleep(intervals[i]);

      const elapsedMs = Date.now() - startTime;
      const elapsedSec = Math.round(elapsedMs / 1000);

      let status: string;
      if (elapsedSec < 15) status = "Checking if provider has accepted the agreement...";
      else if (elapsedSec < 45) status = "Provider is reviewing the agreement...";
      else if (elapsedSec < 90) status = "Still waiting for provider to accept...";
      else status = "Taking longer than usual — provider may be busy or offline...";

      onProgress?.(status, elapsedMs);

      try {
        const url = await this.resolveProviderEndpoint(bucketId);
        this.providerUrlCache.set(bucketId.toString(), url);
        onProgress?.("Provider accepted — ready to use", Date.now() - startTime);
        return url;
      } catch (err) {
        const retryable =
          err instanceof Error &&
          (err.message.includes("no primary providers") || err.message.includes("not found on chain"));
        if (!retryable) throw err;
        if (i === intervals.length - 1) {
          throw new Error(
            `Provider did not accept the agreement after ${Math.round(
              (Date.now() - startTime) / 1000,
            )}s. The provider may be offline or not accepting new agreements.`,
          );
        }
      }
    }
    throw new Error("Provider did not accept the agreement");
  }

  // ── Account ───────────────────────────────────────────────────────────────

  async getBalance(address: string): Promise<{ free: bigint; reserved: bigint }> {
    const api = this.requireApi();
    const account = await api.query.System.Account.getValue(address);
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

    const driveIds = await api.query.DriveRegistry.UserDrives.getValue(address);
    if (driveIds.length === 0) return [];

    const drives: DriveInfo[] = [];
    for (const driveId of driveIds) {
      const drive = await api.query.DriveRegistry.Drives.getValue(driveId);
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
    const drive = await api.query.DriveRegistry.Drives.getValue(driveId);
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
    const bucket = await api.query.StorageProvider.Buckets.getValue(bucketId);
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
    const bucket = await api.query.StorageProvider.Buckets.getValue(bucketId);
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
