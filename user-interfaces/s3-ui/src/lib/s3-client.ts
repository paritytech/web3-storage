// SPDX-License-Identifier: GPL-3.0-only

/**
 * S3 Client — wraps S3Registry pallet + StorageProvider pallet + provider HTTP
 * endpoints. Ported from console-ui's StorageClient, restructured to match
 * drive-ui's DriveClient class pattern (stateless w.r.t. connection).
 */

import { Binary, Enum, type PolkadotSigner, type Transaction, type TxFinalizedPayload } from "polkadot-api";
import { Subscription } from "rxjs";
import { parachain } from "@polkadot-api/descriptors";
import { resolveProviderEndpoint, toSs58, type ParachainApi } from "@web3-storage/papi";

export type Signer = PolkadotSigner;

const HTTP_RETRY_ATTEMPTS = 3;
const HTTP_RETRY_BASE_MS = 250;

// ── Types ─────────────────────────────────────────────────────────────────────

export interface BucketInfo {
  s3BucketId: bigint;
  name: string;
  layer0BucketId: bigint;
  owner: string;
  createdAt: bigint;
}

export interface S3ObjectInfo {
  key: string;
  size: number;
  lastModified: number;
  etag: string;
}

export interface UploadResult {
  cid: string;
  size: number;
}

export interface SignedTerms {
  terms: {
    owner: string;
    max_bytes: number | bigint;
    duration: number;
    price_per_byte: number | bigint;
    valid_until: number;
    nonce: number | bigint;
    replica_params: unknown | null;
    bucket_id: bigint | null;
  };
  signature: string;
}

