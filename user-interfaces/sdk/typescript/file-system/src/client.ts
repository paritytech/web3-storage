/**
 * File System Client SDK
 *
 * High-level TypeScript client for the Web3 Storage File System Interface.
 */

import { createClient, PolkadotClient } from "polkadot-api";
import { getWsProvider } from "polkadot-api/ws-provider";
import { getPolkadotSigner } from "polkadot-api/signer";
import { Binary } from "@polkadot-api/substrate-bindings";
import { Keyring } from "@polkadot/keyring";
import { cryptoWaitReady, blake2AsU8a } from "@polkadot/util-crypto";
import { parachain } from "@polkadot-api/descriptors";

import type {
  FileSystemConfig,
  DriveInfo,
  CreateDriveOptions,
  DirectoryEntry,
  UploadOptions,
  UploadResult,
  DownloadResult,
  CommitStrategy,
} from "./types.js";

/**
 * File System Client
 *
 * Provides a high-level interface for file and directory operations
 * on the Web3 Storage decentralized storage network.
 *
 * @example
 * ```typescript
 * const client = new FileSystemClient({
 *   chainWs: "ws://127.0.0.1:2222",
 *   providerUrl: "http://127.0.0.1:3333",
 * });
 *
 * await client.connect();
 * await client.setSigner("//Alice");
 *
 * const driveId = await client.createDrive({ capacity: 1_000_000_000n, duration: 500, maxPayment: 1_000_000_000_000n });
 * await client.createDirectory(driveId, "/documents");
 * await client.uploadFile(driveId, "/documents/hello.txt", new TextEncoder().encode("Hello!"));
 * ```
 */
export class FileSystemClient {
  private config: FileSystemConfig;
  private client: PolkadotClient | null = null;
  private api: ReturnType<PolkadotClient["getTypedApi"]> | null = null;
  private signer: ReturnType<typeof getPolkadotSigner> | null = null;
  private signerAddress: string | null = null;

  constructor(config: FileSystemConfig) {
    this.config = config;
  }

  /**
   * Connect to the blockchain
   */
  async connect(): Promise<void> {
    await cryptoWaitReady();
    this.client = createClient(getWsProvider(this.config.chainWs));
    this.api = this.client.getTypedApi(parachain);
  }

  /**
   * Set the signer for transactions
   * @param seed - Seed phrase or dev account (e.g., "//Alice")
   */
  async setSigner(seed: string): Promise<void> {
    const keyring = new Keyring({ type: "sr25519" });
    const account = keyring.addFromUri(seed);
    this.signer = getPolkadotSigner(account.publicKey, "Sr25519", (input) =>
      account.sign(input)
    );
    this.signerAddress = account.address;
  }

  /**
   * Get the current signer's address
   */
  getAddress(): string {
    if (!this.signerAddress) {
      throw new Error("Signer not set. Call setSigner() first.");
    }
    return this.signerAddress;
  }

  /**
   * Disconnect from the blockchain
   */
  disconnect(): void {
    if (this.client) {
      this.client.destroy();
      this.client = null;
      this.api = null;
    }
  }

  /**
   * Create a new drive
   */
  async createDrive(options: CreateDriveOptions): Promise<bigint> {
    this.ensureConnected();

    const result = await this.api!.tx.DriveRegistry.create_drive({
      name: options.name
        ? Binary.fromBytes(new TextEncoder().encode(options.name))
        : undefined,
      max_bytes: options.capacity,
      duration: options.duration,
      max_payment: options.maxPayment,
      min_providers: options.minProviders ?? 1,
    }).signAndSubmit(this.signer!);

    // Extract drive ID from events
    const events = this.api!.event.DriveRegistry.DriveCreated.filter(result.events);
    if (events.length === 0) {
      throw new Error("DriveCreated event not found");
    }
    return events[0].drive_id;
  }

