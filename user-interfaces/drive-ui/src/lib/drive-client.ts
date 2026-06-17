// SPDX-License-Identifier: GPL-3.0-only

/**
 * Drive Client — wraps DriveRegistry pallet + StorageProvider pallet + provider
 * HTTP endpoints. Stateless w.r.t. connection: takes a `ParachainApi` and a
 * `Signer` (both supplied by the state layer) per-instance, and exposes one
 * method per documented operation.
 */

import { Binary, Enum, type PolkadotSigner, type Transaction, type TxFinalizedPayload } from "polkadot-api";
import { parachain } from "@polkadot-api/descriptors";
import { parseMultiaddrToUrl, resolveProviderEndpoint, toSs58, type SignedTerms } from "@web3-storage/papi";

// Re-exported so state modules can keep importing it from the client facade.
export type { SignedTerms } from "@web3-storage/papi";
import type { ParachainApi } from "@/state/chain.state";

export type Signer = PolkadotSigner;

const HTTP_RETRY_ATTEMPTS = 3;
const HTTP_RETRY_BASE_MS = 250;

/** A primary provider backing a drive's underlying layer-0 bucket. */
export interface DriveProviderInfo {
  account: string;
  multiaddr: string;
  /** Resolved HTTP(S) base URL, or `null` if the multiaddr isn't an HTTP endpoint. */
  url: string | null;
}

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
  /** Primary providers of the underlying layer-0 bucket. */
  providerInfo: DriveProviderInfo[];
}


/**
 * Provider-signed agreement terms returned by `POST /negotiate` on the
 * provider node. The signature is the SCALE-encoded `MultiSignature` as
 * hex (e.g. `0x01<64-byte-sr25519-sig>`).
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

export interface MatchingProviders extends AvailableProvider {
  matchScore: number;
  partialReason: string;
  stake: bigint,
  committedBytes: bigint;
  minDuration: number;
  maxDuration: number;
  acceptingPrimary: boolean;
  replicaSyncPrice?: bigint;
  acceptingExtensions: boolean;
  registeredAt: number;
  agreementsTotal: number;
  agreementsExtended: number;
  agreementsNotExtended: number;
  agreementsBurned: number;
  challengesReceived: number;
  challengesFailed: number;
  maxCapacity: bigint;
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

export interface QueryMatchingProvidersParams {
  query: {
    bytesNeeded: bigint,
    minDuration: number,
    maxPricePerByte: bigint,
    primaryOnly: boolean,
  },
  limit: number;
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
    bucket_id: t.bucket_id ? BigInt(t.bucket_id) : undefined,
  };
  return { provider: providerAccount, terms, sig };
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

  async queryMatchingProviders(
    query: QueryMatchingProvidersParams['query'],
    limit: QueryMatchingProvidersParams['limit'],
  ): Promise<MatchingProviders[]> {
    const api = this.requireApi();
    const matches = await api.apis.StorageProviderApi.find_matching_providers({
      bytes_needed: query.bytesNeeded,
      min_duration: query.minDuration,
      max_price_per_byte: query.maxPricePerByte,
      primary_only: query.primaryOnly,
    }, limit);

    return matches.map((match) => {
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
    }).sort((a, b) => b.matchScore - a.matchScore);
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
      providerInfo: [{ account: providerAccount, multiaddr: "", url: providerUrl }],
    };
  }

  async listDrives(): Promise<DriveInfo[]> {
    const api = this.requireApi();
    const { address } = this.requireSigner();

    const driveIds = await api.query.DriveRegistry.UserDrives.getValue(address);
    if (driveIds.length === 0) return [];

    // Batch all drive lookups into one storage query instead of N round-trips.
    const driveValues = await api.query.DriveRegistry.Drives.getValues(
      driveIds.map((driveId) => [driveId] as const),
    );

    const drives: DriveInfo[] = [];
    const bucketIds: bigint[] = [];
    driveValues.forEach((drive, i) => {
      if (!drive) return;
      drives.push({
        driveId: driveIds[i]!,
        bucketId: drive.bucket_id,
        owner: drive.owner,
        name: decodeName(drive.name),
        maxCapacity: drive.max_capacity,
        createdAt: drive.created_at,
        storagePeriod: drive.storage_period,
        expiresAt: drive.expires_at,
        providerInfo: [],
      });
      bucketIds.push(drive.bucket_id);
    });

    // `providersByBucket[i]` aligns with `drives[i]` (both built skipping nulls).
    const providersByBucket = await this.resolveBucketProviders(bucketIds);
    drives.forEach((drive, i) => {
      drive.providerInfo = providersByBucket[i] ?? [];
    });

    return drives;
  }

  async getDrive(driveId: bigint): Promise<DriveInfo | null> {
    const api = this.requireApi();
    const drive = await api.query.DriveRegistry.Drives.getValue(driveId);
    if (!drive) return null;
    const [providerInfo] = await this.resolveBucketProviders([drive.bucket_id]);
    return {
      driveId,
      bucketId: drive.bucket_id,
      owner: drive.owner,
      name: decodeName(drive.name),
      maxCapacity: drive.max_capacity,
      createdAt: drive.created_at,
      storagePeriod: drive.storage_period,
      expiresAt: drive.expires_at,
      providerInfo: providerInfo ?? [],
    };
  }

  /**
   * Resolve the primary-provider info for each given layer-0 bucket id, batching
   * every storage read into a single query per pallet map (no per-bucket /
   * per-provider round-trips). Returns an array aligned with `bucketIds`.
   */
  private async resolveBucketProviders(
    bucketIds: bigint[],
  ): Promise<DriveProviderInfo[][]> {
    const api = this.requireApi();
    const bucketInfo = await api.query.StorageProvider.Buckets.getValues(
      bucketIds.map((id) => [id] as const),
    );

    const providerAccounts = [
      ...new Set(bucketInfo.flatMap((info) => info?.primary_providers ?? [])),
    ];
    const providerRecords = await api.query.StorageProvider.Providers.getValues(
      providerAccounts.map((account) => [account] as const),
    );

    const providerMap = new Map<string, DriveProviderInfo>();
    providerAccounts.forEach((account, i) => {
      const record = providerRecords[i];
      if (!record) return;
      const multiaddr = new TextDecoder().decode(record.multiaddr);
      providerMap.set(account, {
        account,
        multiaddr,
        url: parseMultiaddrToUrl(multiaddr),
      });
    });

    return bucketInfo.map((info) =>
      (info?.primary_providers ?? [])
        .map((account) => providerMap.get(account))
        .filter((p): p is DriveProviderInfo => p != null),
    );
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
        body: data as Uint8Array<ArrayBuffer>,
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