export interface NegotiateRequest {
  owner: string;
  max_bytes: number | bigint;
  duration: number;
  price_per_byte: number | bigint;
  replica_params: unknown | null;
  bucket_id?: bigint | null;
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

export interface MatchingProviders extends AvailableProvider {
  matchScore: number;
  partialReason: string;
  committedBytes: bigint;
  replicaSyncPrice?: bigint;
  acceptingExtensions: boolean;
  registeredAt: number;
  agreementsExtended: number;
  agreementsNotExtended: number;
  agreementsBurned: number;
  challengesReceived: number;
  challengesFailed: number;
}

export type MemberRole = "Admin" | "Writer" | "Reader";

export interface BucketMember {
  account: string;
  role: MemberRole;
}

export interface CheckpointInfo {
  mmrRoot: string;
  startSeq: bigint;
  leafCount: bigint;
  checkpointBlock: number;
}

export interface CheckpointDuty {
  bucketId: number;
  mmrRoot: string;
  startSeq: number;
  leafCount: number;
  ready: boolean;
}

export interface ChallengeResult {
  challengeId: { deadline: number; index: number };
  respondBy: number;
}

export interface OpenChallenge {
  deadline: number;
  index: number;
  bucketId: bigint;
  provider: string;
  challenger: string;
  leafIndex: bigint;
  chunkIndex: bigint;
  deposit: bigint;
}

export interface ChallengeDefenseResult {
  challengeId: { deadline: number; index: number };
  provider: string;
  responseTimeBlocks: number;
  challengerCost: bigint;
  providerCost: bigint;
  blockNumber: number;
  blockHash: string;
}

export interface ChallengeSlashResult {
  challengeId: { deadline: number; index: number };
  provider: string;
  slashedAmount: bigint;
  challengerReward: bigint;
  blockNumber: number;
  blockHash: string;
}

export interface QueryMatchingProvidersParams {
  query: {
    bytesNeeded: bigint;
    minDuration: number;
    maxPricePerByte: bigint;
    primaryOnly: boolean;
  };
  limit: number;
}

// ── Helpers ───────────────────────────────────────────────────────────────────

function sleep(ms: number): Promise<void> {
  return new Promise((r) => setTimeout(r, ms));
}

function isAbortError(err: unknown): boolean {
  return (
    err instanceof DOMException &&
    (err.name === "AbortError" || err.code === DOMException.ABORT_ERR)
  );
}

function isRetryableHttpError(status: number | null): boolean {
  if (status === null) return true;
  return status >= 500 && status < 600;
}

async function httpFetch(
  url: string,
  init: RequestInit & { signal?: AbortSignal } = {},
): Promise<Response> {
  let lastError: unknown = null;
  for (let attempt = 0; attempt < HTTP_RETRY_ATTEMPTS; attempt++) {
    try {
      const res = await fetch(url, init);
      if (res.ok || !isRetryableHttpError(res.status)) return res;
      lastError = new Error(`HTTP ${res.status}: ${await res.text().catch(() => "")}`);
    } catch (err) {
      if (isAbortError(err)) throw err;
      lastError = err;
    }
    if (attempt < HTTP_RETRY_ATTEMPTS - 1) {
      await sleep(HTTP_RETRY_BASE_MS * Math.pow(2, attempt));
    }
  }
  throw lastError instanceof Error ? lastError : new Error("HTTP request failed");
}

function hexToBytes(hex: string): Uint8Array {
  const h = hex.startsWith("0x") ? hex.slice(2) : hex;
  const out = new Uint8Array(h.length / 2);
  for (let i = 0; i < out.length; i++) {
    out[i] = parseInt(h.substring(i * 2, i * 2 + 2), 16);
  }
  return out;
}

function bytesToHex(bytes: Uint8Array): string {
  return Array.from(bytes)
    .map((b) => b.toString(16).padStart(2, "0"))
    .join("");
}

// MultiSignature SCALE variant order from sp_runtime.
const MULTI_SIGNATURE_VARIANT: Record<number, string> = {
  0: "Ed25519",
  1: "Sr25519",
  2: "Ecdsa",
  3: "Eth",
};

export function buildSignedTermsArgs(
  providerAccount: string,
  signed: SignedTerms,
) {
  const sigBytes = hexToBytes(signed.signature);
  if (sigBytes.length < 1) {
    throw new Error("signature too short to contain a MultiSignature variant byte");
  }
  const variantByte = sigBytes[0]!;
  const variantName = MULTI_SIGNATURE_VARIANT[variantByte];
  if (!variantName) {
    throw new Error(`unknown MultiSignature variant byte: ${variantByte}`);
  }
  const sigPayloadHex =
    "0x" +
    Array.from(sigBytes.slice(1))
      .map((b) => b.toString(16).padStart(2, "0"))
      .join("");
  // eslint-disable-next-line @typescript-eslint/no-explicit-any
  const sig = Enum(variantName as any, sigPayloadHex);

  const t = signed.terms;
  const terms = {
    owner: t.owner,
    max_bytes: BigInt(t.max_bytes),
    duration: t.duration,
    price_per_byte: BigInt(t.price_per_byte),
    valid_until: t.valid_until,
    nonce: BigInt(t.nonce),
    // eslint-disable-next-line @typescript-eslint/no-explicit-any
    replica_params: (t.replica_params ?? undefined) as any,
    bucket_id: t.bucket_id ? BigInt(t.bucket_id) : undefined,
  };
  return { provider: providerAccount, terms, sig };
}

export function parseMultiaddrToHttp(multiaddr: string): string | null {
  const parts = multiaddr.split("/").filter(Boolean);
  let host: string | null = null;
  let port: string | null = null;

  for (let i = 0; i < parts.length; i++) {
    const seg = parts[i];
    const next = parts[i + 1];
    if (!next) continue;

    if ((seg === "ip4" || seg === "ip6" || seg === "dns4" || seg === "dns6") && host === null) {
      host = seg.startsWith("ip6") ? `[${next}]` : next;
    }
    if (seg === "tcp" && port === null) {
      port = next;
    }
    if (host !== null && port !== null) break;
  }

  if (host && port) return `http://${host}:${port}`;
  return null;
}

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

function decodeName(name: unknown): string {
  if (name == null) return "";
  try {
    if (typeof name === "string") return name;
    if (typeof (name as any).asText === "function") return (name as any).asText();
    return new TextDecoder().decode(name as Uint8Array);
  } catch {
    return "";
  }
}

// ── S3Client class ────────────────────────────────────────────────────────────

export class S3Client {
  private api: ParachainApi | null = null;
  private signer: Signer | null = null;
  private signerAddress: string | null = null;
  private keypair: import("@/lib/crypto").Keypair | null = null;
  private providerUrlCache = new Map<string, string>();

  setApi(api: ParachainApi | null): void {
    if (api !== this.api) {
      this.providerUrlCache.clear();
    }
    this.api = api;
  }

  setSigner(signer: Signer | null, address: string | null): void {
    this.signer = signer;
    this.signerAddress = address;
  }

  setKeypair(keypair: import("@/lib/crypto").Keypair | null): void {
    this.keypair = keypair;
  }

  hasApi(): boolean {
    return this.api !== null;
  }

  hasSigner(): boolean {
    return this.signer !== null && this.signerAddress !== null;
  }