  /**
   * Get drive information
   */
  async getDrive(driveId: bigint): Promise<DriveInfo | null> {
    this.ensureConnected();

    const drive = await this.api!.query.DriveRegistry.Drives.getValue(driveId);
    if (!drive) return null;

    return {
      driveId,
      owner: drive.owner,
      name: drive.name ? new TextDecoder().decode(drive.name.asBytes()) : null,
      bucketId: drive.bucket_id,
      rootCid: drive.root_cid ? this.toHex(drive.root_cid.asBytes()) : null,
      createdAt: drive.created_at,
      updatedAt: drive.updated_at,
    };
  }

  /**
   * Get the Layer 0 bucket ID associated with a drive
   */
  async getBucketId(driveId: bigint): Promise<bigint> {
    const drive = await this.getDrive(driveId);
    if (!drive) {
      throw new Error(`Drive ${driveId} not found`);
    }
    return drive.bucketId;
  }

  /**
   * List all drives owned by the current user
   */
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

  /**
   * Create a directory
   */
  async createDirectory(
    driveId: bigint,
    path: string,
    bucketId?: bigint
  ): Promise<void> {
    this.ensureConnected();

    const bucket = bucketId ?? (await this.getBucketId(driveId));

    // For now, directories are tracked client-side via the root CID tree
    // This is a simplified implementation - full implementation would
    // update the on-chain root CID with the new directory structure
    console.log(`Creating directory ${path} in drive ${driveId} (bucket ${bucket})`);

    // In a full implementation, we would:
    // 1. Fetch current directory tree from provider
    // 2. Add new directory entry
    // 3. Upload updated tree
    // 4. Update root CID on chain
  }

  /**
   * Upload a file
   */
  async uploadFile(
    driveId: bigint,
    path: string,
    data: Uint8Array,
    options?: UploadOptions
  ): Promise<UploadResult> {
    const bucketId = await this.getBucketId(driveId);

    // Calculate content hash (CID)
    const hash = blake2AsU8a(data);
    const cid = this.toHex(hash);

    // Upload to provider
    const response = await fetch(`${this.config.providerUrl}/node`, {
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
    const commitResponse = await fetch(`${this.config.providerUrl}/commit`, {
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

  /**
   * Download a file
   */
  async downloadFile(driveId: bigint, path: string): Promise<DownloadResult> {
    // In a full implementation, we would:
    // 1. Look up the file's CID from the directory tree
    // 2. Download by CID

    // For now, this is a placeholder that requires knowing the CID
    throw new Error(
      "downloadFile requires path-to-CID resolution. Use downloadByCid() instead."
    );
  }

  /**
   * Download content by CID
   */
  async downloadByCid(bucketId: bigint, cid: string): Promise<Uint8Array> {
    const response = await fetch(
      `${this.config.providerUrl}/node?hash=${cid}&bucket_id=${bucketId}`
    );

    if (!response.ok) {
      throw new Error(`Download failed: ${response.status}`);
    }

    const json = await response.json();
    return this.fromBase64(json.data);
  }

  /**
   * List directory contents
   */
  async listDirectory(driveId: bigint, path: string): Promise<DirectoryEntry[]> {
    // In a full implementation, we would:
    // 1. Fetch the directory tree from the root CID
    // 2. Navigate to the specified path
    // 3. Return the entries

    // Placeholder implementation
    console.log(`Listing directory ${path} in drive ${driveId}`);
    return [];
  }

  /**
   * Delete a drive
   */
  async deleteDrive(driveId: bigint): Promise<void> {
    this.ensureConnected();

    await this.api!.tx.DriveRegistry.delete_drive({
      drive_id: driveId,
    }).signAndSubmit(this.signer!);
  }

  /**
   * Clear drive contents (reset to empty)
   */
  async clearDrive(driveId: bigint): Promise<void> {
    this.ensureConnected();

    await this.api!.tx.DriveRegistry.clear_drive({
      drive_id: driveId,
    }).signAndSubmit(this.signer!);
  }

  // --- Helper methods ---

  private ensureConnected(): void {
    if (!this.api) {
      throw new Error("Not connected. Call connect() first.");
    }
    if (!this.signer) {
      throw new Error("Signer not set. Call setSigner() first.");
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
    return Buffer.from(bytes).toString("base64");
  }

  private fromBase64(str: string): Uint8Array {
    return new Uint8Array(Buffer.from(str, "base64"));
  }
}
