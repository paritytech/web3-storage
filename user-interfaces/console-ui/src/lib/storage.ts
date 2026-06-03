/**
 * Storage SDK - Browser-compatible wrapper for S3 operations
 * Uses real chain types via polkadot-api
 */

import { createClient, type PolkadotClient, type TypedApi } from "polkadot-api";
import { getWsProvider } from "polkadot-api/ws";
import { getPolkadotSigner } from "polkadot-api/signer";
import { parachain } from "@polkadot-api/descriptors";
import { Binary, Enum } from "polkadot-api";
import { resolveProviderEndpoint } from "@web3-storage/papi";
import { EncryptionKey } from "./encryption";
import { type Keypair, seedToKeypair, toHex, toSs58 } from "./crypto";

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

/**
 * Provider-signed agreement terms returned by `POST /negotiate` on the
 * provider node. The signature is the SCALE-encoded `MultiSignature` as
 * hex (e.g. `0x01<64-byte-sr25519-sig>`).
 */
export interface SignedTerms {
  terms: {
    owner: string;
    max_bytes: number | bigint;
    duration: number;
    price_per_byte: number | bigint;
    valid_until: number;
    nonce: number | bigint;
    replica_params: unknown | null;
  };
  signature: string;
}

export interface NegotiateRequest {
  owner: string;
  max_bytes: number | bigint;
  duration: number;
  price_per_byte: number | bigint;
  replica_params: unknown | null;
}

/**
 * Parse a multiaddr (e.g. `/ip4/127.0.0.1/tcp/3333` or `/dns4/host/tcp/3333`)
 * into an HTTP base URL. Exported so the UI can derive the negotiate
 * endpoint from a picked provider's chain metadata before any agreement
 * exists on chain.
 */
export function parseMultiaddrToHttp(multiaddr: string): string | null {
  const parts = multiaddr.split("/").filter(Boolean);
  let host: string | null = null;
  let port: string | null = null;
  for (let i = 0; i < parts.length; i++) {
    if (
      (parts[i] === "ip4" ||
        parts[i] === "ip6" ||
        parts[i] === "dns4" ||
        parts[i] === "dns6") &&
      parts[i + 1]
    ) {
      host = parts[i + 1];
    }
    if (parts[i] === "tcp" && parts[i + 1]) {
      port = parts[i + 1];
    }
  }
  return host && port ? `http://${host}:${port}` : null;
}

/**
 * POST a `NegotiateRequest` to the provider's `/negotiate` endpoint and
 * return the provider-signed terms bundle. Mirrors
 * `examples/papi/api.js::negotiateTerms`.
 */
export async function negotiateTerms(
  providerUrl: string,
  request: NegotiateRequest,
): Promise<SignedTerms> {
  const res = await fetch(`${providerUrl.replace(/\/$/, "")}/negotiate`, {
    method: "POST",
    headers: { "content-type": "application/json" },
    body: JSON.stringify(request, (_k, v) =>
      typeof v === "bigint" ? v.toString() : v,
    ),
  });
  if (!res.ok) {
    const body = await res.text().catch(() => "");
    throw new Error(`/negotiate failed: ${res.status} ${body}`);
  }
  return res.json();
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
    out[i] = parseInt(h.substr(i * 2, 2), 16);
  }
  return out;
}