  getSignerAddress(): string | null {
    return this.signerAddress;
  }

  private requireApi(): ParachainApi {
    if (!this.api) throw new Error("Not connected to chain");
    return this.api;
  }

  private requireSigner(): { signer: Signer; address: string } {
    if (!this.signer || !this.signerAddress) throw new Error("Signer not set");
    return { signer: this.signer, address: this.signerAddress };
  }

  // ── Tx submission ─────────────────────────────────────────────────────────

  private async submit(tx: Transaction): Promise<TxFinalizedPayload> {
    const { signer } = this.requireSigner();
    const result = await tx.signAndSubmit(signer);
    if (!result.ok) {
      const err = JSON.stringify(result.dispatchError, (_k, v) =>
        typeof v === "bigint" ? v.toString() : v,
      );
      throw new Error(`Transaction failed on-chain: ${err}`);
    }
    return result;
  }

  // ── Provider resolution ───────────────────────────────────────────────────

  async getProviderUrl(bucketId: bigint): Promise<string> {
    const key = bucketId.toString();
    const cached = this.providerUrlCache.get(key);
    if (cached) return cached;
    const url = await resolveProviderEndpoint(this.requireApi(), bucketId);
    this.providerUrlCache.set(key, url);
    return url;
  }

  invalidateProviderUrl(bucketId: bigint): void {
    this.providerUrlCache.delete(bucketId.toString());
  }

  // ── S3 Bucket operations (on-chain) ─────────────────────────────────────

  async createBucket(
    name: string,
    providerAccount: string,
    providerUrl: string,
    signed: SignedTerms,
  ): Promise<BucketInfo> {
    const api = this.requireApi();
    const { address } = this.requireSigner();

    const tx = api.tx.S3Registry.create_s3_bucket({
      name: Binary.fromText(name),
      ...buildSignedTermsArgs(providerAccount, signed),
    });

    const result = await this.submit(tx);

    const created = api.event.S3Registry.S3BucketCreated.filter(result.events);
    if (created.length === 0) {
      // Fallback: query buckets for the latest
      const buckets = await this.listBuckets();
      if (buckets.length > 0) {
        const latest = buckets[buckets.length - 1]!;
        this.providerUrlCache.set(latest.layer0BucketId.toString(), providerUrl);
        return latest;
      }
      throw new Error(
        "S3BucketCreated event not found. The runtime descriptor may be stale — run: pnpm papi:generate",
      );
    }
    const { s3_bucket_id, layer0_bucket_id } = created[0]!.payload;

    this.providerUrlCache.set(layer0_bucket_id.toString(), providerUrl);

    return {
      s3BucketId: s3_bucket_id,
      name,
      layer0BucketId: layer0_bucket_id,
      owner: address,
      createdAt: 0n,
    };
  }

  async listBuckets(): Promise<BucketInfo[]> {
    const api = this.requireApi();
    const { address } = this.requireSigner();

    const bucketIds = await api.query.S3Registry.UserBuckets.getValue(address);
    if (bucketIds.length === 0) return [];

    const buckets: BucketInfo[] = [];
    for (const s3BucketId of bucketIds) {
      const bucket = await api.query.S3Registry.S3Buckets.getValue(s3BucketId);
      if (!bucket) continue;
      buckets.push({
        s3BucketId,
        name: decodeName(bucket.name),
        layer0BucketId: bucket.layer0_bucket_id,
        owner: bucket.owner,
        createdAt: BigInt(bucket.created_at ?? 0),
      });
    }
    return buckets;
  }

  async deleteBucket(s3BucketId: bigint): Promise<void> {
    const api = this.requireApi();
    const tx = api.tx.S3Registry.delete_s3_bucket({ s3_bucket_id: s3BucketId });
    await this.submit(tx);
  }

  // ── S3 Object operations (HTTP) ─────────────────────────────────────────

  private signRequest(method: string, bucketId: bigint): Record<string, string> {
    if (!this.keypair) return {};
    const timestamp = Math.floor(Date.now() / 1000).toString();
    const message = `web3storage:${method}:${Number(bucketId)}:${timestamp}`;
    const msgBytes = new TextEncoder().encode(message);
    const sig = this.keypair.sign(msgBytes);
    const pubHex = bytesToHex(this.keypair.publicKey);
    const sigHex = bytesToHex(sig);
    return { Authorization: `Web3Storage ${pubHex}:${sigHex}:${timestamp}` };
  }

