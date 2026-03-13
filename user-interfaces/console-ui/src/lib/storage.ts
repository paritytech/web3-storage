/**
 * Storage SDK - Browser-compatible wrapper for File System and S3 operations
 * Uses real chain types via polkadot-api
 */

import { createClient, type PolkadotClient, type TypedApi } from "polkadot-api";
import { getWsProvider } from "polkadot-api/ws-provider/web";
import { getPolkadotSigner } from "polkadot-api/signer";
import { Keyring } from "@polkadot/keyring";
import { cryptoWaitReady } from "@polkadot/util-crypto";
import { parachain } from "@polkadot-api/descriptors";
import { Binary, FixedSizeBinary, Enum } from "polkadot-api";

// Transaction result from best-block watching
interface TxResult {
  blockHash: string;
  blockNumber: number;
  events: any[];
}

// Types
export interface DriveInfo {
  driveId: bigint;
  owner: string;
  name: string | null;
  bucketId: bigint;
  createdAt: bigint;
}

export interface FsEntryInfo {
  name: string;
  path: string;
  entryType: "file" | "directory";
  size: number;
  mtime: number;
}

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

export interface CreateDriveOptions {
  name?: string;
  capacity: bigint;
  duration: number;
  maxPayment: bigint;
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
  private providerUrlCache: Map<string, string> = new Map();

  constructor(chainWs: string) {
    this.chainWs = chainWs;
  }

