/**
 * S3-Compatible Client SDK
 *
 * TypeScript client for the Web3 Storage S3-Compatible Interface.
 */

import { createClient, PolkadotClient } from "polkadot-api";
import { getWsProvider } from "polkadot-api/ws-provider";
import { getPolkadotSigner } from "polkadot-api/signer";
import { Binary } from "@polkadot-api/substrate-bindings";
import { Keyring } from "@polkadot/keyring";
import { cryptoWaitReady, blake2AsU8a } from "@polkadot/util-crypto";
import { parachain } from "@polkadot-api/descriptors";

import type {
  S3Config,
  BucketInfo,
  ObjectMetadata,
  PutObjectOptions,
  PutObjectResponse,
  GetObjectResponse,
  ListObjectsParams,
  ListObjectsResponse,
  ObjectSummary,
  CreateBucketOptions,
} from "./types.js";

/**
 * S3-Compatible Client
 *
 * Provides an S3-compatible interface for object storage on the
 * Web3 Storage decentralized storage network.
 *
 * @example
 * ```typescript
 * const client = new S3Client({
 *   chainWs: "ws://127.0.0.1:2222",
 *   providerUrl: "http://127.0.0.1:3333",
 * });
 *
 * await client.connect();
 * await client.setSigner("//Alice");
 *
 * await client.createBucket("my-bucket");
 * await client.putObject("my-bucket", "hello.txt", new TextEncoder().encode("Hello!"));
 * const obj = await client.getObject("my-bucket", "hello.txt");
 * ```
 */
export class S3Client {
  private config: S3Config;
  private client: PolkadotClient | null = null;
  private api: ReturnType<PolkadotClient["getTypedApi"]> | null = null;
  private signer: ReturnType<typeof getPolkadotSigner> | null = null;
  private signerAddress: string | null = null;