  async putObject(
    bucketId: bigint,
    key: string,
    data: Uint8Array,
    options?: { signal?: AbortSignal },
  ): Promise<UploadResult> {
    const providerUrl = await this.getProviderUrl(bucketId);
    const params = new URLSearchParams({ key });
    const headers: Record<string, string> = {
      "Content-Type": "application/octet-stream",
      ...this.signRequest("PUT", bucketId),
    };
    const response = await httpFetch(
      `${providerUrl}/s3/${Number(bucketId)}/object?${params.toString()}`,
      {
        method: "PUT",
        headers,
        body: data as Uint8Array<ArrayBuffer>,
        signal: options?.signal,
      },
    );

    if (!response.ok) {
      throw new Error(`Upload failed: ${response.status} ${await response.text().catch(() => "")}`);
    }

    const result = await response.json().catch(() => ({}));
    return {
      cid: result.etag ?? result.cid ?? "",
      size: data.length,
    };
  }

  async getObject(
    bucketId: bigint,
    key: string,
  ): Promise<Uint8Array> {
    const providerUrl = await this.getProviderUrl(bucketId);
    const params = new URLSearchParams({ key });
    const response = await httpFetch(
      `${providerUrl}/s3/${Number(bucketId)}/object?${params.toString()}`,
      { headers: this.signRequest("GET", bucketId) },
    );

    if (!response.ok) {
      throw new Error(`Download failed: ${response.status}`);
    }

    const buffer = await response.arrayBuffer();
    return new Uint8Array(buffer);
  }

  async listObjects(
    bucketId: bigint,
    prefix?: string,
  ): Promise<S3ObjectInfo[]> {
    const providerUrl = await this.getProviderUrl(bucketId);
    const params = new URLSearchParams();
    if (prefix) params.set("prefix", prefix);
    const response = await httpFetch(
      `${providerUrl}/s3/${Number(bucketId)}/objects?${params.toString()}`,
      { headers: this.signRequest("GET", bucketId) },
    );

    if (!response.ok) {
      throw new Error(`List objects failed: ${response.status}`);
    }

    const result = await response.json();
    return (result.contents || []).map((o: any) => ({
      key: o.key,
      size: o.size ?? 0,
      lastModified: (o.last_modified ?? o.lastModified ?? 0) * 1000,
      etag: o.etag ?? "",
    }));
  }

  async deleteObject(bucketId: bigint, key: string): Promise<void> {
    const providerUrl = await this.getProviderUrl(bucketId);
    const params = new URLSearchParams({ key });
    const response = await httpFetch(
      `${providerUrl}/s3/${Number(bucketId)}/object?${params.toString()}`,
      {
        method: "DELETE",
        headers: this.signRequest("DELETE", bucketId),
      },
    );

    if (!response.ok) {
      throw new Error(`Delete failed: ${response.status} ${await response.text().catch(() => "")}`);
    }
  }

  // ── Members ─────────────────────────────────────────────────────────────

  async getBucketMembers(bucketId: bigint): Promise<BucketMember[]> {
    const api = this.requireApi();
    const bucket = await api.query.StorageProvider.Buckets.getValue(bucketId);
    if (!bucket) throw new Error(`Bucket ${bucketId} not found`);

    const roleMap: Record<string, MemberRole> = {
      Admin: "Admin",
      Writer: "Writer",
      Reader: "Reader",
    };

    return (bucket.members ?? []).map((m: { account: string; role: { type?: string } | string }) => {
      const roleType = typeof m.role === "string" ? m.role : m.role?.type ?? "Reader";
      return {
        account: m.account,
        role: roleMap[roleType] ?? "Reader",
      };
    });
  }

  async addMember(bucketId: bigint, account: string, role: MemberRole): Promise<void> {
    const api = this.requireApi();
    const tx = api.tx.StorageProvider.set_member({
      bucket_id: bucketId,
      member: account,
      role: Enum(role),
    });
    await this.submit(tx);
  }

  async removeMember(bucketId: bigint, account: string): Promise<void> {
    const api = this.requireApi();
    const tx = api.tx.StorageProvider.remove_member({
      bucket_id: bucketId,
      member: account,
    });
    await this.submit(tx);
  }

  // ── Provider discovery ────────────────────────────────────────────────────