  async connect(): Promise<void> {
    console.log("[StorageClient] Connecting to chain:", this.chainWs);
    await cryptoWaitReady();
    this.client = createClient(getWsProvider(this.chainWs));
    console.log("[StorageClient] Client created, getting typed API with parachain descriptor...");
    try {
      this.api = this.client.getTypedApi(parachain);
      console.log("[StorageClient] Typed API ready");
    } catch (err) {
      console.error("[StorageClient] Failed to get typed API (descriptor mismatch?):", err);
      throw err;
    }
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

  /**
   * Submit a transaction and resolve on best-block inclusion (~6s)
   * instead of finalization (~12-24s). Matches the Bulletin Chain pattern.
   */
  private submitAndWatchBestBlock(tx: any): Promise<TxResult> {
    return new Promise((resolve, reject) => {
      let resolved = false;

      const handleEvent = (ev: any) => {
        console.log("[StorageClient] Tx event:", ev.type, ev);
        if (ev.type === "txBestBlocksState" && ev.found && !resolved) {
          resolved = true;
          subscription.unsubscribe();

          // polkadot-api may deliver events as undefined or empty if
          // the runtime metadata has changed and event decoding fails.
          const events = ev.events ?? [];
          console.log("[StorageClient] Tx included in block:", ev.block.number,
            "events:", events.length,
            "ok:", ev.ok ?? "unknown",
            "dispatchError:", ev.dispatchError ?? "none");
          for (const event of events) {
            console.log("[StorageClient]   Event:", event.type, event.value?.type, event.value?.value);
          }

          // Check for dispatch error (tx reverted on-chain)
          // Look for System.ExtrinsicFailed event which contains the error
          const failedEvent = events.find(
            (e: any) => e.type === "System" && e.value?.type === "ExtrinsicFailed"
          );
          if (failedEvent) {
            const dispatchError = failedEvent.value?.value?.dispatch_error ?? ev.dispatchError;
            const errorStr = dispatchError
              ? JSON.stringify(dispatchError, (_k, v) =>
                  typeof v === "bigint" ? v.toString() : v
                )
              : "unknown dispatch error";
            console.error("[StorageClient] ExtrinsicFailed:", errorStr);
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
        console.error("[StorageClient] Tx error:", err);
        console.error("[StorageClient] Tx error details:", JSON.stringify(err, null, 2));
        if (!resolved) {
          resolved = true;
          subscription?.unsubscribe();
          reject(err);
        }
      };

      console.log("[StorageClient] Signing and submitting tx...");
      const subscription = tx.signSubmitAndWatch(this.signer!).subscribe({
        next: handleEvent,
        error: handleError,
      });

      // Timeout after 2 minutes
      setTimeout(() => {
        if (!resolved) {
          resolved = true;
          subscription?.unsubscribe();
          reject(new Error("Transaction timed out"));
        }
      }, 120000);
    });
  }

  // --- Provider Resolution ---

  /**
   * Parse a multiaddr string like /ip4/127.0.0.1/tcp/3333 or /dns4/host/tcp/3333
   * into an HTTP URL.
   */
  private parseMultiaddrToHttp(multiaddr: string): string | null {
    const parts = multiaddr.split("/").filter(Boolean);
    let host: string | null = null;
    let port: string | null = null;

    for (let i = 0; i < parts.length; i++) {
      if ((parts[i] === "ip4" || parts[i] === "ip6" || parts[i] === "dns4" || parts[i] === "dns6") && parts[i + 1]) {
        host = parts[i + 1];
      }
      if (parts[i] === "tcp" && parts[i + 1]) {
        port = parts[i + 1];
      }
    }

    if (host && port) {
      return `http://${host}:${port}`;
    }
    return null;
  }

  /**
   * Resolve the HTTP endpoint for a bucket's primary provider by reading
   * on-chain bucket data and provider multiaddr.
   */
  private async resolveProviderEndpoint(bucketId: bigint): Promise<string> {
    if (!this.api) throw new Error("Not connected");

    const bucket = await this.api.query.StorageProvider.Buckets.getValue(bucketId);
    if (!bucket) throw new Error(`Bucket ${bucketId} not found on chain`);

    const providers: string[] = bucket.primary_providers ?? [];
    if (providers.length === 0) {
      throw new Error(`Bucket ${bucketId} has no primary providers`);
    }

    // Try each provider until we find one with a valid multiaddr
    for (const providerAccount of providers) {
      const provider = await this.api.query.StorageProvider.Providers.getValue(providerAccount);
      if (!provider) continue;

      // multiaddr is a BoundedVec<u8> — decode to string
      let multiaddrStr: string;
      if (provider.multiaddr instanceof Uint8Array) {
        multiaddrStr = new TextDecoder().decode(provider.multiaddr);
      } else if (typeof provider.multiaddr.asBytes === "function") {
        multiaddrStr = new TextDecoder().decode(provider.multiaddr.asBytes());
      } else {
        multiaddrStr = String(provider.multiaddr);
      }

      console.log(`[StorageClient] Provider ${providerAccount} multiaddr raw:`, provider.multiaddr, `decoded: "${multiaddrStr}"`);
      const url = this.parseMultiaddrToHttp(multiaddrStr);
      console.log(`[StorageClient] Parsed URL:`, url);
      if (url) return url;
    }

    throw new Error(`Could not resolve HTTP endpoint for bucket ${bucketId} providers`);
  }

  /**
   * Get the provider HTTP URL for a bucket, with caching.
   * Retries a few times if the bucket has no providers yet (agreement pending acceptance).
   */
  async getProviderUrl(
    bucketId: bigint,
    onProgress?: (status: string, attempt: number, total: number) => void,
  ): Promise<string> {
    const key = bucketId.toString();
    const cached = this.providerUrlCache.get(key);
    if (cached) return cached;

    // Retry with backoff — provider auto-accepts agreements every ~6s
    const delays = [0, 2000, 4000, 6000];
    for (let i = 0; i < delays.length; i++) {
      if (delays[i] > 0) {
        const msg = `Waiting for provider to accept agreement...`;
        console.log(`[StorageClient] ${msg} (attempt ${i + 1}/${delays.length}, bucket ${bucketId})`);
        onProgress?.(msg, i + 1, delays.length);
        await new Promise(r => setTimeout(r, delays[i]));
      }
      try {
        const url = await this.resolveProviderEndpoint(bucketId);
        this.providerUrlCache.set(key, url);
        onProgress?.("Provider ready", delays.length, delays.length);
        return url;
      } catch (err) {
        if (i === delays.length - 1) throw err;
        // Only retry on "no primary providers" — other errors are fatal
        if (err instanceof Error && !err.message.includes("no primary providers")) throw err;
      }
    }
    throw new Error(`Bucket ${bucketId} has no primary providers after waiting`);
  }

  /**
   * Wait for a bucket's provider to become available.
   * Use after bucket/drive creation to ensure the provider has accepted the agreement.
   */
  async waitForProvider(
    bucketId: bigint,
    onProgress?: (status: string, attempt: number, total: number) => void,
  ): Promise<string> {
    // Invalidate cache so we do a fresh check
    this.invalidateProviderCache(bucketId);
    return this.getProviderUrl(bucketId, onProgress);
  }

  /** Clear cached provider URL for a bucket (e.g. after provider changes). */
  invalidateProviderCache(bucketId?: bigint): void {
    if (bucketId !== undefined) {
      this.providerUrlCache.delete(bucketId.toString());
    } else {
      this.providerUrlCache.clear();
    }
  }

  // --- File System (Drive) Operations ---

  async createDrive(options: CreateDriveOptions): Promise<bigint> {
    this.ensureConnected();

    // Single extrinsic: create_drive automatically creates Layer 0 bucket,
    // selects providers, and establishes storage agreements.
    const tx = this.api!.tx.DriveRegistry.create_drive({
      name: options.name ? Binary.fromText(options.name) : undefined,
      max_capacity: options.capacity,
      storage_period: options.duration,
      payment: options.maxPayment,
      min_providers: undefined, // Auto-determine
    });

    const result = await this.submitAndWatchBestBlock(tx);

    // Extract drive ID from DriveCreated event
    for (const event of result.events) {
      if (event.type === "DriveRegistry" && event.value.type === "DriveCreated") {
        return event.value.value.drive_id;
      }
    }

    // Fallback: if events couldn't be decoded, reload drives and return the latest
    console.warn(
      "[StorageClient] DriveCreated event not found in tx events (",
      result.events.length, "events). Falling back to chain query."
    );
    const drives = await this.listDrives();
    if (drives.length > 0) {
      // Return the most recently created drive
      const latest = drives.reduce((a, b) => a.createdAt > b.createdAt ? a : b);
      return latest.driveId;
    }
    throw new Error(
      "DriveCreated event not found and drive lookup failed. " +
      "The runtime descriptor may be stale — run: npx papi update"
    );
  }

  async listDrives(): Promise<DriveInfo[]> {
    this.ensureConnected();

    const driveIds = await this.api!.query.DriveRegistry.UserDrives.getValue(
      this.signerAddress!
    );

    if (!driveIds) return [];

    const drives: DriveInfo[] = [];
    for (const driveId of driveIds) {
      const drive = await this.getDrive(driveId);
      if (drive) drives.push(drive);
    }
    return drives;
  }

  async getDrive(driveId: bigint): Promise<DriveInfo | null> {
    this.ensureConnected();

    const drive = await this.api!.query.DriveRegistry.Drives.getValue(driveId);
    if (!drive) return null;

    // Handle name - it may be an Option<BoundedVec<u8>> or Binary
    let name: string | null = null;
    if (drive.name) {
      if (typeof drive.name.asBytes === 'function') {
        name = new TextDecoder().decode(drive.name.asBytes());
      } else if (drive.name instanceof Uint8Array) {
        name = new TextDecoder().decode(drive.name);
      }
    }

    return {
      driveId,
      owner: drive.owner,
      name,
      bucketId: BigInt(drive.bucket_id),
      createdAt: BigInt(drive.created_at),
    };
  }

  async deleteDrive(driveId: bigint): Promise<void> {
    this.ensureConnected();

    await this.submitAndWatchBestBlock(
      this.api!.tx.DriveRegistry.delete_drive({
        drive_id: driveId,
      })
    );
  }

  async uploadToDrive(
    _driveId: bigint,
    bucketId: bigint,
    path: string,
    data: Uint8Array
  ): Promise<UploadResult> {
    const providerUrl = await this.getProviderUrl(bucketId);
    // Upload via FS endpoint which handles chunking, Merkle tree,
    // MMR commit, and FS index update in a single request.
    const response = await fetch(
      `${providerUrl}/fs/${Number(bucketId)}/file?path=${encodeURIComponent(path)}`,
      {
        method: "PUT",
        headers: { "Content-Type": "application/octet-stream" },
        body: data,
      },
    );

    if (!response.ok) {
      throw new Error(`Upload failed: ${response.status} ${await response.text()}`);
    }

    const result = await response.json();
    return { cid: result.data_root, size: data.length };
  }

  async downloadByCid(bucketId: bigint, cid: string): Promise<Uint8Array> {
    const providerUrl = await this.getProviderUrl(bucketId);
    const response = await fetch(
      `${providerUrl}/node?hash=${cid}&bucket_id=${bucketId}`
    );

    if (!response.ok) {
      throw new Error(`Download failed: ${response.status}`);
    }

    const json = await response.json();
    return this.fromBase64(json.data);
  }

  async listDriveFiles(bucketId: bigint, path?: string): Promise<FsEntryInfo[]> {
    const providerUrl = await this.getProviderUrl(bucketId);
    const params = new URLSearchParams();
    if (path) params.set("path", path);

    const response = await fetch(
      `${providerUrl}/fs/${Number(bucketId)}/ls?${params.toString()}`
    );

    if (!response.ok) {
      throw new Error(`List drive files failed: ${response.status}`);
    }

    const result = await response.json();
    return (result.entries || []).map((e: any) => ({
      name: e.name,
      path: e.path,
      entryType: e.entry_type,
      size: e.size,
      mtime: e.mtime * 1000, // Unix seconds → JS millis
    }));
  }

  // --- S3 Operations ---

  async createBucket(name: string, options: CreateBucketOptions): Promise<BucketInfo> {
    this.ensureConnected();
    this.validateBucketName(name);

    console.log("[StorageClient] createBucket:", name, {
      capacity: options.capacity.toString(),
      duration: options.duration,
      maxPayment: options.maxPayment.toString(),
    });

    // S3 bucket creation with automatic provider matching
    // The pallet creates a Layer 0 bucket and requests agreements with providers
    const txEntry = this.api!.tx.S3Registry.create_s3_bucket;

    // Check compatibility level before attempting submission
    try {
      const level = await txEntry.getCompatibilityLevel();
      console.log("[StorageClient] S3Registry.create_s3_bucket compatibility level:", level);
    } catch (err) {
      console.warn("[StorageClient] Could not check compatibility level:", err);
    }

    const txArgs = {
      name: Binary.fromText(name),
      min_providers: 1, // 1 for local/testnet
      max_capacity: options.capacity,
      storage_period: options.duration,
      max_payment: options.maxPayment,
    };

    let tx;
    try {
      tx = txEntry(txArgs);
      console.log("[StorageClient] Tx built successfully");
    } catch (err) {
      console.error("[StorageClient] Failed to build S3Registry.create_s3_bucket tx:", err);
      console.error("[StorageClient] This usually means the runtime descriptor is out of date.");
      console.error("[StorageClient] Try: cd user-interfaces/console-ui && npx papi update");
      throw err;
    }

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
      this.signerAddress!
    );

    if (!bucketIds) return [];

    const buckets: BucketInfo[] = [];
    for (const bucketId of bucketIds) {
      const bucket = await this.api!.query.S3Registry.S3Buckets.getValue(bucketId);
      if (bucket) {
        // Handle name - Binary type in polkadot-api
        const bucketName = bucket.name.asText();

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
      Binary.fromText(name)
    );

    if (bucketId === undefined) return null;

    const bucket = await this.api!.query.S3Registry.S3Buckets.getValue(bucketId);
    if (!bucket) return null;

    // Handle name - Binary type in polkadot-api
    const bucketName = bucket.name.asText();

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
      Binary.fromText(name)
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
    this.validateObjectKey(key);

    const providerUrl = await this.getProviderUrl(bucketId);
    // Upload via S3 endpoint which handles chunking, Merkle tree, MMR commit,
    // and S3 index update in a single request.
    const headers: Record<string, string> = {
      "Content-Type": options?.contentType || "application/octet-stream",
    };
    if (options?.metadata) {
      for (const [k, v] of Object.entries(options.metadata)) {
        headers[`x-amz-meta-${k}`] = v;
      }
    }

    const response = await fetch(
      `${providerUrl}/s3/${Number(bucketId)}/object?key=${encodeURIComponent(key)}`,
      { method: "PUT", headers, body: data },
    );

    if (!response.ok) {
      throw new Error(`Upload failed: ${response.status} ${await response.text()}`);
    }

    const result = await response.json();
    return { cid: result.data_root || result.etag, size: data.length };
  }

  async getObject(bucketId: bigint, cid: string): Promise<Uint8Array> {
    return this.downloadByCid(bucketId, cid);
  }

  async listObjects(bucketId: bigint, prefix?: string): Promise<S3ObjectInfo[]> {
    const providerUrl = await this.getProviderUrl(bucketId);
    const params = new URLSearchParams();
    if (prefix) params.set("prefix", prefix);

    const response = await fetch(
      `${providerUrl}/s3/${Number(bucketId)}/objects?${params.toString()}`
    );

    if (!response.ok) {
      throw new Error(`List objects failed: ${response.status}`);
    }

    const result = await response.json();
    return (result.contents || []).map((obj: any) => ({
      key: obj.key,
      size: obj.size,
      lastModified: obj.last_modified * 1000, // Unix seconds → JS millis
      etag: obj.etag,
    }));
  }

  // --- File System Additional Operations ---

  async createDirectory(bucketId: bigint, path: string): Promise<void> {
    const providerUrl = await this.getProviderUrl(bucketId);
    const response = await fetch(
      `${providerUrl}/fs/${Number(bucketId)}/mkdir?path=${encodeURIComponent(path)}`,
      { method: "POST" },
    );

    if (!response.ok) {
      throw new Error(`Create directory failed: ${response.status} ${await response.text()}`);
    }
  }

  async deleteFile(bucketId: bigint, path: string): Promise<void> {
    const providerUrl = await this.getProviderUrl(bucketId);
    const response = await fetch(
      `${providerUrl}/fs/${Number(bucketId)}/file?path=${encodeURIComponent(path)}`,
      { method: "DELETE" },
    );

    if (!response.ok) {
      throw new Error(`Delete file failed: ${response.status} ${await response.text()}`);
    }
  }

  // --- S3 Additional Operations ---

  async deleteObject(bucketId: bigint, key: string): Promise<void> {
    const providerUrl = await this.getProviderUrl(bucketId);
    const response = await fetch(
      `${providerUrl}/s3/${Number(bucketId)}/object?key=${encodeURIComponent(key)}`,
      { method: "DELETE" },
    );

    if (!response.ok) {
      throw new Error(`Delete object failed: ${response.status} ${await response.text()}`);
    }
  }

  // --- Checkpoint Operations ---

  async getCheckpointConfig(bucketId: bigint): Promise<CheckpointConfig> {
    if (!this.api) throw new Error("Not connected. Call connect() first.");

    try {
      const config = await this.api.query.StorageProvider.CheckpointConfigs.getValue(bucketId);
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
      const lw = await this.api.query.StorageProvider.LastCheckpointWindow.getValue(bucketId);
      if (lw !== undefined) lastWindow = BigInt(lw);
    } catch {
      // Storage item may not exist yet
    }

    let poolBalance = 0n;
    try {
      const pool = await this.api.query.StorageProvider.CheckpointPool.getValue(bucketId);
      if (pool !== undefined) poolBalance = BigInt(pool);
    } catch {
      // Storage item may not exist yet
    }

    let pendingRewards = 0n;
    try {
      if (this.signerAddress) {
        const rewards = await this.api.query.StorageProvider.CheckpointRewards.getValue(
          bucketId, this.signerAddress
        );
        if (rewards !== undefined) pendingRewards = BigInt(rewards);
      }
    } catch {
      // Storage item may not exist yet
    }

    let snapshot: CheckpointStatus["snapshot"] = null;
    try {
      const bucket = await this.api.query.StorageProvider.Buckets.getValue(bucketId);
      if (bucket?.snapshot) {
        const s = bucket.snapshot;
        const mmrBytes = s.mmr_root instanceof Uint8Array
          ? s.mmr_root
          : (s.mmr_root as any).asBytes?.() ?? new Uint8Array(32);
        const mmrHex = "0x" + Array.from(mmrBytes as Uint8Array).map((b) => b.toString(16).padStart(2, "0")).join("");
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

  async submitCheckpointForBucket(bucketId: bigint, currentBlock: number): Promise<void> {
    this.ensureConnected();

    // 1. Get config interval
    const config = await this.getCheckpointConfig(bucketId);
    const window = BigInt(Math.floor(currentBlock / config.interval));

    // 2. Fetch signature from provider
    const sigInfo = await this.getCheckpointSignature(bucketId);

    // 3. Get primary providers for this bucket
    const bucket = await this.api!.query.StorageProvider.Buckets.getValue(bucketId);
    if (!bucket) throw new Error("Bucket not found on chain");

    const primaryProviders: string[] = bucket.primary_providers ?? [];
    if (primaryProviders.length === 0) {
      throw new Error("No primary providers found for bucket");
    }

    // 4. Build the MMR root FixedSizeBinary<32>
    const mmrRootHex = sigInfo.mmrRoot.startsWith("0x") ? sigInfo.mmrRoot : `0x${sigInfo.mmrRoot}`;
    const mmrRoot = FixedSizeBinary.fromHex(mmrRootHex) as FixedSizeBinary<32>;

    // 5. Build signature tuple: [providerAccount, MultiSignature::Sr25519(sig)]
    const sigHex = sigInfo.providerSignature.startsWith("0x")
      ? sigInfo.providerSignature
      : `0x${sigInfo.providerSignature}`;
    const providerAccount = primaryProviders[0];
    const signature = Enum("Sr25519", FixedSizeBinary.fromHex(sigHex) as FixedSizeBinary<64>);
    const signatures: Array<[string, typeof signature]> = [[providerAccount, signature]];

    // 6. Submit extrinsic
    const tx = this.api!.tx.StorageProvider.provider_checkpoint({
      bucket_id: bucketId,
      mmr_root: mmrRoot,
      start_seq: BigInt(sigInfo.startSeq),
      leaf_count: BigInt(sigInfo.leafCount),
      window,
      signatures,
    });

    await this.submitAndWatchBestBlock(tx);
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

    const account = await this.api.query.System.Account.getValue(address);
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

  async checkProviderHealth(bucketId: bigint): Promise<boolean> {
    try {
      const providerUrl = await this.getProviderUrl(bucketId);
      const response = await fetch(`${providerUrl}/health`);
      return response.ok;
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
      `${providerUrl}/fs/${Number(bucketId)}/file?path=${encodeURIComponent(path)}`
    );
    if (!response.ok) {
      throw new Error(`Download failed: ${response.status}`);
    }
    return response.blob();
  }

  // --- Bucket Members & Permissions ---

  async getBucketMembers(bucketId: bigint): Promise<BucketMember[]> {
    if (!this.api) throw new Error("Not connected");

    const bucket = await this.api.query.StorageProvider.Buckets.getValue(bucketId);
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
        this.signerAddress
      );
      if (!bucketIds) return [];
      return bucketIds.map((id: any) => BigInt(id));
    } catch {
      return [];
    }
  }

  async getBucketProviders(bucketId: bigint): Promise<ProviderEndpointInfo[]> {
    if (!this.api) throw new Error("Not connected");

    const bucket = await this.api.query.StorageProvider.Buckets.getValue(bucketId);
    if (!bucket) throw new Error(`Bucket ${bucketId} not found`);

    const providers: string[] = bucket.primary_providers ?? [];
    const results: ProviderEndpointInfo[] = [];

    for (const providerAccount of providers) {
      const provider = await this.api.query.StorageProvider.Providers.getValue(providerAccount);
      if (!provider) {
        results.push({ account: providerAccount, endpoint: "unknown", healthy: false });
        continue;
      }

      let multiaddrStr: string;
      if (provider.multiaddr instanceof Uint8Array) {
        multiaddrStr = new TextDecoder().decode(provider.multiaddr);
      } else if (typeof provider.multiaddr.asBytes === "function") {
        multiaddrStr = new TextDecoder().decode(provider.multiaddr.asBytes());
      } else {
        multiaddrStr = String(provider.multiaddr);
      }

      const url = this.parseMultiaddrToHttp(multiaddrStr) ?? "unknown";
      let healthy = false;
      if (url !== "unknown") {
        try {
          const resp = await fetch(`${url}/health`);
          healthy = resp.ok;
        } catch {
          // provider unreachable
        }
      }

      results.push({ account: providerAccount, endpoint: url, healthy });
    }

    return results;
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

  private validateObjectKey(key: string): void {
    if (key.length === 0 || key.length > 1024) {
      throw new Error("Object key must be 1-1024 characters");
    }
  }

  private fromBase64(str: string): Uint8Array {
    // Browser-compatible base64 decoding
    const binary = atob(str);
    const bytes = new Uint8Array(binary.length);
    for (let i = 0; i < binary.length; i++) {
      bytes[i] = binary.charCodeAt(i);
    }
    return bytes;
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