  constructor(config: S3Config) {
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

  // --- Bucket Operations ---

  /**
   * Create a new S3 bucket
   */
  async createBucket(
    name: string,
    options?: CreateBucketOptions
  ): Promise<BucketInfo> {
    this.ensureConnected();
    this.validateBucketName(name);

    const result = await this.api!.tx.S3Registry.create_s3_bucket({
      name: Binary.fromBytes(new TextEncoder().encode(name)),
      min_providers: options?.minProviders ?? 1,
    }).signAndSubmit(this.signer!);

    // Extract bucket info from events
    const events = this.api!.event.S3Registry.S3BucketCreated.filter(result.events);
    if (events.length === 0) {
      throw new Error("S3BucketCreated event not found");
    }

    return this.headBucket(name);
  }

  /**
   * Delete an S3 bucket (must be empty)
   */
  async deleteBucket(name: string): Promise<void> {
    this.ensureConnected();

    const bucket = await this.headBucket(name);

    await this.api!.tx.S3Registry.delete_s3_bucket({
      s3_bucket_id: bucket.s3BucketId,
    }).signAndSubmit(this.signer!);
  }

  /**
   * Get bucket information
   */
  async headBucket(name: string): Promise<BucketInfo> {
    this.ensureConnected();

    // Look up bucket ID by name
    const bucketId = await this.api!.query.S3Registry.BucketNameToId.getValue(
      Binary.fromBytes(new TextEncoder().encode(name))
    );

    if (bucketId === undefined) {
      throw new Error(`Bucket not found: ${name}`);
    }

    const bucket = await this.api!.query.S3Registry.S3Buckets.getValue(bucketId);
    if (!bucket) {
      throw new Error(`Bucket data not found: ${name}`);
    }

    return {
      s3BucketId: bucketId,
      name: new TextDecoder().decode(bucket.name.asBytes()),
      layer0BucketId: bucket.layer0_bucket_id,
      owner: bucket.owner,
      createdAt: bucket.created_at,
      objectCount: bucket.object_count,
      totalSize: bucket.total_size,
    };
  }

  /**
   * List all buckets owned by the user
   */
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
        buckets.push({
          s3BucketId: bucketId,
          name: new TextDecoder().decode(bucket.name.asBytes()),
          layer0BucketId: bucket.layer0_bucket_id,
          owner: bucket.owner,
          createdAt: bucket.created_at,
          objectCount: bucket.object_count,
          totalSize: bucket.total_size,
        });
      }
    }
    return buckets;
  }

  // --- Object Operations ---

  /**
   * Upload an object
   */
  async putObject(
    bucket: string,
    key: string,
    data: Uint8Array,
    options?: PutObjectOptions
  ): Promise<PutObjectResponse> {
    this.ensureConnected();
    this.validateObjectKey(key);

    const bucketInfo = await this.headBucket(bucket);

    // Calculate content hash (CID)
    const hash = blake2AsU8a(data);
    const cid = this.toHex(hash);
    const etag = cid.slice(2, 34); // First 16 bytes as hex

    // Upload to provider
    const response = await fetch(`${this.config.providerUrl}/node`, {
      method: "PUT",
      headers: { "Content-Type": "application/json" },
      body: JSON.stringify({
        bucket_id: Number(bucketInfo.layer0BucketId),
        hash: cid,
        data: this.toBase64(data),
        children: null,
      }),
    });

    if (!response.ok) {
      throw new Error(`Upload failed: ${response.status} ${await response.text()}`);
    }

    // Commit to MMR
    await fetch(`${this.config.providerUrl}/commit`, {
      method: "POST",
      headers: { "Content-Type": "application/json" },
      body: JSON.stringify({
        bucket_id: Number(bucketInfo.layer0BucketId),
        data_roots: [cid],
      }),
    });

    // Update metadata on-chain
    const contentType = options?.contentType || "application/octet-stream";
    const metadata: Array<[Uint8Array, Uint8Array]> = [];
    if (options?.metadata) {
      for (const [k, v] of Object.entries(options.metadata)) {
        metadata.push([
          new TextEncoder().encode(k),
          new TextEncoder().encode(v),
        ]);
      }
    }

    await this.api!.tx.S3Registry.put_object_metadata({
      s3_bucket_id: bucketInfo.s3BucketId,
      key: Binary.fromBytes(new TextEncoder().encode(key)),
      cid: Binary.fromBytes(hash),
      size: BigInt(data.length),
      content_type: Binary.fromBytes(new TextEncoder().encode(contentType)),
      user_metadata: metadata.map(([k, v]) => [Binary.fromBytes(k), Binary.fromBytes(v)]),
    }).signAndSubmit(this.signer!);

    return { cid, etag, size: data.length };
  }

  /**
   * Download an object
   */
  async getObject(bucket: string, key: string): Promise<GetObjectResponse> {
    this.ensureConnected();

    const metadata = await this.headObject(bucket, key);
    const bucketInfo = await this.headBucket(bucket);

    // Download from provider
    const response = await fetch(
      `${this.config.providerUrl}/node?hash=${metadata.cid}&bucket_id=${bucketInfo.layer0BucketId}`
    );

    if (!response.ok) {
      throw new Error(`Download failed: ${response.status}`);
    }

    const json = await response.json();
    const data = this.fromBase64(json.data);

    return { data, metadata };
  }

  /**
   * Get object metadata without downloading
   */
  async headObject(bucket: string, key: string): Promise<ObjectMetadata> {
    this.ensureConnected();

    const bucketInfo = await this.headBucket(bucket);

    const metadata = await this.api!.query.S3Registry.Objects.getValue(
      bucketInfo.s3BucketId,
      Binary.fromBytes(new TextEncoder().encode(key))
    );

    if (!metadata) {
      throw new Error(`Object not found: ${bucket}/${key}`);
    }

    const userMetadata: Record<string, string> = {};
    for (const [k, v] of metadata.user_metadata) {
      userMetadata[new TextDecoder().decode(k.asBytes())] = new TextDecoder().decode(
        v.asBytes()
      );
    }

    return {
      key,
      cid: this.toHex(metadata.cid.asBytes()),
      size: Number(metadata.size),
      lastModified: metadata.last_modified,
      contentType: new TextDecoder().decode(metadata.content_type.asBytes()),
      etag: this.toHex(metadata.etag.asBytes()),
      metadata: userMetadata,
    };
  }

  /**
   * Delete an object
   */
  async deleteObject(bucket: string, key: string): Promise<void> {
    this.ensureConnected();

    const bucketInfo = await this.headBucket(bucket);

    await this.api!.tx.S3Registry.delete_object_metadata({
      s3_bucket_id: bucketInfo.s3BucketId,
      key: Binary.fromBytes(new TextEncoder().encode(key)),
    }).signAndSubmit(this.signer!);
  }

  /**
   * Copy an object
   */
  async copyObject(
    srcBucket: string,
    srcKey: string,
    dstBucket: string,
    dstKey: string
  ): Promise<void> {
    this.ensureConnected();

    const srcBucketInfo = await this.headBucket(srcBucket);
    const dstBucketInfo = await this.headBucket(dstBucket);

    await this.api!.tx.S3Registry.copy_object_metadata({
      src_bucket_id: srcBucketInfo.s3BucketId,
      src_key: Binary.fromBytes(new TextEncoder().encode(srcKey)),
      dst_bucket_id: dstBucketInfo.s3BucketId,
      dst_key: Binary.fromBytes(new TextEncoder().encode(dstKey)),
    }).signAndSubmit(this.signer!);
  }

  /**
   * List objects in a bucket
   */
  async listObjectsV2(
    bucket: string,
    params?: ListObjectsParams
  ): Promise<ListObjectsResponse> {
    this.ensureConnected();

    await this.headBucket(bucket);

    // In a full implementation, we would iterate over the Objects storage map
    // with prefix filtering. For now, return a simplified response.
    // This would require enumerating the StorageDoubleMap entries.

    const contents: ObjectSummary[] = [];
    const commonPrefixes: string[] = [];

    // Note: Full implementation would iterate Objects storage map
    // and apply prefix/delimiter filtering

    return {
      contents,
      commonPrefixes,
      isTruncated: false,
      keyCount: contents.length,
    };
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

  private validateBucketName(name: string): void {
    if (name.length < 3 || name.length > 63) {
      throw new Error("Bucket name must be 3-63 characters");
    }
    if (!/^[a-z0-9][a-z0-9-]*[a-z0-9]$/.test(name) && name.length > 2) {
      throw new Error(
        "Bucket name must be lowercase alphanumeric with hyphens, not starting/ending with hyphen"
      );
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
    return Buffer.from(bytes).toString("base64");
  }

  private fromBase64(str: string): Uint8Array {
    return new Uint8Array(Buffer.from(str, "base64"));
  }
}
