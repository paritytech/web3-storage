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
import { cryptoWaitReady } from "@polkadot/util-crypto";
import { parachain } from "@polkadot-api/descriptors";

import type {
  FileSystemConfig,
  DriveInfo,
  CreateDriveOptions,
  DirectoryEntry,
  DirectoryListing,
  IndexRoot,
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
        ? Binary.fromText(options.name)
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
      name: drive.name ? new TextDecoder().decode(drive.name) : null,
      bucketId: drive.bucket_id,
      rootCid: drive.root_cid ? this.toHex(drive.root_cid) : null,
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
  ): Promise<{ path: string; created: boolean }> {
    const bucket = bucketId ?? (await this.getBucketId(driveId));

    const url = `${this.config.providerUrl}/fs/${bucket}/mkdir?path=${encodeURIComponent(path)}`;
    const response = await fetch(url, { method: "POST" });

    if (!response.ok) {
      throw new Error(
        `Failed to create directory: ${response.status} ${await response.text()}`
      );
    }

    return (await response.json()) as { path: string; created: boolean };
  }

  /**
   * Upload a file
   *
   * Uses the high-level `/fs/` provider endpoint which handles chunking,
   * tree-building, MMR commit, and index update in a single request.
   */
  async uploadFile(
    driveId: bigint,
    path: string,
    data: Uint8Array,
    options?: UploadOptions
  ): Promise<UploadResult> {
    const bucketId = await this.getBucketId(driveId);

    const contentType = options?.contentType || "application/octet-stream";
    const url = `${this.config.providerUrl}/fs/${bucketId}/file?path=${encodeURIComponent(path)}`;
    const response = await fetch(url, {
      method: "PUT",
      headers: { "Content-Type": contentType },
      body: data,
    });

    if (!response.ok) {
      throw new Error(`Upload failed: ${response.status} ${await response.text()}`);
    }

    const result = (await response.json()) as Record<string, any>;
    return {
      cid: result.data_root,
      size: result.size ?? data.length,
    };
  }

  /**
   * Download a file by path
   */
  async downloadFile(driveId: bigint, path: string): Promise<DownloadResult> {
    const bucketId = await this.getBucketId(driveId);

    const url = `${this.config.providerUrl}/fs/${bucketId}/file?path=${encodeURIComponent(path)}`;
    const response = await fetch(url);

    if (response.status === 404) {
      throw new Error(`File not found: ${path}`);
    }
    if (!response.ok) {
      throw new Error(`Download failed: ${response.status} ${await response.text()}`);
    }

    const data = new Uint8Array(await response.arrayBuffer());
    return {
      data,
      contentType: response.headers.get("content-type") || "application/octet-stream",
      size: data.length,
    };
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

    const json = (await response.json()) as Record<string, any>;
    return this.fromBase64(json.data);
  }

  /**
   * List directory contents
   */
  async listDirectory(
    driveId: bigint,
    path: string,
    options?: { recursive?: boolean }
  ): Promise<DirectoryListing> {
    const bucketId = await this.getBucketId(driveId);

    const params = new URLSearchParams({ path });
    if (options?.recursive) params.set("recursive", "true");

    const url = `${this.config.providerUrl}/fs/${bucketId}/ls?${params}`;
    const response = await fetch(url);

    if (!response.ok) {
      throw new Error(
        `List directory failed: ${response.status} ${await response.text()}`
      );
    }

    const result = (await response.json()) as Record<string, any>;
    return {
      path: result.path,
      entries: (result.entries || []).map((e: any) => ({
        name: e.name,
        path: e.path,
        entry_type: e.entry_type,
        size: e.size ?? 0,
        mtime: e.mtime ?? 0,
      })),
      fileCount: result.file_count ?? 0,
      dirCount: result.dir_count ?? 0,
      totalSize: result.total_size ?? 0,
    };
  }

  /**
   * Delete a file
   */
  async deleteFile(driveId: bigint, path: string): Promise<{ deleted: boolean }> {
    const bucketId = await this.getBucketId(driveId);

    const url = `${this.config.providerUrl}/fs/${bucketId}/file?path=${encodeURIComponent(path)}`;
    const response = await fetch(url, { method: "DELETE" });

    if (!response.ok) {
      throw new Error(
        `Delete failed: ${response.status} ${await response.text()}`
      );
    }

    return (await response.json()) as { deleted: boolean };
  }

  /**
   * Get the drive's index root (merkle root, file/dir counts, total size)
   */
  async getIndexRoot(driveId: bigint): Promise<IndexRoot> {
    const bucketId = await this.getBucketId(driveId);

    const url = `${this.config.providerUrl}/fs/${bucketId}/index_root`;
    const response = await fetch(url);

    if (!response.ok) {
      throw new Error(
        `Get index root failed: ${response.status} ${await response.text()}`
      );
    }

    const result = (await response.json()) as Record<string, any>;
    return {
      metadataMerkleRoot: result.metadata_merkle_root ?? result.merkle_root ?? "",
      fileCount: result.file_count ?? 0,
      dirCount: result.dir_count ?? 0,
      totalSize: result.total_size ?? 0,
    };
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