/**
 * Build the agreement terms and sign it
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
  const sig = Enum(variantName as any, sigPayloadHex);

  const t = signed.terms;
  const terms = {
    owner: t.owner,
    max_bytes: BigInt(t.max_bytes),
    duration: t.duration,
    price_per_byte: BigInt(t.price_per_byte),
    valid_until: t.valid_until,
    nonce: BigInt(t.nonce),
    replica_params: (t.replica_params ?? undefined) as any,
  };
  return { provider: providerAccount, terms, sig };
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
  private providerUrlCache: Map<string, string> = new Map();
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
      console.log("[StorageClient] Typed API ready");
    } catch (err) {
      console.error("[StorageClient] Failed to get typed API (descriptor mismatch?):", err);
      throw err;
    }
  }

  async setSigner(seed: string): Promise<string> {
    const keypair = seedToKeypair(seed);
    this.keypair = keypair;
    this.signer = getPolkadotSigner(keypair.publicKey, "Sr25519", (input) =>
      keypair.sign(input),
    );
    this.signerAddress = toSs58(keypair.publicKey);
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

          // Check for dispatch error. Two ways the tx can fail and we
          // need to catch both: (a) ev.ok === false means PAPI saw a
          // dispatch error directly, (b) some runtimes only surface it
          // via a System.ExtrinsicFailed event in events[]. When events
          // fail to decode (events.length === 0 with a stale descriptor),
          // path (a) is the only signal we have.
          if (ev.ok === false) {
            const errorStr = ev.dispatchError
              ? JSON.stringify(ev.dispatchError, (_k, v) =>
                  typeof v === "bigint" ? v.toString() : v
                )
              : "dispatch error (no detail)";
            console.error("[StorageClient] tx ok=false:", errorStr);
            reject(new Error(`Transaction failed on-chain: ${errorStr}`));
            return;
          }
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

    // Try to resolve from on-chain bucket data (works when provider has accepted agreement)
    try {
      if (!this.api) throw new Error("Not connected");
      const url = await resolveProviderEndpoint(this.api, bucketId);
      this.providerUrlCache.set(key, url);
      onProgress?.("Provider ready", 1, 1);
      return url;
    } catch {
      // Fall back to default local provider for dev chains.
      // The provider stores data regardless of on-chain agreements — agreements
      // are only needed for checkpoints/accountability, not for HTTP uploads.
      if (this.chainWs.includes("127.0.0.1") || this.chainWs.includes("localhost")) {
        const fallback = "http://127.0.0.1:3333";
        console.log(`[StorageClient] No on-chain provider for bucket ${bucketId}, using local fallback: ${fallback}`);
        this.providerUrlCache.set(key, fallback);
        return fallback;
      }
      throw new Error(`Bucket ${bucketId} has no primary providers and no fallback available`);
    }
  }

  /** Clear cached provider URL for a bucket (e.g. after provider changes). */
  invalidateProviderCache(bucketId?: bigint): void {
    if (bucketId !== undefined) {
      this.providerUrlCache.delete(bucketId.toString());
    } else {
      this.providerUrlCache.clear();
    }
  }

  // --- S3 Operations ---

  /**
   * Redeem provider-signed agreement terms on chain to open a Layer-0
   * bucket + primary agreement and register the S3 metadata bucket on top
   * of it — atomically in one extrinsic.
   *
   * **This is only step 2 of bucket creation.** Step 1 is the HTTP
   * negotiation against the provider (`negotiateTerms`). Splitting the two
   * lets the caller retry a failed on-chain submit without re-negotiating
   * (the signed terms are still valid until `terms.valid_until`).
   *
   * The provider URL is needed only so the cache can be primed for the
   * first upload — the chain itself doesn't see it.
   */
  async submitCreateBucket(
    name: string,
    providerAccount: string,
    providerUrl: string,
    signed: SignedTerms,
  ): Promise<BucketInfo> {
    this.ensureConnected();
    this.validateBucketName(name);

    console.log(
      "[StorageClient] submitCreateBucket:",
      name,
      "provider=",
      providerAccount,
      providerUrl,
      "nonce=",
      signed.terms.nonce,
    );

    const tx = this.api!.tx.S3Registry.create_s3_bucket({
      name: Binary.fromText(name),
      ...buildSignedTermsArgs(providerAccount, signed),
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
        // Prime the provider URL cache so the first upload doesn't have to
        // resolve it from chain.
        this.providerUrlCache.set(looked.layer0BucketId.toString(), providerUrl);
        return looked;
      }
      throw new Error(
        "S3BucketCreated event not found and bucket lookup failed. " +
        "The runtime descriptor may be stale — run: npx papi update"
      );
    }

    // We already know the provider HTTP URL — prime the cache so the first
    // upload skips the on-chain lookup.
    if (layer0BucketId !== null) {
      this.providerUrlCache.set(layer0BucketId.toString(), providerUrl);
    }

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
      Binary.fromText(name)
    );

    if (bucketId === undefined) return null;

    const bucket = await this.api!.query.S3Registry.S3Buckets.getValue(bucketId);
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

    // Encrypt data before upload if encryption is enabled
    const uploadData = this.encryptionKey
      ? await this.encryptionKey.encrypt(data)
      : data;

    // Upload via S3 endpoint which handles chunking, Merkle tree, MMR commit,
    // and S3 index update in a single request.
    const headers: Record<string, string> = {
      "Content-Type": options?.contentType || "application/octet-stream",
      ...this.signRequest("PUT", bucketId),
    };
    if (options?.metadata) {
      for (const [k, v] of Object.entries(options.metadata)) {
        headers[`x-amz-meta-${k}`] = v;
      }
    }

    const response = await fetch(
      `${providerUrl}/s3/${Number(bucketId)}/object?key=${encodeURIComponent(key)}`,
      { method: "PUT", headers, body: uploadData },
    );

    if (!response.ok) {
      throw new Error(`Upload failed: ${response.status} ${await response.text()}`);
    }

    const result = await response.json();
    return { cid: result.data_root || result.etag, size: data.length };
  }

  /**
   * Download an S3 object by key. Returns the raw bytes.
   * Uses the S3 GET endpoint which reassembles chunks into the full object.
   */
  async downloadS3Object(bucketId: bigint, key: string): Promise<Blob> {
    const providerUrl = await this.getProviderUrl(bucketId);
    const response = await fetch(
      `${providerUrl}/s3/${Number(bucketId)}/object?key=${encodeURIComponent(key)}`,
      { headers: this.signRequest("GET", bucketId) },
    );
    if (!response.ok) {
      throw new Error(`Download failed: ${response.status} ${await response.text()}`);
    }

    // Decrypt after download if encryption is enabled
    if (this.encryptionKey) {
      const encrypted = new Uint8Array(await response.arrayBuffer());
      const decrypted = await this.encryptionKey.decrypt(encrypted);
      return new Blob([decrypted]);
    }

    return response.blob();
  }

  async listObjects(bucketId: bigint, prefix?: string): Promise<S3ObjectInfo[]> {
    const providerUrl = await this.getProviderUrl(bucketId);
    const params = new URLSearchParams();
    if (prefix) params.set("prefix", prefix);

    const response = await fetch(
      `${providerUrl}/s3/${Number(bucketId)}/objects?${params.toString()}`,
      { headers: this.signRequest("GET", bucketId) },
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

  // --- S3 Additional Operations ---

  async deleteObject(bucketId: bigint, key: string): Promise<void> {
    const providerUrl = await this.getProviderUrl(bucketId);
    const response = await fetch(
      `${providerUrl}/s3/${Number(bucketId)}/object?key=${encodeURIComponent(key)}`,
      { method: "DELETE", headers: this.signRequest("DELETE", bucketId) },
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
          this.signerAddress, bucketId
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

      const multiaddrStr = new TextDecoder().decode(provider.multiaddr);

      const url = parseMultiaddrToHttp(multiaddrStr) ?? "unknown";
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
      // 0 means unlimited
      let availableCapacity: bigint = 0n;

      if (maxCapacity !== 0n) {
        // On-chain already enforces `maxCapacity` > `committedBytes`
        availableCapacity = maxCapacity - committedBytes;
      }

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

}

// Singleton instance
let storageClient: StorageClient | null = null;

export function getStorageClient(chainWs: string): StorageClient {
  if (!storageClient || storageClient["chainWs"] !== chainWs) {
    storageClient = new StorageClient(chainWs);
  }
  return storageClient;
}
