/**
 * Storage SDK - Browser-compatible wrapper for S3 operations
 * Uses real chain types via polkadot-api
 */

import { createClient, type PolkadotClient, type TypedApi } from "polkadot-api";
import { getWsProvider } from "polkadot-api/ws";
import { getPolkadotSigner } from "polkadot-api/signer";
import { parachain } from "@polkadot-api/descriptors";
import { Binary, Enum } from "polkadot-api";
import {
  makeSigner,
  parseMultiaddrToUrl,
  READ_OPTS,
  submitTx,
  type ChainSigner,
} from "@web3-storage/sdk";
import { S3Client as SdkS3Client } from "@web3-storage/sdk/s3";
import { EncryptionKey } from "./encryption";
import { type Keypair, toHex } from "./crypto";

// Transaction result from best-block watching
interface TxResult {
  blockHash: string;
  blockNumber: number;
  events: any[];
}

// Types
export interface BucketInfo {
  s3BucketId: bigint;
  name: string;
  layer0BucketId: bigint;
  owner: string;
  createdAt: bigint;
}

export interface UploadResult {
  cid: string;
  size: number;
}

export interface CreateBucketOptions {
  capacity: bigint;
  duration: number;
  maxPayment: bigint;
}

export interface CheckpointConfig {
  interval: number;
  gracePeriod: number;
  enabled: boolean;
}

export interface CheckpointSignatureInfo {
  bucketId: number;
  mmrRoot: string;
  startSeq: number;
  leafCount: number;
  providerSignature: string;
}

export interface CheckpointStatus {
  config: CheckpointConfig;
  lastWindow: bigint;
  currentWindow: bigint;
  poolBalance: bigint;
  pendingRewards: bigint;
  snapshot: {
    mmrRoot: string;
    startSeq: bigint;
    leafCount: bigint;
    checkpointBlock: number;
  } | null;
}

export interface BucketMember {
  account: string;
  role: 'Admin' | 'Writer' | 'Reader';
}

export interface ProviderEndpointInfo {
  account: string;
  endpoint: string;
  healthy: boolean;
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

export interface PutObjectOptions {
  contentType?: string;
  metadata?: Record<string, string>;
}

export interface S3ObjectInfo {
  key: string;
  size: number;
  lastModified: number;
  etag: string;
}

type ParachainApi = TypedApi<typeof parachain>;

/**
 * Storage Client for browser-based operations
 * Uses real chain types for pallet interactions
 */
export class StorageClient {
  private chainWs: string;
  private client: PolkadotClient | null = null;
  private api: ParachainApi | null = null;
  private signer: ReturnType<typeof getPolkadotSigner> | null = null;
  private signerAddress: string | null = null;
  private keypair: Keypair | null = null;
  private chainSigner: ChainSigner | null = null;
  private s3c: SdkS3Client | null = null;
  /** Dev-chain fallback URLs (per bucket) when on-chain resolution fails. */
  private fallbackUrlCache: Map<string, string> = new Map();
  private encryptionKey: EncryptionKey | null = null;

  constructor(chainWs: string) {
    this.chainWs = chainWs;
  }

  async connect(): Promise<void> {
    console.log("[StorageClient] Connecting to chain:", this.chainWs);
    this.client = createClient(getWsProvider(this.chainWs));
    console.log("[StorageClient] Client created, getting typed API with parachain descriptor...");
    try {
      this.api = this.client.getTypedApi(parachain);
      // Dev chains: pin the local provider URL. The provider stores data
      // regardless of on-chain agreements, and on a dev chain the registered
      // multiaddr points at the same localhost endpoint anyway.
      const devOverride =
        this.chainWs.includes("127.0.0.1") || this.chainWs.includes("localhost")
          ? "http://127.0.0.1:3333"
          : undefined;
      this.s3c = new SdkS3Client({
        api: this.api,
        signer: this.chainSigner,
        providerUrl: devOverride,
      });
      console.log("[StorageClient] Typed API ready");
    } catch (err) {
      console.error("[StorageClient] Failed to get typed API (descriptor mismatch?):", err);
      throw err;
    }
  }