  async listAvailableProviders(): Promise<AvailableProvider[]> {
    const api = this.requireApi();
    const entries = await api.query.StorageProvider.Providers.getEntries();
    const providers: AvailableProvider[] = [];

    for (const entry of entries) {
      const provider = entry.value;
      const account = entry.keyArgs[0] as string;
      const settings = provider.settings;

      const multiaddrStr = new TextDecoder().decode(provider.multiaddr);
      const maxCapacity = BigInt(settings.max_capacity ?? 0);
      const committedBytes = BigInt(provider.committed_bytes ?? 0);
      const availableCapacity =
        maxCapacity > committedBytes ? maxCapacity - committedBytes : 0n;

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
        // eslint-disable-next-line @typescript-eslint/no-explicit-any
        agreementsTotal: (provider.stats as any)?.agreements_total ?? 0,
      });
    }

    providers.sort((a, b) => {
      if (b.availableCapacity > a.availableCapacity) return 1;
      if (b.availableCapacity < a.availableCapacity) return -1;
      return 0;
    });

    return providers;
  }

  async queryMatchingProviders(
    query: QueryMatchingProvidersParams["query"],
    limit: QueryMatchingProvidersParams["limit"],
  ): Promise<MatchingProviders[]> {
    const api = this.requireApi();
    const matches = await api.apis.StorageProviderApi.find_matching_providers(
      {
        bytes_needed: query.bytesNeeded,
        min_duration: query.minDuration,
        max_price_per_byte: query.maxPricePerByte,
        primary_only: query.primaryOnly,
      },
      limit,
    );

    return matches
      .map((match) => {
        const info = match.info;
        const maxCapacity = BigInt(info.max_capacity ?? 0);
        const committedBytes = BigInt(info.committed_bytes ?? 0);
        const availableCapacity =
          maxCapacity > committedBytes ? maxCapacity - committedBytes : 0n;

        return {
          account: toSs58(match.account),
          multiaddr: new TextDecoder().decode(info.multiaddr),
          stake: BigInt(info.stake ?? 0),
          availableCapacity,
          maxCapacity,
          committedBytes,
          pricePerByte: BigInt(info.price_per_byte ?? 0),
          minDuration: info.min_duration ?? 0,
          maxDuration: info.max_duration ?? 0,
          acceptingPrimary: info.accepting_primary ?? false,
          replicaSyncPrice:
            info.replica_sync_price != null ? BigInt(info.replica_sync_price) : undefined,
          acceptingExtensions: info.accepting_extensions ?? false,
          registeredAt: Number(info.registered_at ?? 0),
          agreementsTotal: info.agreements_total ?? 0,
          agreementsExtended: info.agreements_extended ?? 0,
          agreementsNotExtended: info.agreements_not_extended ?? 0,
          agreementsBurned: info.agreements_burned ?? 0,
          challengesReceived: info.challenges_received ?? 0,
          challengesFailed: info.challenges_failed ?? 0,
          matchScore: match.match_score,
          partialReason: match.partial_reason?.type ?? "",
        };
      })
      .sort((a, b) => b.matchScore - a.matchScore);
  }

  // ── Checkpoint ──────────────────────────────────────────────────────────

  async getCheckpointInfo(bucketId: bigint): Promise<CheckpointInfo | null> {
    const api = this.requireApi();
    const bucket = await api.query.StorageProvider.Buckets.getValue(bucketId);
    if (!bucket) throw new Error(`Bucket ${bucketId} not found`);

    const snapshot = bucket.snapshot;
    if (!snapshot) return null;

    return {
      mmrRoot: snapshot.mmr_root,
      startSeq: snapshot.start_seq,
      leafCount: snapshot.leaf_count,
      checkpointBlock: snapshot.checkpoint_block,
    };
  }

  async getCheckpointDuty(bucketId: bigint): Promise<CheckpointDuty | null> {
    const providerUrl = await this.getProviderUrl(bucketId);
    const response = await httpFetch(
      `${providerUrl}/checkpoint/duty?bucket_id=${Number(bucketId)}`,
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
      `${providerUrl}/checkpoint/trigger?bucket_id=${Number(bucketId)}`,
      { method: "POST" },
    );

    if (!response.ok) {
      throw new Error(`Checkpoint trigger failed: ${response.status} ${await response.text().catch(() => "")}`);
    }
  }

  // ── Challenge ──────────────────────────────────────────────────────────

  async getBucketProviders(bucketId: bigint): Promise<string[]> {
    const api = this.requireApi();
    const bucket = await api.query.StorageProvider.Buckets.getValue(bucketId);
    if (!bucket) throw new Error(`Bucket ${bucketId} not found`);
    const providers: string[] = bucket.primary_providers ?? [];
    if (providers.length === 0) {
      throw new Error(`Bucket ${bucketId} has no primary providers`);
    }
    return providers;
  }

  async challengeCheckpoint(
    bucketId: bigint,
    provider: string,
    leafIndex: bigint,
    chunkIndex: bigint,
  ): Promise<ChallengeResult> {
    const api = this.requireApi();
    const tx = api.tx.StorageProvider.challenge_checkpoint({
      bucket_id: bucketId,
      provider,
      leaf_index: leafIndex,
      chunk_index: chunkIndex,
    });

    const result = await this.submit(tx);

    const created = api.event.StorageProvider.ChallengeCreated.filter(result.events);
    if (created.length === 0) {
      throw new Error("ChallengeCreated event not found in transaction result");
    }
    const { challenge_id, respond_by } = created[0]!.payload;
    return {
      challengeId: { deadline: challenge_id.deadline, index: challenge_id.index },
      respondBy: respond_by,
    };
  }

  async getLeafChunkCount(bucketId: bigint, leafIndex: bigint): Promise<number> {
    const providerUrl = await this.getProviderUrl(bucketId);
    const params = new URLSearchParams({
      bucket_id: Number(bucketId).toString(),
      leaf_index: leafIndex.toString(),
    });
    const response = await httpFetch(`${providerUrl}/mmr_proof?${params.toString()}`);
    if (!response.ok) {
      throw new Error(`MMR proof request failed: ${response.status}`);
    }
    const result = await response.json();
    const dataSize: number = result.leaf?.data_size ?? 0;
    const chunkSize = 262144; // 256 KiB — DEFAULT_CHUNK_SIZE
    return dataSize === 0 ? 1 : Math.ceil(dataSize / chunkSize);
  }

  async isChallengeActive(deadline: number): Promise<boolean> {
    const api = this.requireApi();
    const challenges = await api.query.StorageProvider.Challenges.getValue(deadline);
    return challenges !== undefined && challenges.length > 0;
  }

  async getOpenChallenges(bucketId: bigint): Promise<OpenChallenge[]> {
    const api = this.requireApi();
    // Challenges is a StorageDoubleMap keyed by (deadline, index): each entry
    // is a single challenge with keyArgs = [deadline, index] and value = the
    // Challenge (there is no per-deadline Vec to iterate).
    const entries = await api.query.StorageProvider.Challenges.getEntries();
    const result: OpenChallenge[] = [];

    for (const entry of entries) {
      const [deadline, index] = entry.keyArgs as [number, number];
      const c = entry.value;
      if (c.bucket_id === bucketId) {
        result.push({
          deadline,
          index,
          bucketId: c.bucket_id,
          provider: c.provider,
          challenger: c.challenger,
          leafIndex: c.leaf_index,
          chunkIndex: c.chunk_index,
          deposit: c.deposit,
        });
      }
    }

    return result.sort((a, b) => a.deadline - b.deadline);
  }

  watchChallengeOutcome(
    deadline: number,
    providerAddress: string,
    onDefended: (result: ChallengeDefenseResult) => void,
    onSlashed: (result: ChallengeSlashResult) => void,
  ): () => void {
    const api = this.requireApi();
    const sub = new Subscription();

    sub.add(
      api.event.StorageProvider.ChallengeDefended.watch().subscribe({
        next: ({ block, events }) => {
          for (const ev of events) {
            const p = ev.payload;
            if (p.challenge_id.deadline !== deadline) continue;
            if (p.provider !== providerAddress) continue;
            onDefended({
              challengeId: { deadline: p.challenge_id.deadline, index: p.challenge_id.index },
              provider: p.provider,
              responseTimeBlocks: p.response_time_blocks,
              challengerCost: p.challenger_cost,
              providerCost: p.provider_cost,
              blockNumber: block.number,
              blockHash: block.hash,
            });
          }
        },
        error: () => {},
      }),
    );

    sub.add(
      api.event.StorageProvider.ChallengeSlashed.watch().subscribe({
        next: ({ block, events }) => {
          for (const ev of events) {
            const p = ev.payload;
            if (p.challenge_id.deadline !== deadline) continue;
            if (p.provider !== providerAddress) continue;
            onSlashed({
              challengeId: { deadline: p.challenge_id.deadline, index: p.challenge_id.index },
              provider: p.provider,
              slashedAmount: p.slashed_amount,
              challengerReward: p.challenger_reward,
              blockNumber: block.number,
              blockHash: block.hash,
            });
          }
        },
        error: () => {},
      }),
    );

    return () => sub.unsubscribe();
  }
}

export { parachain };
