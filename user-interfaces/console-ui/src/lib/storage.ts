/**
 * Storage SDK - Browser-compatible wrapper for File System and S3 operations
 */

import { createClient, type PolkadotClient } from "polkadot-api";
import { getWsProvider } from "polkadot-api/ws-provider/web";
import { getPolkadotSigner } from "polkadot-api/signer";
import { Keyring } from "@polkadot/keyring";
import { cryptoWaitReady, blake2AsU8a } from "@polkadot/util-crypto";

// Types
export interface DriveInfo {
  driveId: bigint;
  owner: string;
  name: string | null;
  bucketId: bigint;
  rootCid: string | null;
  createdAt: bigint;
  updatedAt: bigint;
}

export interface BucketInfo {
  s3BucketId: bigint;
  name: string;
  layer0BucketId: bigint;
  owner: string;
  createdAt: bigint;
  objectCount: bigint;
  totalSize: bigint;
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

/**
 * Storage Client for browser-based operations
 */
export class StorageClient {
  private chainWs: string;
  private providerUrl: string;
  private client: PolkadotClient | null = null;
  private signer: ReturnType<typeof getPolkadotSigner> | null = null;
  private signerAddress: string | null = null;

  constructor(chainWs: string, providerUrl: string) {
    this.chainWs = chainWs;
    this.providerUrl = providerUrl;
  }

  async connect(): Promise<void> {
    await cryptoWaitReady();
    this.client = createClient(getWsProvider(this.chainWs));
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
    }
  }

  isConnected(): boolean {
    return this.client !== null;
  }

  hasSigner(): boolean {
    return this.signer !== null;
  }

  // --- File System Operations ---

  async createDrive(options: CreateDriveOptions): Promise<bigint> {
    // For now, simulate drive creation since we need chain types
    // In production, this would call the DriveRegistry pallet
    console.log("Creating drive with options:", options);

    // Simulate by creating Layer 0 bucket first
    const bucketId = await this.createLayer0Bucket(options);

    // Return simulated drive ID
    return BigInt(Date.now());
  }

  async listDrives(): Promise<DriveInfo[]> {
    // Query drives from chain
    // For now, return empty - requires chain types
    return [];
  }

  async getDrive(driveId: bigint): Promise<DriveInfo | null> {
    // Query drive from chain
    return null;
  }

  async deleteDrive(driveId: bigint): Promise<void> {
    console.log("Deleting drive:", driveId);
  }

  async uploadToDrive(
    driveId: bigint,
    bucketId: bigint,
    path: string,
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
    this.validateBucketName(name);

    // Create Layer 0 bucket first
    const layer0BucketId = await this.createLayer0Bucket(options);

    // Return simulated bucket info
    return {
      s3BucketId: BigInt(Date.now()),
      name,
      layer0BucketId,
      owner: this.signerAddress || "",
      createdAt: BigInt(Date.now()),
      objectCount: 0n,
      totalSize: 0n,
    };
  }

  async listBuckets(): Promise<BucketInfo[]> {
    // Query buckets from chain
    return [];
  }

  async headBucket(name: string): Promise<BucketInfo | null> {
    return null;
  }

  async deleteBucket(name: string): Promise<void> {
    console.log("Deleting bucket:", name);
  }

  async putObject(
    bucketName: string,
    key: string,
    data: Uint8Array,
    bucketId: bigint,
    options?: PutObjectOptions
  ): Promise<UploadResult> {
    this.validateObjectKey(key);

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
    await fetch(`${this.providerUrl}/commit`, {
      method: "POST",
      headers: { "Content-Type": "application/json" },
      body: JSON.stringify({
        bucket_id: Number(bucketId),
        data_roots: [cid],
      }),
    });

    return { cid, size: data.length };
  }

  async getObject(bucketId: bigint, cid: string): Promise<Uint8Array> {
    return this.downloadByCid(bucketId, cid);
  }

  // --- Layer 0 Operations ---

  private async createLayer0Bucket(options: { capacity: bigint; duration: number; maxPayment: bigint }): Promise<bigint> {
    // This would interact with the storage-provider pallet
    // For now, simulate bucket creation
    console.log("Creating Layer 0 bucket with options:", options);
    return BigInt(Date.now());
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