  async setSigner(seed: string): Promise<string> {
    // One derivation source for everything: the sdk signer carries the raw
    // keypair (provider auth) alongside the PolkadotSigner (extrinsics).
    this.chainSigner = makeSigner(seed);
    this.keypair = this.chainSigner.keypair ?? null;
    this.signer = this.chainSigner.signer;
    this.signerAddress = this.chainSigner.address;
    this.s3c?.setSigner(this.chainSigner);
    return this.signerAddress;
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

  /** Enable client-side encryption with the given key. */
  setEncryptionKey(key: EncryptionKey): void {
    this.encryptionKey = key;
  }

  /** Disable client-side encryption. */
  clearEncryptionKey(): void {
    this.encryptionKey = null;
  }

  /** Returns true if client-side encryption is enabled. */
  isEncryptionEnabled(): boolean {
    return this.encryptionKey !== null;
  }

  private ensureConnected(): void {
    if (!this.api) throw new Error("Not connected. Call connect() first.");
    if (!this.signer) throw new Error("Signer not set. Call setSigner() first.");
  }

  private requireS3(): SdkS3Client {
    if (!this.s3c) throw new Error("Not connected. Call connect() first.");
    return this.s3c;
  }

  /**
   * Produce an Authorization header for provider-node requests.
   * Format: Web3Storage <pubkey_hex>:<signature_hex>:<timestamp>
   * Signed message: web3storage:<method>:<bucketId>:<timestamp>
   */
  private signRequest(method: string, bucketId: bigint): Record<string, string> {
    if (!this.keypair) return {};
    const timestamp = Math.floor(Date.now() / 1000).toString();
    const message = `web3storage:${method}:${Number(bucketId)}:${timestamp}`;
    const msgBytes = new TextEncoder().encode(message);
    const sig = this.keypair.sign(msgBytes);
    const pubHex = toHex(this.keypair.publicKey);
    const sigHex = toHex(sig);
    return { Authorization: `Web3Storage ${pubHex}:${sigHex}:${timestamp}` };
  }

  /**
   * Submit a transaction via the sdk and resolve on best-block inclusion
   * (~2-6s) instead of finalization. Reads in this client target the best
   * head (READ_OPTS), so read-your-writes holds. Throws TxDispatchError on
   * chain-side failure; no stale-nonce auto-retry (user-visible retry is the
   * right UX). The old ExtrinsicFailed-event fallback for stale descriptors
   * is gone — CI now fails loudly when the tracked metadata drifts.
   */
  private async submitAndWatchBestBlock(tx: any): Promise<TxResult> {
    const ev = (await submitTx(tx, this.signer!, {
      mode: "best",
      retryStale: 0,
      timeoutMs: 120_000,
      onStatus: null,
      label: "console-ui tx",
    })) as any;
    return {
      blockHash: ev.block.hash,
      blockNumber: ev.block.number,
      events: ev.events ?? [],
    };
  }

  // --- Provider Resolution ---

  /**
   * Get the provider HTTP URL for a bucket, with caching.
   * Retries a few times if the bucket has no providers yet (agreement pending acceptance).
   */
  async getProviderUrl(
    bucketId: bigint,
    onProgress?: (status: string, attempt: number, total: number) => void,
  ): Promise<string> {
    const key = bucketId.toString();
    const fallback = this.fallbackUrlCache.get(key);
    if (fallback) return fallback;

    // Resolve from on-chain bucket data via the sdk (cached inside the client).
    try {
      const url = await this.requireS3().getProviderUrl(bucketId);
      onProgress?.("Provider ready", 1, 1);
      return url;
    } catch {
      // Fall back to default local provider for dev chains.
      // The provider stores data regardless of on-chain agreements — agreements
      // are only needed for checkpoints/accountability, not for HTTP uploads.
      if (this.chainWs.includes("127.0.0.1") || this.chainWs.includes("localhost")) {
        const local = "http://127.0.0.1:3333";
        console.log(`[StorageClient] No on-chain provider for bucket ${bucketId}, using local fallback: ${local}`);
        this.fallbackUrlCache.set(key, local);
        return local;
      }
      throw new Error(`Bucket ${bucketId} has no primary providers and no fallback available`);
    }
  }

  /**
   * Wait for a bucket's provider to become available. watchValue-based: the
   * sdk replays current bucket state and emits on change — no poll interval,
   * no missed-acceptance window. Calls onProgress with elapsed ms so the UI
   * can show timing warnings.
   */
  async waitForProvider(
    bucketId: bigint,
    onProgress?: (status: string, elapsedMs: number, attempt: number) => void,
  ): Promise<string> {
    this.invalidateProviderCache(bucketId);

    const statusFor = (elapsedSec: number): string => {
      if (elapsedSec < 30) return "Waiting for provider to accept the agreement...";
      if (elapsedSec < 60) return "Provider is processing — this typically takes about a minute...";
      if (elapsedSec < 100) return "Still waiting for provider acceptance...";
      return "Taking longer than usual — provider may be busy or offline...";
    };

    onProgress?.(statusFor(0), 0, 1);
    const startTime = Date.now();
    let tick = 0;
    try {
      const url = await this.requireS3().waitForProvider(bucketId, {
        timeoutMs: 150_000,
        tickMs: 3_000,
        onTick: (elapsedMs) => {
          tick += 1;
          onProgress?.(statusFor(Math.round(elapsedMs / 1000)), elapsedMs, tick);
        },
      });
      onProgress?.("Provider accepted — ready to use", Date.now() - startTime, tick + 1);
      return url;
    } catch {
      throw new Error(
        `Provider did not accept the agreement after ${Math.round((Date.now() - startTime) / 1000)}s. ` +
        `The provider may be offline or not accepting new agreements.`
      );
    }
  }

  /** Clear cached provider URL for a bucket (e.g. after provider changes). */
  invalidateProviderCache(bucketId?: bigint): void {
    this.s3c?.invalidateProviderUrl(bucketId);
    if (bucketId !== undefined) {
      this.fallbackUrlCache.delete(bucketId.toString());
    } else {
      this.fallbackUrlCache.clear();
    }
  }

  // --- S3 Operations ---

  async createBucket(name: string, options: CreateBucketOptions): Promise<BucketInfo> {
    this.ensureConnected();
    this.validateBucketName(name);

    console.log("[StorageClient] createBucket:", name, options);

    // Create the S3 bucket (this also creates the Layer 0 bucket).
    // Provider agreement is requested separately after creation.
    const tx = this.api!.tx.S3Registry.create_s3_bucket({
      name: Binary.fromText(name),
      min_providers: 1,
    });

    const result = await this.submitAndWatchBestBlock(tx);

    // Extract bucket ID from events
    let s3BucketId: bigint | null = null;
    let layer0BucketId: bigint | null = null;
    for (const event of result.events) {
      if (event.type === "S3Registry" && event.value.type === "S3BucketCreated") {
        s3BucketId = event.value.value.s3_bucket_id;
        layer0BucketId = event.value.value.layer0_bucket_id;
        break;
      }
    }

    // Fallback: if events couldn't be decoded (stale descriptor), look up by name
    if (s3BucketId === null) {
      console.warn(
        "[StorageClient] S3BucketCreated event not found in tx events (",
        result.events.length, "events). Falling back to chain query."
      );
      const looked = await this.headBucket(name);
      if (looked) {
        return looked;
      }
      throw new Error(
        "S3BucketCreated event not found and bucket lookup failed. " +
        "The runtime descriptor may be stale — run: npx papi update"
      );
    }

    // Return bucket info from the event data
    return {
      s3BucketId,
      name,
      layer0BucketId: layer0BucketId ?? 0n,
      owner: this.signerAddress!,
      createdAt: BigInt(Date.now()),
    };
  }

  async listBuckets(): Promise<BucketInfo[]> {
    this.ensureConnected();

    const bucketIds = await this.api!.query.S3Registry.UserBuckets.getValue(
      this.signerAddress!,
      READ_OPTS,
    );

    if (!bucketIds) return [];

    const buckets: BucketInfo[] = [];
    for (const bucketId of bucketIds) {
      const bucket = await this.api!.query.S3Registry.S3Buckets.getValue(bucketId, READ_OPTS);
      if (bucket) {
        const bucketName = new TextDecoder().decode(bucket.name);

        buckets.push({
          s3BucketId: BigInt(bucketId),
          name: bucketName,
          layer0BucketId: BigInt(bucket.layer0_bucket_id),
          owner: bucket.owner,
          createdAt: BigInt(bucket.created_at),
        });
      }
    }
    return buckets;
  }

  async headBucket(name: string): Promise<BucketInfo | null> {
    this.ensureConnected();

    const bucketId = await this.api!.query.S3Registry.BucketNameToId.getValue(
      Binary.fromText(name),
      READ_OPTS,
    );

    if (bucketId === undefined) return null;

    const bucket = await this.api!.query.S3Registry.S3Buckets.getValue(bucketId, READ_OPTS);
    if (!bucket) return null;

    const bucketName = new TextDecoder().decode(bucket.name);

    return {
      s3BucketId: BigInt(bucketId),
      name: bucketName,
      layer0BucketId: BigInt(bucket.layer0_bucket_id),
      owner: bucket.owner,
      createdAt: BigInt(bucket.created_at),
    };
  }

  async deleteBucket(name: string): Promise<void> {
    this.ensureConnected();

    const bucketId = await this.api!.query.S3Registry.BucketNameToId.getValue(
      Binary.fromText(name),
      READ_OPTS,
    );

    if (bucketId === undefined) {
      throw new Error(`Bucket not found: ${name}`);
    }

    await this.submitAndWatchBestBlock(
      this.api!.tx.S3Registry.delete_s3_bucket({
        s3_bucket_id: bucketId,
      })
    );
  }

  async putObject(
    _bucketName: string,
    key: string,
    data: Uint8Array,
    bucketId: bigint,
    options?: PutObjectOptions
  ): Promise<UploadResult> {
    this.ensureConnected();

    // Encrypt data before upload if encryption is enabled; the sdk client
    // carries opaque bytes so the on-chain CID covers what the provider stores.
    const uploadData = this.encryptionKey
      ? await this.encryptionKey.encrypt(data)
      : data;

    const result = await this.requireS3().putObject(
      { layer0BucketId: bucketId },
      key,
      uploadData,
      { contentType: options?.contentType, metadata: options?.metadata },
    );
    return { cid: result.cid ?? "", size: data.length };
  }

  /**
   * Download an S3 object by key. Verifies the stored bytes against the
   * on-chain CID when `s3BucketId` is provided: single-chunk mismatches throw
   * CidMismatchError; multi-chunk payloads (or objects without on-chain
   * metadata) come back `verified: false`. Decryption happens after
   * verification — the CID covers what the provider stores.
   */
  async downloadS3Object(
    bucketId: bigint,
    key: string,
    s3BucketId?: bigint,
  ): Promise<{ blob: Blob; verified: boolean }> {
    const got = await this.requireS3().getObject(
      { layer0BucketId: bucketId, s3BucketId },
      key,
    );

    if (this.encryptionKey) {
      const decrypted = await this.encryptionKey.decrypt(got.data);
      return { blob: new Blob([decrypted as BlobPart]), verified: got.verified };
    }
    return { blob: new Blob([got.data as BlobPart]), verified: got.verified };
  }

  async listObjects(bucketId: bigint, prefix?: string): Promise<S3ObjectInfo[]> {
    const listed = await this.requireS3().listObjects({ layer0BucketId: bucketId }, prefix);
    return listed.map((o) => ({
      key: o.key,
      size: o.size,
      lastModified: o.lastModified ?? 0,
      etag: o.etag ?? "",
    }));
  }

  // --- S3 Additional Operations ---

  async deleteObject(bucketId: bigint, key: string): Promise<void> {
    await this.requireS3().deleteObject({ layer0BucketId: bucketId }, key);
  }

  // --- Checkpoint Operations ---

  async getCheckpointConfig(bucketId: bigint): Promise<CheckpointConfig> {
    if (!this.api) throw new Error("Not connected. Call connect() first.");

    try {
      const config = await this.api.query.StorageProvider.CheckpointConfigs.getValue(bucketId, READ_OPTS);
      if (config) {
        return {
          interval: config.interval,
          gracePeriod: config.grace_period,
          enabled: config.enabled,
        };
      }
    } catch (err) {
      console.warn("[StorageClient] Failed to read CheckpointConfigs, using defaults:", err);
    }
    return { interval: 100, gracePeriod: 20, enabled: true };
  }

  async getCheckpointStatus(bucketId: bigint, currentBlock: number): Promise<CheckpointStatus> {
    if (!this.api) throw new Error("Not connected. Call connect() first.");

    const config = await this.getCheckpointConfig(bucketId);
    const currentWindow = config.interval > 0
      ? BigInt(Math.floor(currentBlock / config.interval))
      : 0n;

    let lastWindow = 0n;
    try {
      const lw = await this.api.query.StorageProvider.LastCheckpointWindow.getValue(bucketId, READ_OPTS);
      if (lw !== undefined) lastWindow = BigInt(lw);
    } catch {
      // Storage item may not exist yet
    }

    let poolBalance = 0n;
    try {
      const pool = await this.api.query.StorageProvider.CheckpointPool.getValue(bucketId, READ_OPTS);
      if (pool !== undefined) poolBalance = BigInt(pool);
    } catch {
      // Storage item may not exist yet
    }

    let pendingRewards = 0n;
    try {
      if (this.signerAddress) {
        const rewards = await this.api.query.StorageProvider.CheckpointRewards.getValue(
          this.signerAddress, bucketId, READ_OPTS
        );
        if (rewards !== undefined) pendingRewards = BigInt(rewards);
      }
    } catch {
      // Storage item may not exist yet
    }

    let snapshot: CheckpointStatus["snapshot"] = null;
    try {
      const bucket = await this.api.query.StorageProvider.Buckets.getValue(bucketId, READ_OPTS);
      if (bucket?.snapshot) {
        const s = bucket.snapshot;
        const mmrHex = typeof s.mmr_root === "string" ? s.mmr_root : String(s.mmr_root);
        snapshot = {
          mmrRoot: mmrHex,
          startSeq: BigInt(s.start_seq),
          leafCount: BigInt(s.leaf_count),
          checkpointBlock: s.checkpoint_block,
        };
      }
    } catch {
      // Bucket may not have a snapshot yet
    }

    return { config, lastWindow, currentWindow, poolBalance, pendingRewards, snapshot };
  }

  async getCheckpointSignature(bucketId: bigint): Promise<CheckpointSignatureInfo> {
    const providerUrl = await this.getProviderUrl(bucketId);
    const response = await fetch(
      `${providerUrl}/checkpoint-signature?bucket_id=${Number(bucketId)}`
    );
    if (!response.ok) {
      throw new Error(`Failed to get checkpoint signature: ${response.status} ${await response.text()}`);
    }
    const data = await response.json();
    return {
      bucketId: data.bucket_id,
      mmrRoot: data.mmr_root,
      startSeq: data.start_seq,
      leafCount: data.leaf_count,
      providerSignature: data.provider_signature,
    };
  }

  /**
   * Trigger checkpoint submission via the provider node.
   *
   * Instead of building and submitting the on-chain extrinsic directly (which
   * requires the caller to be a provider), this tells the provider's checkpoint
   * coordinator to handle leader election, signature collection, and on-chain
   * submission using the provider's own signing key.
   */
  async submitCheckpointForBucket(bucketId: bigint, _currentBlock: number): Promise<void> {
    const providerUrl = await this.getProviderUrl(bucketId);

    const response = await fetch(
      `${providerUrl}/checkpoint/trigger?bucket_id=${Number(bucketId)}`,
      { method: "POST" },
    );

    if (!response.ok) {
      const text = await response.text();
      throw new Error(`Checkpoint trigger failed: ${response.status} ${text}`);
    }
  }

  async configureCheckpointWindow(
    bucketId: bigint,
    interval: number,
    gracePeriod: number,
    enabled: boolean
  ): Promise<void> {
    this.ensureConnected();

    const tx = this.api!.tx.StorageProvider.configure_checkpoint_window({
      bucket_id: bucketId,
      interval,
      grace_period: gracePeriod,
      enabled,
    });

    await this.submitAndWatchBestBlock(tx);
  }

  async fundCheckpointPool(bucketId: bigint, amount: bigint): Promise<void> {
    this.ensureConnected();

    const tx = this.api!.tx.StorageProvider.fund_checkpoint_pool({
      bucket_id: bucketId,
      amount,
    });

    await this.submitAndWatchBestBlock(tx);
  }

  async claimCheckpointRewards(bucketId: bigint): Promise<void> {
    this.ensureConnected();

    const tx = this.api!.tx.StorageProvider.claim_checkpoint_rewards({
      bucket_id: bucketId,
    });

    await this.submitAndWatchBestBlock(tx);
  }

  // --- Account & Provider Info ---

  async getBalance(address: string): Promise<{ free: bigint; reserved: bigint }> {
    if (!this.api) throw new Error("Not connected. Call connect() first.");

    const account = await this.api.query.System.Account.getValue(address, READ_OPTS);
    return {
      free: BigInt(account.data.free),
      reserved: BigInt(account.data.reserved),
    };
  }

  async getProviderInfo(): Promise<{ maxCapacity: bigint; committedBytes: bigint } | null> {
    if (!this.api) throw new Error("Not connected. Call connect() first.");

    try {
      const entries = await this.api.query.StorageProvider.Providers.getEntries();
      if (entries.length === 0) return null;

      const provider = entries[0].value;
      return {
        maxCapacity: BigInt(provider.settings.max_capacity),
        committedBytes: BigInt(provider.committed_bytes ?? 0),
      };
    } catch {
      return null;
    }
  }

  // --- Provider Health ---

  /**
   * Liveness probe for `${baseUrl}/health`. Tries CORS-mode first so a readable
   * non-2xx counts as unhealthy; on a CORS rejection, falls back to a no-cors
   * fetch — opaque, but resolves whenever the host answered. Guards against
   * duplicate-ACAO from a misconfigured proxy.
   */
  private async probeHealthy(baseUrl: string): Promise<boolean> {
    const target = `${baseUrl}/health`;
    try {
      const response = await fetch(target, { cache: "no-store" });
      return response.ok;
    } catch {
      try {
        await fetch(target, { mode: "no-cors", cache: "no-store" });
        return true;
      } catch {
        return false;
      }
    }
  }

  async checkProviderHealth(bucketId: bigint): Promise<boolean> {
    try {
      const providerUrl = await this.getProviderUrl(bucketId);
      return await this.probeHealthy(providerUrl);
    } catch {
      return false;
    }
  }

  /**
   * Download a file by path from the FS endpoint.
   * Returns a Blob suitable for browser download.
   */
  async downloadFile(bucketId: bigint, path: string): Promise<Blob> {
    const providerUrl = await this.getProviderUrl(bucketId);
    const response = await fetch(
      `${providerUrl}/fs/${Number(bucketId)}/file?path=${encodeURIComponent(path)}`,
      { headers: this.signRequest("GET", bucketId) },
    );
    if (!response.ok) {
      throw new Error(`Download failed: ${response.status}`);
    }
    return response.blob();
  }

  // --- Bucket Members & Permissions ---

  async getBucketMembers(bucketId: bigint): Promise<BucketMember[]> {
    if (!this.api) throw new Error("Not connected");

    const bucket = await this.api.query.StorageProvider.Buckets.getValue(bucketId, READ_OPTS);
    if (!bucket) throw new Error(`Bucket ${bucketId} not found`);

    const roleMap: Record<string, 'Admin' | 'Writer' | 'Reader'> = {
      Admin: 'Admin',
      Writer: 'Writer',
      Reader: 'Reader',
    };

    return (bucket.members ?? []).map((m: any) => ({
      account: m.account,
      role: roleMap[m.role?.type ?? m.role] ?? 'Reader',
    }));
  }

  async setMember(bucketId: bigint, account: string, role: 'Admin' | 'Writer' | 'Reader'): Promise<void> {
    this.ensureConnected();

    const roleEnum = Enum(role);
    const tx = this.api!.tx.StorageProvider.set_member({
      bucket_id: bucketId,
      member: account,
      role: roleEnum,
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

  async listAccessibleBucketIds(): Promise<bigint[]> {
    if (!this.api) throw new Error("Not connected");
    if (!this.signerAddress) throw new Error("Signer not set");

    try {
      // MemberBuckets is a new storage map — use dynamic access in case the
      // typed descriptor hasn't been regenerated yet.
      const storageProvider = this.api.query.StorageProvider as any;
      if (!storageProvider.MemberBuckets) return [];
      const bucketIds = await storageProvider.MemberBuckets.getValue(
        this.signerAddress,
        READ_OPTS,
      );
      if (!bucketIds) return [];
      return bucketIds.map((id: any) => BigInt(id));
    } catch {
      return [];
    }
  }

  async getBucketProviders(bucketId: bigint): Promise<ProviderEndpointInfo[]> {
    if (!this.api) throw new Error("Not connected");

    const bucket = await this.api.query.StorageProvider.Buckets.getValue(bucketId, READ_OPTS);
    if (!bucket) throw new Error(`Bucket ${bucketId} not found`);

    const providers: string[] = bucket.primary_providers ?? [];
    const results: ProviderEndpointInfo[] = [];

    for (const providerAccount of providers) {
      const provider = await this.api.query.StorageProvider.Providers.getValue(providerAccount, READ_OPTS);
      if (!provider) {
        results.push({ account: providerAccount, endpoint: "unknown", healthy: false });
        continue;
      }

      const multiaddrStr = new TextDecoder().decode(provider.multiaddr);

      const url = parseMultiaddrToUrl(multiaddrStr) ?? "unknown";
      const healthy = url !== "unknown" ? await this.probeHealthy(url) : false;

      results.push({ account: providerAccount, endpoint: url, healthy });
    }

    return results;
  }

  // --- Provider Discovery ---

  async listAvailableProviders(): Promise<AvailableProvider[]> {
    if (!this.api) throw new Error("Not connected. Call connect() first.");

    const entries = await this.api.query.StorageProvider.Providers.getEntries();
    const providers: AvailableProvider[] = [];

    for (const entry of entries) {
      const provider = entry.value;
      const account = entry.keyArgs[0] as string;
      const settings = provider.settings;

      // Decode multiaddr
      const multiaddrStr = new TextDecoder().decode(provider.multiaddr);

      const maxCapacity = BigInt(settings.max_capacity ?? 0);
      const committedBytes = BigInt(provider.committed_bytes ?? 0);
      const availableCapacity = maxCapacity > committedBytes ? maxCapacity - committedBytes : 0n;

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
        agreementsTotal: (provider.stats as any)?.agreements_total ?? 0,
      });
    }

    // Sort by available capacity descending
    providers.sort((a, b) => {
      if (b.availableCapacity > a.availableCapacity) return 1;
      if (b.availableCapacity < a.availableCapacity) return -1;
      return 0;
    });

    return providers;
  }

  async requestAgreementWithProvider(
    bucketId: bigint,
    providerAccount: string,
    maxBytes: bigint,
    duration: number,
    maxPayment: bigint,
  ): Promise<void> {
    this.ensureConnected();

    const tx = this.api!.tx.StorageProvider.request_primary_agreement({
      bucket_id: bucketId,
      provider: providerAccount,
      max_bytes: maxBytes,
      duration,
      max_payment: maxPayment,
    });

    await this.submitAndWatchBestBlock(tx);
  }

  // --- Helpers ---

  private validateBucketName(name: string): void {
    if (name.length < 3 || name.length > 63) {
      throw new Error("Bucket name must be 3-63 characters");
    }
    if (!/^[a-z0-9]/.test(name)) {
      throw new Error("Bucket name must start with lowercase letter or number");
    }
    if (!/[a-z0-9]$/.test(name)) {
      throw new Error("Bucket name must end with lowercase letter or number");
    }
    if (!/^[a-z0-9.-]+$/.test(name)) {
      throw new Error("Bucket name can only contain lowercase letters, numbers, hyphens, and dots");
    }
  }

}

// Singleton instance
let storageClient: StorageClient | null = null;

export function getStorageClient(chainWs: string): StorageClient {
  if (!storageClient || storageClient["chainWs"] !== chainWs) {
    storageClient = new StorageClient(chainWs);
  }
  return storageClient;
}
