/**
 * S3-Compatible Client SDK
 *
 * TypeScript client for the Web3 Storage S3-Compatible Interface.
 * Bucket operations go through the chain. Object operations go through
 * the provider's S3 HTTP API.
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

  // --- Bucket Operations (chain) ---

  /**
   * Create a new S3 bucket
   *
   * If `capacity`, `duration`, and `maxPayment` are provided, attempts the
   * atomic `create_s3_bucket_with_storage` extrinsic (bucket + storage
   * agreement in one transaction). Falls back to `create_s3_bucket` if the
   * runtime doesn't support the atomic extrinsic.
   */
  async createBucket(
    name: string,
    options?: CreateBucketOptions
  ): Promise<BucketInfo> {
    this.ensureConnected();
    this.validateBucketName(name);

    const nameBytes = Binary.fromText(name);

    // Try atomic extrinsic when storage options are provided
    if (options?.capacity && options?.duration && options?.maxPayment) {
      try {
        const tx = this.api!.tx.S3Registry.create_s3_bucket_with_storage({
          name: nameBytes,
          max_capacity: options.capacity,
          duration: options.duration,
          max_payment: options.maxPayment,
        });
        const result = await tx.signAndSubmit(this.signer!);

        const events = this.api!.event.S3Registry.S3BucketCreated.filter(result.events);
        if (events.length > 0) {
          return this.headBucket(name);
        }
      } catch {
        // Atomic extrinsic not available — fall through to legacy
      }
    }

    // Legacy: create bucket only (no storage agreement)
    const result = await this.api!.tx.S3Registry.create_s3_bucket({
      name: nameBytes,
      min_providers: options?.minProviders ?? 1,
    }).signAndSubmit(this.signer!);

    const events = this.api!.event.S3Registry.S3BucketCreated.filter(result.events);
    if (events.length === 0) {
      throw new Error("S3BucketCreated event not found");
    }

    return this.headBucket(name);
  }

  /**
   * Delete an S3 bucket
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
      Binary.fromText(name)
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
      name: new TextDecoder().decode(bucket.name),
      layer0BucketId: bucket.layer0_bucket_id,
      owner: bucket.owner,
      createdAt: bucket.created_at,
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
          name: new TextDecoder().decode(bucket.name),
          layer0BucketId: bucket.layer0_bucket_id,
          owner: bucket.owner,
          createdAt: bucket.created_at,
        });
      }
    }
    return buckets;
  }

  // --- Object Operations (provider HTTP API) ---

  /**
   * Upload an object via the provider's S3 HTTP API
   */
  async putObject(
    bucket: string,
    key: string,
    data: Uint8Array,
    options?: PutObjectOptions
  ): Promise<PutObjectResponse> {
    this.validateObjectKey(key);

    const bucketInfo = await this.headBucket(bucket);

    const contentType = options?.contentType || "application/octet-stream";
    const headers: Record<string, string> = {
      "Content-Type": contentType,
    };
    if (options?.metadata) {
      for (const [k, v] of Object.entries(options.metadata)) {
        headers[`x-amz-meta-${k}`] = v;
      }
    }

    const url = `${this.config.providerUrl}/s3/${bucketInfo.layer0BucketId}/object?key=${encodeURIComponent(key)}`;
    const response = await fetch(url, {
      method: "PUT",
      headers,
      body: data,
    });

    if (!response.ok) {
      throw new Error(`Upload failed: ${response.status} ${await response.text()}`);
    }

    const result = (await response.json()) as Record<string, any>;
    return {
      cid: result.data_root || result.etag,
      etag: result.etag,
      size: result.size || data.length,
    };
  }

  /**
   * Download an object via the provider's S3 HTTP API
   */
  async getObject(bucket: string, key: string): Promise<GetObjectResponse> {
    const bucketInfo = await this.headBucket(bucket);

    const url = `${this.config.providerUrl}/s3/${bucketInfo.layer0BucketId}/object?key=${encodeURIComponent(key)}`;
    const response = await fetch(url);

    if (response.status === 404) {
      throw new Error(`Object not found: ${bucket}/${key}`);
    }
    if (!response.ok) {
      throw new Error(`Download failed: ${response.status}`);
    }

    const data = new Uint8Array(await response.arrayBuffer());

    const metadata: ObjectMetadata = {
      key,
      cid: response.headers.get("etag") || "",
      size: data.length,
      lastModified: BigInt(response.headers.get("last-modified") || "0"),
      contentType: response.headers.get("content-type") || "application/octet-stream",
      etag: response.headers.get("etag") || "",
      metadata: this.extractAmzMetadata(response.headers),
    };

    return { data, metadata };
  }

  /**
   * Get object metadata without downloading (HEAD request)
   */
  async headObject(bucket: string, key: string): Promise<ObjectMetadata> {
    const bucketInfo = await this.headBucket(bucket);

    const url = `${this.config.providerUrl}/s3/${bucketInfo.layer0BucketId}/object?key=${encodeURIComponent(key)}`;
    const response = await fetch(url, { method: "HEAD" });

    if (response.status === 404) {
      throw new Error(`Object not found: ${bucket}/${key}`);
    }
    if (!response.ok) {
      throw new Error(`HEAD failed: ${response.status}`);
    }

    return {
      key,
      cid: response.headers.get("x-amz-data-root") || response.headers.get("etag") || "",
      size: Number(response.headers.get("content-length") || "0"),
      lastModified: BigInt(response.headers.get("last-modified") || "0"),
      contentType: response.headers.get("content-type") || "application/octet-stream",
      etag: response.headers.get("etag") || "",
      metadata: this.extractAmzMetadata(response.headers),
    };
  }

  /**
   * Delete an object via the provider's S3 HTTP API
   */
  async deleteObject(bucket: string, key: string): Promise<void> {
    const bucketInfo = await this.headBucket(bucket);

    const url = `${this.config.providerUrl}/s3/${bucketInfo.layer0BucketId}/object?key=${encodeURIComponent(key)}`;
    const response = await fetch(url, { method: "DELETE" });

    if (!response.ok) {
      throw new Error(`Delete failed: ${response.status}`);
    }
  }

  /**
   * Copy an object (download + re-upload via provider HTTP)
   */
  async copyObject(
    srcBucket: string,
    srcKey: string,
    dstBucket: string,
    dstKey: string
  ): Promise<void> {
    const srcObj = await this.getObject(srcBucket, srcKey);
    await this.putObject(dstBucket, dstKey, srcObj.data, {
      contentType: srcObj.metadata.contentType || undefined,
      metadata: srcObj.metadata.metadata,
    });
  }

  /**
   * List objects in a bucket via the provider's S3 HTTP API
   */
  async listObjectsV2(
    bucket: string,
    params?: ListObjectsParams
  ): Promise<ListObjectsResponse> {
    const bucketInfo = await this.headBucket(bucket);

    const urlParams = new URLSearchParams();
    if (params?.prefix) urlParams.set("prefix", params.prefix);
    if (params?.delimiter) urlParams.set("delimiter", params.delimiter);
    if (params?.maxKeys) urlParams.set("max_keys", params.maxKeys.toString());
    if (params?.continuationToken) urlParams.set("continuation_token", params.continuationToken);

    const qs = urlParams.toString();
    const url = `${this.config.providerUrl}/s3/${bucketInfo.layer0BucketId}/objects${qs ? `?${qs}` : ""}`;
    const response = await fetch(url);

    if (!response.ok) {
      throw new Error(`List failed: ${response.status}`);
    }

    const result = (await response.json()) as Record<string, any>;

    const contents: ObjectSummary[] = (result.objects || []).map((obj: any) => ({
      key: obj.key,
      size: obj.size,
      lastModified: BigInt(obj.last_modified || 0),
      etag: obj.etag || "",
    }));

    return {
      contents,
      commonPrefixes: result.common_prefixes || [],
      isTruncated: result.is_truncated || false,
      keyCount: result.key_count || contents.length,
      nextContinuationToken: result.next_continuation_token,
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

  private extractAmzMetadata(headers: Headers): Record<string, string> {
    const metadata: Record<string, string> = {};
    headers.forEach((value, name) => {
      const lower = name.toLowerCase();
      if (lower.startsWith("x-amz-meta-")) {
        metadata[lower.slice("x-amz-meta-".length)] = value;
      }
    });
    return metadata;
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
