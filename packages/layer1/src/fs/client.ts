// SPDX-License-Identifier: GPL-3.0-only

/**
 * FileSystemClient — drives (DriveRegistry) + the provider node's /fs HTTP
 * surface. Chain ops delegate to the layer-0 pallet wrappers (silent, no
 * auto-retry, finalized submission + finalized reads by default — UI-grade,
 * reorg-safe; tests/examples opt into in-block/best via readOpts/submitMode);
 * HTTP ops go through core's retrying fetch and are signed with the signer's
 * raw keypair, which the provider always requires.
 *
 * Verification: `downloadByCid` is verified (single chunk — its hash IS the
 * CID; the layer-0 downloadChunk throws CidMismatchError). Path-based
 * `downloadFile` is NOT verified: the provider's /fs file route returns no
 * data_root, and multi-chunk verification needs a Merkle DAG walk
 * (Rust-client parity) — tracked separately.
 */

import {
  httpFetch,
  signProviderRequest,
  type HttpFetchOpts,
} from "@web3-storage/core";
import {
  createDrive as createDriveTx,
  deleteDrive as deleteDriveTx,
  downloadChunk,
  removeMember as removeMemberTx,
  setMember as setMemberTx,
  shareDrive as shareDriveTx,
  unshareDrive as unshareDriveTx,
  type ChainSigner,
  type ParachainApi,
  type SubmitOpts,
  type TxStatusListener,
  type WaitOpts,
} from "@web3-storage/layer0";

import {
  ProviderUrlResolver,
  resolveBucketProviders,
  resolveCreationTerms,
} from "../provider-url.js";
import type {
  BucketMember,
  CheckpointDuty,
  CreateDriveOptions,
  DriveInfo,
  FsEntry,
  IndexRoot,
  MemberRole,
  UploadOptions,
  UploadResult,
} from "./types.js";

export interface FileSystemClientOptions {
  api: ParachainApi;
  signer?: ChainSigner | null;
  /** Explicit provider URL (dev/tests) — skips on-chain resolution. */
  providerUrl?: string;
  /** Injection point for unit tests. */
  fetch?: typeof fetch;
  /** Tx progress listener. Default null (silent) — apps drive their own UI. */
  onStatus?: TxStatusListener | null;
  /**
   * Read view for chain lookups. Defaults to "finalized" (UI-grade,
   * reorg-safe). Tests/examples pass READ_OPTS ({at: "best"}) to match their
   * in-block submission semantics.
   */
  readOpts?: { at: "best" | "finalized" };
  /**
   * Submission doneness. Defaults to "finalized" (UI-grade). Tests/examples
   * pass "best" for speed.
   */
  submitMode?: "best" | "finalized";
}

function decodeName(name: Uint8Array | string | undefined | null): string | null {
  if (name == null) return null;
  if (typeof name === "string") return name;
  try {
    return new TextDecoder().decode(name);
  } catch {
    return null;
  }
}

export class FileSystemClient {
  private readonly api: ParachainApi;
  private signer: ChainSigner | null;
  private readonly providers: ProviderUrlResolver;
  private readonly fetchOpts: HttpFetchOpts;
  private readonly onStatus: TxStatusListener | null;
  private readonly readOpts: { at: "best" | "finalized" };
  private readonly submitMode: "best" | "finalized";
  private readonly creationUrlOverride?: string;

  constructor(opts: FileSystemClientOptions) {
    this.api = opts.api;
    this.signer = opts.signer ?? null;
    this.readOpts = opts.readOpts ?? { at: "finalized" };
    this.submitMode = opts.submitMode ?? "finalized";
    this.providers = new ProviderUrlResolver(opts.api, opts.providerUrl, this.readOpts);
    this.creationUrlOverride = opts.providerUrl;
    this.fetchOpts = opts.fetch ? { fetchImpl: opts.fetch } : {};
    this.onStatus = opts.onStatus ?? null;
  }

  setSigner(signer: ChainSigner | null): void {
    this.signer = signer;
  }

  private requireSigner(): ChainSigner {
    if (!this.signer) throw new Error("Signer not set");
    return this.signer;
  }

  private submitOpts(): SubmitOpts {
    return { mode: this.submitMode, retryStale: 0, onStatus: this.onStatus };
  }

  private authHeaders(method: string, bucketId: bigint): Record<string, string> {
    const kp = this.signer?.keypair;
    return kp ? signProviderRequest(kp, method, bucketId) : {};
  }

  // ── Drive chain ops ─────────────────────────────────────────────────────

