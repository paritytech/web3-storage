/**
 * Storage SDK - Browser-compatible wrapper for File System and S3 operations
 * Uses real chain types via polkadot-api
 */

import { createClient, type PolkadotClient, type TypedApi } from "polkadot-api";
import { getWsProvider } from "polkadot-api/ws-provider/web";
import { getPolkadotSigner } from "polkadot-api/signer";
import { Keyring } from "@polkadot/keyring";
import { cryptoWaitReady, blake2AsU8a } from "@polkadot/util-crypto";
import { parachain } from "@polkadot-api/descriptors";
import { Binary } from "polkadot-api";

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
  rootCid: string | null;
  createdAt: bigint;
  lastCommittedAt: bigint;
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
  private providerUrl: string;
  private client: PolkadotClient | null = null;
  private api: ParachainApi | null = null;
  private signer: ReturnType<typeof getPolkadotSigner> | null = null;
  private signerAddress: string | null = null;

  constructor(chainWs: string, providerUrl: string) {
    this.chainWs = chainWs;
    this.providerUrl = providerUrl;
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
          console.log("[StorageClient] Tx included in block:", ev.block.number,
            "events:", ev.events?.length ?? 0);
          if (ev.events) {
            for (const event of ev.events) {
              console.log("[StorageClient]   Event:", event.type, event.value?.type, event.value?.value);
            }
          }
          resolve({
            blockHash: ev.block.hash,
            blockNumber: ev.block.number,
            events: ev.events || [],
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

  // --- File System (Drive) Operations ---

  async createDrive(options: CreateDriveOptions): Promise<bigint> {
    this.ensureConnected();

    // Step 1: Create a Layer 0 bucket with storage via StorageProvider pallet
    const bucketTx = this.api!.tx.StorageProvider.create_bucket_with_storage({
      max_bytes: options.capacity,
      duration: options.duration,
      max_price_per_byte: options.maxPayment,
    });

    const bucketResult = await this.submitAndWatchBestBlock(bucketTx);

    // Extract bucket ID from events
    let bucketId: bigint | null = null;
    for (const event of bucketResult.events) {
      if (event.type === "StorageProvider" && event.value.type === "BucketCreated") {
        bucketId = event.value.value.bucket_id;
        break;
      }
    }

    if (bucketId === null) {
      throw new Error("BucketCreated event not found - bucket creation failed");
    }

    // Step 2: Create a drive on that bucket via DriveRegistry pallet
    // Using empty root CID (32 zero bytes) for new drive
    const emptyRootCid = Binary.fromBytes(new Uint8Array(32));

    const driveTx = this.api!.tx.DriveRegistry.create_drive_on_bucket({
      bucket_id: bucketId,
      root_cid: emptyRootCid,
      name: options.name ? Binary.fromText(options.name) : undefined,
    });

    const driveResult = await this.submitAndWatchBestBlock(driveTx);

    // Extract drive ID from events
    for (const event of driveResult.events) {
      if (event.type === "DriveRegistry" && event.value.type === "DriveCreatedOnBucket") {
        return event.value.value.drive_id;
      }
      if (event.type === "DriveRegistry" && event.value.type === "DriveCreated") {
        return event.value.value.drive_id;
      }
    }

    throw new Error("DriveCreated event not found in transaction result");
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

    // Handle root_cid - it's a FixedSizeBinary<32> in polkadot-api
    let rootCid: string | null = null;
    if (drive.root_cid) {
      // FixedSizeBinary has asBytes() method
      const cidBytes = drive.root_cid.asBytes();
      // Check if it's all zeros (empty CID)
      const isZero = Array.from(cidBytes).every(b => b === 0);
      rootCid = isZero ? null : this.toHex(cidBytes);
    }

    return {
      driveId,
      owner: drive.owner,
      name,
      bucketId: BigInt(drive.bucket_id),
      rootCid,
      createdAt: BigInt(drive.created_at),
      lastCommittedAt: BigInt(drive.last_committed_at),
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
    _path: string,
    data: Uint8Array
  ): Promise<UploadResult> {
    const hash = blake2AsU8a(data);
    const cid = this.toHex(hash);

    // Upload to provider
    const response = await fetch(`${this.providerUrl}/node`, {
      method: "PUT",
      headers: { "Content-Type": "application/json" },
      body: JSON.stringify({
        bucket_id: Number(bucketId),
        hash: cid,
        data: this.toBase64(data),
        children: null,
      }),
    });

    if (!response.ok) {
      throw new Error(`Upload failed: ${response.status} ${await response.text()}`);
    }

    // Commit to MMR
    const commitResponse = await fetch(`${this.providerUrl}/commit`, {
      method: "POST",
      headers: { "Content-Type": "application/json" },
      body: JSON.stringify({
        bucket_id: Number(bucketId),
        data_roots: [cid],
      }),
    });

    if (!commitResponse.ok) {
      throw new Error(`Commit failed: ${commitResponse.status}`);
    }

    return { cid, size: data.length };
  }

  async downloadByCid(bucketId: bigint, cid: string): Promise<Uint8Array> {
    const response = await fetch(
      `${this.providerUrl}/node?hash=${cid}&bucket_id=${bucketId}`
    );

    if (!response.ok) {
      throw new Error(`Download failed: ${response.status}`);
    }

    const json = await response.json();
    return this.fromBase64(json.data);
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

    if (s3BucketId === null) {
      throw new Error("S3BucketCreated event not found in transaction result");
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
      `${this.providerUrl}/s3/${Number(bucketId)}/object?key=${encodeURIComponent(key)}`,
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
    const params = new URLSearchParams();
    if (prefix) params.set("prefix", prefix);

    const response = await fetch(
      `${this.providerUrl}/s3/${Number(bucketId)}/objects?${params.toString()}`
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

  // --- Provider Health ---

  async checkProviderHealth(): Promise<boolean> {
    try {
      const response = await fetch(`${this.providerUrl}/health`);
      return response.ok;
    } catch {
      return false;
    }
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

  private toHex(bytes: Uint8Array): string {
    return (
      "0x" +
      Array.from(bytes)
        .map((b) => b.toString(16).padStart(2, "0"))
        .join("")
    );
  }

  private toBase64(bytes: Uint8Array): string {
    // Browser-compatible base64 encoding
    let binary = "";
    for (let i = 0; i < bytes.length; i++) {
      binary += String.fromCharCode(bytes[i]);
    }
    return btoa(binary);
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

export function getStorageClient(chainWs: string, providerUrl: string): StorageClient {
  if (!storageClient || storageClient["chainWs"] !== chainWs || storageClient["providerUrl"] !== providerUrl) {
    storageClient = new StorageClient(chainWs, providerUrl);
  }
  return storageClient;
}
