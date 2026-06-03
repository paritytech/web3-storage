/**
 * Drive Client — wraps DriveRegistry pallet + StorageProvider pallet + provider
 * HTTP endpoints. Stateless w.r.t. connection: takes a `ParachainApi` and a
 * `Signer` (both supplied by the state layer) per-instance, and exposes one
 * method per documented operation.
 */

import { Binary, Enum, type PolkadotSigner, type Transaction, type TxFinalizedPayload } from "polkadot-api";
import { parachain } from "@polkadot-api/descriptors";
import { resolveProviderEndpoint } from "@web3-storage/papi";
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
}

/**
 * Provider-signed agreement terms returned by `POST /negotiate` on the
 * provider node. The signature is the SCALE-encoded `MultiSignature` as
 * hex (e.g. `0x01<64-byte-sr25519-sig>`).
 */
export interface SignedTerms {
  terms: {
    owner: string;
    max_bytes: number | bigint;
    duration: number;
    price_per_byte: number | bigint;
    valid_until: number;
    nonce: number | bigint;
    replica_params: unknown | null;
  };
  signature: string;
}

export interface NegotiateRequest {
  owner: string;
  max_bytes: number | bigint;
  duration: number;
  price_per_byte: number | bigint;
  replica_params: unknown | null;
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
  /** Price per byte per block. Defaults to 0 if omitted. */
  pricePerByte?: bigint;
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

function decodeName(name: Uint8Array | undefined): string | null {
  if (!name) return null;
  try {
    return new TextDecoder().decode(name);
  } catch {
    return null;
  }
}

/**
 * POST a `NegotiateRequest` to the provider's `/negotiate` endpoint and
 * return the provider-signed terms bundle. Mirrors
 * `console-ui/src/lib/storage.ts::negotiateTerms`.
 */
export async function negotiateTerms(
  providerUrl: string,
  request: NegotiateRequest,
): Promise<SignedTerms> {
  const res = await fetch(`${providerUrl.replace(/\/$/, "")}/negotiate`, {
    method: "POST",
    headers: { "content-type": "application/json" },
    body: JSON.stringify(request, (_k, v) =>
      typeof v === "bigint" ? v.toString() : v,
    ),
  });
  if (!res.ok) {
    const body = await res.text().catch(() => "");
    throw new Error(`/negotiate failed: ${res.status} ${body}`);
  }
  return res.json();
}

// MultiSignature SCALE variant order from sp_runtime.
const MULTI_SIGNATURE_VARIANT: Record<number, string> = {
  0: "Ed25519",
  1: "Sr25519",
  2: "Ecdsa",
  3: "Eth",
};

function hexToBytes(hex: string): Uint8Array {
  const h = hex.startsWith("0x") ? hex.slice(2) : hex;
  const out = new Uint8Array(h.length / 2);
  for (let i = 0; i < out.length; i++) {
    out[i] = parseInt(h.substring(i * 2, i * 2 + 2), 16);
  }
  return out;
}

/**
 * Build the `{ provider, terms, sig }` args shared by every signed-terms
 * extrinsic.
 */
export function buildSignedTermsArgs(
  providerAccount: string,
  signed: SignedTerms,
) {
  const sigBytes = hexToBytes(signed.signature);
  if (sigBytes.length < 1) {
    throw new Error("signature too short to contain a MultiSignature variant byte");
  }
  const variantName = MULTI_SIGNATURE_VARIANT[sigBytes[0]];
  if (!variantName) {
    throw new Error(`unknown MultiSignature variant byte: ${sigBytes[0]}`);
  }
  const sigPayloadHex =
    "0x" +
    Array.from(sigBytes.slice(1))
      .map((b) => b.toString(16).padStart(2, "0"))
      .join("");
  // eslint-disable-next-line @typescript-eslint/no-explicit-any
  const sig = Enum(variantName as any, sigPayloadHex);

  const t = signed.terms;
  const terms = {
    owner: t.owner,
    max_bytes: BigInt(t.max_bytes),
    duration: t.duration,
    price_per_byte: BigInt(t.price_per_byte),
    valid_until: t.valid_until,
    nonce: BigInt(t.nonce),
    // eslint-disable-next-line @typescript-eslint/no-explicit-any
    replica_params: (t.replica_params ?? undefined) as any,
  };
  return { provider: providerAccount, terms, sig };
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
    const { signer } = this.requireSigner();
    const result = await tx.signAndSubmit(signer);
    if (!result.ok) {
      const err = JSON.stringify(result.dispatchError, (_k, v) =>
        typeof v === "bigint" ? v.toString() : v,
      );
      throw new Error(`Transaction failed on-chain: ${err}`);
    }
    return result;
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

  /**
   * Walk `StorageProvider.Providers` storage and return all registered
   * providers with their settings, sorted by free capacity descending.
   * Used by the provider picker to surface candidates before negotiation.
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

  // ── Account ───────────────────────────────────────────────────────────────

  async getBalance(address: string): Promise<{ free: bigint; reserved: bigint }> {
    const api = this.requireApi();
    const account = await api.query.System.Account.getValue(address);
    return { free: account.data.free, reserved: account.data.reserved };
  }

  // ── Drive on-chain operations ─────────────────────────────────────────────

  /**
   * Redeem provider-signed agreement terms on chain to open a Layer-0
   * bucket + primary agreement and register the drive on top — atomically
   * in one extrinsic.
   *
   * **Step 2 of drive creation.** Step 1 is the HTTP `negotiateTerms` call
   * against the chosen provider. Splitting the two lets a failed on-chain
   * submit be retried without re-negotiating (terms valid until
   * `terms.valid_until`).
   */
  async submitCreateDrive(
    name: string | undefined,
    providerAccount: string,
    providerUrl: string,
    signed: SignedTerms,
  ): Promise<DriveInfo> {
    const api = this.requireApi();
    const { address } = this.requireSigner();

    const tx = api.tx.DriveRegistry.create_drive({
      name: name ? Binary.fromText(name) : undefined,
      ...buildSignedTermsArgs(providerAccount, signed),
    });

    const result = await this.submit(tx);

    const created = api.event.DriveRegistry.DriveCreated.filter(result.events);
    if (created.length === 0) {
      const drives = await this.listDrives();
      if (drives.length > 0) {
        // Prime the provider URL cache so the first upload skips the on-chain lookup.
        this.providerUrlCache.set(drives[drives.length - 1].bucketId.toString(), providerUrl);
        return drives[drives.length - 1];
      }
      throw new Error(
        "DriveCreated event not found. The runtime descriptor may be stale — run: pnpm papi:generate",
      );
    }
    const { drive_id, bucket_id } = created[0].payload;

    // We already know the provider HTTP URL — prime the cache so the first
    // upload doesn't have to resolve it from chain.
    this.providerUrlCache.set(bucket_id.toString(), providerUrl);

    return {
      driveId: drive_id,
      bucketId: bucket_id,
      owner: address,
      name: name ?? null,
      maxCapacity: BigInt(signed.terms.max_bytes),
      createdAt: 0,
      storagePeriod: signed.terms.duration,
      expiresAt: 0,
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