  /**
   * Create a drive via the negotiate -> establish flow: pick a provider
   * (explicit `options.provider` or auto-discovered), POST /negotiate for
   * signed terms (unless `options.signedTerms` is supplied), then redeem them
   * in create_drive — which opens the underlying bucket + agreement atomically.
   */
  async createDrive(options: CreateDriveOptions): Promise<{
    driveId: bigint;
    bucketId: bigint;
    provider: string;
  }> {
    const signer = this.requireSigner();
    const { provider, signedTerms } = await resolveCreationTerms(this.api, {
      owner: signer.address,
      maxBytes: options.maxCapacity,
      duration: options.storagePeriod,
      provider: options.provider,
      signedTerms: options.signedTerms,
      urlOverride: this.creationUrlOverride,
      readOpts: this.readOpts,
      fetchOpts: this.fetchOpts,
    });
    return createDriveTx(this.api, signer, options.name ?? "", provider, signedTerms, this.submitOpts());
  }

  async getDrive(driveId: bigint): Promise<DriveInfo | null> {
    const drive = await this.api.query.DriveRegistry.Drives.getValue(driveId, this.readOpts);
    if (!drive) return null;
    const [providerInfo] = await resolveBucketProviders(
      this.api,
      [drive.bucket_id],
      this.readOpts,
    );
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

  async listDrives(owner?: string): Promise<DriveInfo[]> {
    const who = owner ?? this.requireSigner().address;
    const driveIds = await this.api.query.DriveRegistry.UserDrives.getValue(who, this.readOpts);
    if (!driveIds || driveIds.length === 0) return [];

    // Batch all drive lookups into one storage query instead of N round-trips.
    const driveValues = await this.api.query.DriveRegistry.Drives.getValues(
      driveIds.map((id) => [id] as const),
      this.readOpts,
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
    const providersByBucket = await resolveBucketProviders(this.api, bucketIds, this.readOpts);
    drives.forEach((drive, i) => {
      drive.providerInfo = providersByBucket[i] ?? [];
    });

    return drives;
  }

  async deleteDrive(driveId: bigint): Promise<void> {
    await deleteDriveTx(this.api, this.requireSigner(), driveId, this.submitOpts());
  }

  async shareDrive(driveId: bigint, member: string, role: MemberRole): Promise<void> {
    await shareDriveTx(
      this.api,
      this.requireSigner(),
      driveId,
      { address: member },
      role,
      this.submitOpts(),
    );
  }

  async unshareDrive(driveId: bigint, member: string): Promise<void> {
    await unshareDriveTx(
      this.api,
      this.requireSigner(),
      driveId,
      { address: member },
      this.submitOpts(),
    );
  }

  async addMember(bucketId: bigint, account: string, role: MemberRole): Promise<void> {
    await setMemberTx(
      this.api,
      this.requireSigner(),
      bucketId,
      { address: account },
      role,
      this.submitOpts(),
    );
  }

  async removeMember(bucketId: bigint, account: string): Promise<void> {
    await removeMemberTx(
      this.api,
      this.requireSigner(),
      bucketId,
      { address: account },
      this.submitOpts(),
    );
  }

  async getBucketMembers(bucketId: bigint): Promise<BucketMember[]> {
    const bucket = await this.api.query.StorageProvider.Buckets.getValue(bucketId, this.readOpts);
    if (!bucket) throw new Error(`Bucket ${bucketId} not found`);
    return (bucket.members ?? []).map((m: { account: string; role: { type?: string } | string }) => {
      const roleType = typeof m.role === "string" ? m.role : (m.role?.type ?? "Reader");
      const role: MemberRole =
        roleType === "Admin" || roleType === "Writer" ? roleType : "Reader";
      return { account: m.account, role };
    });
  }

  // ── Provider resolution ─────────────────────────────────────────────────

  getProviderUrl(bucketId: bigint): Promise<string> {
    return this.providers.get(bucketId);
  }

  invalidateProviderUrl(bucketId?: bigint): void {
    this.providers.invalidate(bucketId);
  }

  waitForProvider(bucketId: bigint, opts?: WaitOpts): Promise<string> {
    return this.providers.waitForProvider(bucketId, opts);
  }

  // ── FS HTTP ops ─────────────────────────────────────────────────────────

  async listDirectory(
    bucketId: bigint,
    path: string,
    opts: { recursive?: boolean; signal?: AbortSignal } = {},
  ): Promise<FsEntry[]> {
    const providerUrl = await this.getProviderUrl(bucketId);
    const params = new URLSearchParams({ path });
    if (opts.recursive) params.set("recursive", "true");
    const response = await httpFetch(
      `${providerUrl}/fs/${bucketId}/ls?${params.toString()}`,
      { signal: opts.signal, headers: this.authHeaders("GET", bucketId) },
      this.fetchOpts,
    );
    if (!response.ok) throw new Error(`List directory failed: ${response.status}`);
    const result = await response.json();
    type WireEntry = { name: string; path: string; entry_type: string; size?: number; mtime?: number };
    return ((result.entries ?? []) as WireEntry[]).map((e) => ({
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
  ): Promise<UploadResult> {
    const providerUrl = await this.getProviderUrl(bucketId);
    const response = await httpFetch(
      `${providerUrl}/fs/${bucketId}/file?path=${encodeURIComponent(path)}`,
      {
        method: "PUT",
        headers: {
          "Content-Type": options.contentType || "application/octet-stream",
          ...this.authHeaders("PUT", bucketId),
        },
        body: data as BodyInit,
        signal: options.signal,
      },
      this.fetchOpts,
    );
    if (!response.ok) {
      throw new Error(`Upload failed: ${response.status} ${await response.text().catch(() => "")}`);
    }
    const body = await response.json().catch(() => ({}));
    return { dataRoot: body.data_root, size: data.length };
  }

  /**
   * Download a file by path. UNVERIFIED — the /fs file route returns no
   * data_root to check against; see the module docs.
   */
  async downloadFile(
    bucketId: bigint,
    path: string,
    opts: { signal?: AbortSignal } = {},
  ): Promise<Uint8Array> {
    const providerUrl = await this.getProviderUrl(bucketId);
    const response = await httpFetch(
      `${providerUrl}/fs/${bucketId}/file?path=${encodeURIComponent(path)}`,
      { signal: opts.signal, headers: this.authHeaders("GET", bucketId) },
      this.fetchOpts,
    );
    if (!response.ok) throw new Error(`Download failed: ${response.status}`);
    return new Uint8Array(await response.arrayBuffer());
  }

  /** Download a single chunk by CID — VERIFIED (throws CidMismatchError). */
  async downloadByCid(providerUrl: string, cid: string): Promise<Uint8Array> {
    return downloadChunk(providerUrl, cid);
  }

  async deleteFile(bucketId: bigint, path: string): Promise<void> {
    const providerUrl = await this.getProviderUrl(bucketId);
    const response = await httpFetch(
      `${providerUrl}/fs/${bucketId}/file?path=${encodeURIComponent(path)}`,
      { method: "DELETE", headers: this.authHeaders("DELETE", bucketId) },
      this.fetchOpts,
    );
    if (!response.ok) {
      throw new Error(`Delete failed: ${response.status} ${await response.text().catch(() => "")}`);
    }
  }

  async createDirectory(bucketId: bigint, path: string): Promise<void> {
    const providerUrl = await this.getProviderUrl(bucketId);
    const response = await httpFetch(
      `${providerUrl}/fs/${bucketId}/mkdir?path=${encodeURIComponent(path)}`,
      { method: "POST", headers: this.authHeaders("POST", bucketId) },
      this.fetchOpts,
    );
    if (!response.ok) {
      throw new Error(`Create directory failed: ${response.status} ${await response.text().catch(() => "")}`);
    }
  }

  /** The provider's own view of the drive's metadata Merkle root + counts. */
  async getIndexRoot(bucketId: bigint): Promise<IndexRoot> {
    const providerUrl = await this.getProviderUrl(bucketId);
    const response = await httpFetch(
      `${providerUrl}/fs/${bucketId}/index_root`,
      { headers: this.authHeaders("GET", bucketId) },
      this.fetchOpts,
    );
    if (!response.ok) throw new Error(`index_root failed: ${response.status}`);
    const body = await response.json();
    if (typeof body.metadata_merkle_root !== "string") {
      throw new Error("index_root response is missing metadata_merkle_root");
    }
    return {
      indexRoot: body.metadata_merkle_root,
      fileCount: Number(body.file_count ?? 0),
      dirCount: Number(body.dir_count ?? 0),
      totalSize: BigInt(body.total_size ?? 0),
    };
  }

  // ── Checkpoint (provider HTTP) ──────────────────────────────────────────

  async getCheckpointDuty(bucketId: bigint): Promise<CheckpointDuty | null> {
    const providerUrl = await this.getProviderUrl(bucketId);
    const response = await httpFetch(
      `${providerUrl}/checkpoint/duty?bucket_id=${bucketId}`,
      { headers: this.authHeaders("GET", bucketId) },
      this.fetchOpts,
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
      `${providerUrl}/checkpoint/trigger?bucket_id=${bucketId}`,
      { method: "POST", headers: this.authHeaders("POST", bucketId) },
      this.fetchOpts,
    );
    if (!response.ok) {
      throw new Error(`Checkpoint trigger failed: ${response.status} ${await response.text().catch(() => "")}`);
    }
  }
}
