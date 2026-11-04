// SPDX-License-Identifier: Apache-2.0

/**
 * Provider-node HTTP helpers — the off-chain half of flows the pallet
 * wrappers complete. Platform-neutral (btoa/atob-based base64), so browser
 * consumers can typecheck and use these directly.
 */

import {
  base64ToBytes,
  bytesEq,
  bytesToBase64,
  chunkCount,
  CidMismatchError,
  computeCid,
  concatBytes,
  createLimiter,
  DEFAULT_CHUNK_SIZE,
  hashChildren,
  hashLeaf,
  hexToBytes,
  HttpError,
  httpFetch,
  leafChunkCount,
  merkleRoot,
  merkleTree,
  nodeHash,
  parseNode,
  signProviderRequest,
  splitChunks,
  verifyChunkProof,
  type MmrProof,
  type ProviderRequestSigner,
  type TreeNode,
} from "@web3-storage/core";
import { Enum } from "polkadot-api";

import { asHex, toHex, type ParachainApi } from "./address.js";
import type { ChainSigner } from "./signers.js";
import { READ_OPTS } from "./tx.js";

export interface ProviderFetchOpts extends ProviderHttpOpts {
  method?: string;
  /**
   * Query params. `bigint` is accepted so u64 ids (bucket/leaf/chunk) reach
   * the URL via `String(bigint)` — exact at any size, unlike `Number(bigint)`
   * which would round above 2^53. serde_urlencoded parses the decimal string
   * straight back into u64 provider-side.
   */
  params?: Record<string, string | number | bigint>;
  body?: unknown;
  /**
   * When set, attach the signed `Authorization` header the provider verifies
   * (`crates/providers/auth`) for a bucket-scoped, role-gated request
   * (`PUT /node`, `POST /commit`, …). Omit for public/read endpoints.
   */
  sign?: { signer: ProviderRequestSigner; bucketId: bigint | number };
}

/** Transport options shared by the provider HTTP helpers. */
export interface ProviderHttpOpts {
  /** `fetch` implementation (tests inject a fake provider here). */
  fetch?: typeof fetch;
  signal?: AbortSignal;
  /**
   * Attempts on a 5xx or network error (default 1: no retry). Only
   * idempotent requests should retry.
   */
  retries?: number;
}

/**
 * Send one JSON request to the provider node and return the parsed JSON
 * body. Throws {@link HttpError} (with the status) on a non-2xx response.
 */
export async function providerFetch(
  providerUrl: string,
  path: string,
  opts: ProviderFetchOpts = {},
): Promise<any> {
  const url = new URL(path, providerUrl);
  if (opts.params) {
    for (const [k, v] of Object.entries(opts.params)) url.searchParams.set(k, String(v));
  }
  const method = opts.method || "GET";
  const headers: Record<string, string> = {};
  if (opts.body) headers["Content-Type"] = "application/json";
  // auth.rs reconstructs the message from the upper-case HTTP verb; signing with
  // anything else would fail verification.
  if (opts.sign)
    Object.assign(headers, await signProviderRequest(opts.sign.signer, method.toUpperCase(), opts.sign.bucketId));
  const resp = await httpFetch(
    url.toString(),
    {
      method,
      headers: Object.keys(headers).length ? headers : undefined,
      body: opts.body ? JSON.stringify(opts.body) : undefined,
      signal: opts.signal,
    },
    { retries: opts.retries ?? 1, fetchImpl: opts.fetch },
  );
  if (!resp.ok) throw new HttpError(resp.status, `${path}: ${resp.status} ${await resp.text()}`);
  return resp.json();
}

export interface ProviderNodeReadiness {
  /** The node holds a signing keypair (from --keyfile). */
  signing_configured: boolean;
  /** The node has synced its on-chain registration from a finalized block. */
  provider_info_loaded: boolean;
  /** The synced registration is in its deregister-announcement window. */
  deregistering: boolean;
}

export interface ProviderNodeInfo {
  provider_id?: string;
  readiness: ProviderNodeReadiness;
  /**
   * The provider's on-chain registration as the node currently sees it; `null`
   * until `readiness.provider_info_loaded`.
   */
  provider_registration_info: {
    settings: { price_per_byte: string | number | bigint };
  } | null;
}

/**
 * GET the provider node's `/info`: readiness flags plus the on-chain
 * registration it has synced. The node syncs chain state asynchronously and
 * rejects `/negotiate` with `503 ChainStateNotReady` until ready, so callers
 * that register-then-negotiate should gate on this (see `ensureProviderRegistered`).
 */
export async function getProviderNodeInfo(providerUrl: string): Promise<ProviderNodeInfo> {
  return providerFetch(providerUrl, "/info");
}

export interface PutChunkResult {
  hash: string;
  cid: Uint8Array;
  size: bigint;
  data: Uint8Array;
}

/**
 * PUT a single chunk to the provider without requesting an MMR commitment.
 * Suitable for S3-style object uploads where the Layer 1 metadata records
 * the CID itself and no Layer 0 checkpoint follows immediately.
 *
 * `signer` authenticates the `PUT /node` request; it must hold a Writer/Admin
 * role on `bucketId` (the provider always enforces this).
 *
 * Throws `RangeError` for a chunk above `DEFAULT_CHUNK_SIZE`, before any request.
 */
export async function putChunk(
  providerUrl: string,
  bucketId: bigint | number,
  data: Uint8Array | string,
  signer: ChainSigner,
): Promise<PutChunkResult> {
  const bytes = data instanceof Uint8Array ? data : new TextEncoder().encode(data);
  if (bytes.length > DEFAULT_CHUNK_SIZE) {
    throw new RangeError(
      `chunk is ${bytes.length} bytes, above the ${DEFAULT_CHUNK_SIZE} byte maximum; split the data into ${DEFAULT_CHUNK_SIZE} byte chunks`,
    );
  }
  const cid = hashLeaf(bytes);
  const hash = toHex(cid);
  await providerFetch(providerUrl, "/node", {
    method: "PUT",
    body: { bucket_id: Number(bucketId), hash, data: bytesToBase64(bytes) },
    sign: { signer: signer.signer, bucketId },
  });
  return { hash, cid, size: BigInt(bytes.length), data: bytes };
}

/**
 * PUT the internal node over the child hashes `left` and `right`. Both
 * children must already exist on the provider. Returns the node hash.
 *
 * `signer` authenticates the `PUT /node` request; it must hold a Writer/Admin
 * role on `bucketId`.
 */
export async function putInternalNode(
  providerUrl: string,
  bucketId: bigint | number,
  left: Uint8Array,
  right: Uint8Array,
  signer: ChainSigner,
): Promise<{ hash: string; cid: Uint8Array }> {
  const cid = hashChildren(left, right);
  const hash = toHex(cid);
  await providerFetch(providerUrl, "/node", {
    method: "PUT",
    body: { bucket_id: Number(bucketId), hash, children: [toHex(left), toHex(right)] },
    sign: { signer: signer.signer, bucketId },
  });
  return { hash, cid };
}

/**
 * PUT a chunk to the provider and request an MMR commitment. Returns the
 * chunk hash, original bytes, and the /commit response (mmr_root,
 * leaf_indices, start_seq, provider_signature).
 *
 * `signer` authenticates the `PUT /node` and `POST /commit` requests; it must
 * hold a Writer/Admin role on `bucketId` (the provider always enforces this).
 *
 * Throws `RangeError` for a leaf above `DEFAULT_CHUNK_SIZE`, before any request.
 */
export async function uploadChunk(
  providerUrl: string,
  bucketId: bigint | number,
  data: Uint8Array | string,
  signer: ChainSigner,
): Promise<{ hash: string; data: Uint8Array; commit: any }> {
  const { hash, data: bytes } = await putChunk(providerUrl, bucketId, data, signer);
  const commit = await providerFetch(providerUrl, "/commit", {
    method: "POST",
    body: { bucket_id: Number(bucketId), data_roots: [hash] },
    sign: { signer: signer.signer, bucketId },
  });
  return { hash, data: bytes, commit };
}

/**
 * Download a node by hash and verify it: a chunk's bytes must hash as a leaf,
 * and an internal node's two children must hash as a node over them. Returns
 * `{ data }` for a chunk or `{ children: [left, right] }` for an internal node.
 * Throws {@link CidMismatchError} otherwise.
 */
export async function downloadNode(
  providerUrl: string,
  bucketId: bigint | number,
  hashHex: string,
): Promise<TreeNode> {
  const body = await providerFetch(providerUrl, "/node", { params: { bucket_id: bucketId, hash: hashHex } });
  const expected = asHex(hashHex).toLowerCase();
  const node = parseNode(body);
  if (!node) throw new CidMismatchError(expected, "malformed node");
  const actual = toHex(nodeHash(node));
  if (actual !== expected) throw new CidMismatchError(expected, actual);
  return node;
}

/**
 * Download a chunk by hash and verify it. Throws {@link CidMismatchError} when
 * the bytes do not hash to `chunkHashHex`, and `Error` when the hash names an
 * internal node; use {@link downloadNode} for those.
 */
export async function downloadChunk(
  providerUrl: string,
  bucketId: bigint | number,
  chunkHashHex: string,
): Promise<Uint8Array> {
  const node = await downloadNode(providerUrl, bucketId, chunkHashHex);
  if (!("data" in node)) {
    throw new Error(`${chunkHashHex} is an internal node, not a chunk; use downloadNode`);
  }
  return node.data;
}

/**
 * `signer` authenticates the request — `GET /checkpoint-signature` is
 * Reader-gated on private buckets (anonymous works only while the bucket is
 * public), so pass the member fetching the signature.
 */
export async function fetchCheckpointSignature(
  providerUrl: string,
  bucketId: bigint | number,
  signer?: ChainSigner,
): Promise<any> {
  return providerFetch(providerUrl, "/checkpoint-signature", {
    params: { bucket_id: bucketId },
    sign: signer ? { signer: signer.signer, bucketId } : undefined,
  });
}

/** The `response` argument of `respond_to_challenge`. */
export type ChallengeResponse = Parameters<
  ParachainApi["tx"]["StorageProvider"]["respond_to_challenge"]
>[0]["response"];

/**
 * Build the response for `respond_to_challenge` by reading the challenge from
 * chain state and fetching the proofs from the provider node: a `Proof` with
 * the chunk, or `ChunkOutOfRange` with the leaf's MMR proof alone when the
 * challenged `chunk_index` is at or past the leaf's chunk count.
 *
 * `signer` authenticates the `GET /mmr_proof` step, which is Reader-gated on
 * private buckets — a bucket member works, and so does the provider's own
 * account (the node grants its own account every Reader check).
 * `GET /chunk_proof` is hash-keyed (a capability read) and stays unsigned.
 */
export async function fetchChallengeProof(
  api: ParachainApi,
  providerUrl: string,
  challengeId: { deadline: number; index: number },
  signer?: ChainSigner,
): Promise<ChallengeResponse> {
  // Best block: a finalized read would lag the just-created challenge.
  // Challenges is a StorageDoubleMap keyed by (deadline, index), so the single
  // challenge is read directly with both keys.
  const challenge = await api.query.StorageProvider.Challenges.getValue(
    challengeId.deadline,
    challengeId.index,
    READ_OPTS,
  );
  if (!challenge)
    throw new Error(
      "Challenge not found: deadline " +
        challengeId.deadline +
        " index " +
        challengeId.index,
    );

  const mmr = await providerFetch(providerUrl, "/mmr_proof", {
    params: {
      bucket_id: challenge.bucket_id,
      leaf_index: challenge.target.leaf_index,
    },
    sign: signer ? { signer: signer.signer, bucketId: challenge.bucket_id } : undefined,
  });
  if (!Array.isArray(mmr.items))
    throw new Error(
      "Provider /mmr_proof response has no `items`: the provider node is older than this client",
    );
  if (typeof mmr.leaf?.leaf_version !== "number")
    throw new Error(
      "Provider /mmr_proof leaf has no `leaf_version`: the provider node is older than this client",
    );
  const mmr_proof = {
    leaf: {
      leaf_version: mmr.leaf.leaf_version,
      data_root: asHex(mmr.leaf.data_root),
      data_size: BigInt(mmr.leaf.data_size),
      total_size: BigInt(mmr.leaf.total_size),
    },
    items: mmr.items.map((h: string) => asHex(h)),
  };
  if (challenge.target.chunk_index >= BigInt(leafChunkCount(mmr.leaf))) {
    return Enum("ChunkOutOfRange", { mmr_proof });
  }

  const chunk = await providerFetch(providerUrl, "/chunk_proof", {
    params: {
      bucket_id: challenge.bucket_id,
      data_root: mmr.leaf.data_root,
      chunk_index: challenge.target.chunk_index,
    },
  });
  if (!Array.isArray(chunk.proof))
    throw new Error(
      "Provider /chunk_proof response `proof` is not a list of hashes: the provider node is older than this client",
    );

  return Enum("Proof", {
    chunk_data: base64ToBytes(chunk.chunk_data),
    mmr_proof,
    chunk_proof: chunk.proof.map((h: string) => asHex(h)),
  });
}

// ── Blobs: upload, commit, read ─────────────────────────────────────────────
// A blob of any size is stored as its 256 KiB chunks plus the internal nodes
// of its Merkle tree (`storage_primitives::merkle_tree`). Its
// CID is the tree root (`computeDataRoot`). Blob reads are unauthenticated:
// a blob's hash and its bucket id are enough to read it (#383/#396).

/** Parallel requests per batch in the blob helpers. */
const BLOB_CONCURRENCY = 4;
/** Chunks requested per `GET /read` call. */
const READ_WINDOW_CHUNKS = 16;
/** Attempts for idempotent requests (`PUT /node` and reads). */
const IDEMPOTENT_RETRIES = 3;

async function runLimited<T>(items: T[], limit: number, fn: (item: T) => Promise<void>): Promise<void> {
  let next = 0;
  const worker = async () => {
    while (next < items.length) await fn(items[next++]);
  };
  await Promise.all(Array.from({ length: Math.min(limit, items.length) }, worker));
}

async function putNode(
  providerUrl: string,
  bucketId: bigint | number,
  hash: Uint8Array,
  node: TreeNode,
  signer: ChainSigner,
  opts: ProviderHttpOpts,
): Promise<void> {
  await providerFetch(providerUrl, "/node", {
    method: "PUT",
    body: {
      bucket_id: Number(bucketId),
      hash: toHex(hash),
      ...("data" in node
        ? { data: bytesToBase64(node.data) }
        : { children: node.children.map((c) => toHex(c)) }),
    },
    sign: { signer: signer.signer, bucketId },
    retries: opts.retries ?? IDEMPOTENT_RETRIES,
    fetch: opts.fetch,
    signal: opts.signal,
  });
}

export interface UploadBlobResult {
  /** The blob's CID (`data_root`), 0x-hex. */
  dataRoot: string;
  size: number;
  /** Leaf hash of each chunk, in order. */
  chunkHashes: Uint8Array[];
}

/**
 * Store a blob of any size with `PUT /node`: every chunk, then the internal
 * nodes of its Merkle tree, bottom-up. Does not commit; pass the
 * returned `dataRoot` to {@link commitDataRoots}.
 *
 * `signer` must hold a Writer or Admin role on `bucketId`. Throws for empty
 * `bytes` before any request: empty data has no chunk tree.
 */
export async function uploadBlob(
  providerUrl: string,
  bucketId: bigint | number,
  bytes: Uint8Array,
  signer: ChainSigner,
  opts: ProviderHttpOpts = {},
): Promise<UploadBlobResult> {
  if (bytes.length === 0) throw new Error("empty data has no chunk tree");
  const chunks = splitChunks(bytes);
  const hashes = chunks.map((c) => computeCid(c));
  const { root, nodes } = merkleTree(hashes);
  const sent = new Set<string>();
  const once = <T>(items: T[], hashOf: (item: T) => Uint8Array): T[] =>
    items.filter((item) => {
      const key = toHex(hashOf(item));
      if (sent.has(key)) return false;
      sent.add(key);
      return true;
    });

  const leaves = once(
    chunks.map((data, i) => ({ data, hash: hashes[i] })),
    (c) => c.hash,
  );
  await runLimited(leaves, BLOB_CONCURRENCY, (c) =>
    putNode(providerUrl, bucketId, c.hash, { data: c.data }, signer, opts),
  );
  // `nodes` is level by level; a level can go up once the one below is stored.
  // A level of `width` hashes has `floor(width / 2)` nodes; an odd last hash
  // moves up unchanged.
  let offset = 0;
  for (let width = hashes.length; width > 1; width = Math.ceil(width / 2)) {
    const count = Math.floor(width / 2);
    const level = once(nodes.slice(offset, offset + count), (n) => n.hash);
    offset += count;
    await runLimited(level, BLOB_CONCURRENCY, (n) =>
      putNode(providerUrl, bucketId, n.hash, { children: [n.left, n.right] }, signer, opts),
    );
  }
  return { dataRoot: toHex(root), size: bytes.length, chunkHashes: hashes };
}

export interface CommitResult {
  mmrRoot: string;
  startSeq: bigint;
  /** Leaves in the bucket's MMR after the commit. */
  leafCount: bigint;
  /** MMR leaf index of each committed data root, in request order. */
  leafIndices: bigint[];
  providerSignature: string;
}

/**
 * `POST /commit`: append `dataRoots` to the bucket's MMR as new leaves, in
 * order. Each root must already be stored. Not retried: a retry after a lost
 * response would append the leaves twice.
 *
 * `signer` must hold a Writer or Admin role on `bucketId`.
 */
export async function commitDataRoots(
  providerUrl: string,
  bucketId: bigint | number,
  dataRoots: (string | Uint8Array)[],
  signer: ChainSigner,
  opts: Omit<ProviderHttpOpts, "retries"> = {},
): Promise<CommitResult> {
  const res = await providerFetch(providerUrl, "/commit", {
    method: "POST",
    body: { bucket_id: Number(bucketId), data_roots: dataRoots.map((r) => asHex(r)) },
    sign: { signer: signer.signer, bucketId },
    fetch: opts.fetch,
    signal: opts.signal,
  });
  return {
    mmrRoot: res.mmr_root,
    startSeq: BigInt(res.start_seq),
    leafCount: BigInt(res.leaf_count),
    leafIndices: (res.leaf_indices as (number | string)[]).map((i) => BigInt(i)),
    providerSignature: res.provider_signature,
  };
}

export interface BucketCommitment {
  mmrRoot: Uint8Array;
  startSeq: bigint;
  leafCount: bigint;
  providerSignature: string;
}

/**
 * `GET /commitment`: the provider's current MMR commitment for the bucket.
 * Returns `null` when the provider has no data for the bucket yet (404).
 *
 * The read is Reader-gated on a private bucket: pass a member as `opts.signer`.
 * Anonymous works only while the bucket is public.
 */
export async function getCommitment(
  providerUrl: string,
  bucketId: bigint | number,
  opts: ProviderHttpOpts & { signer?: ChainSigner } = {},
): Promise<BucketCommitment | null> {
  let res: any;
  try {
    res = await providerFetch(providerUrl, "/commitment", {
      params: { bucket_id: bucketId },
      sign: opts.signer ? { signer: opts.signer.signer, bucketId } : undefined,
      retries: opts.retries ?? IDEMPOTENT_RETRIES,
      fetch: opts.fetch,
      signal: opts.signal,
    });
  } catch (err) {
    if (err instanceof HttpError && err.status === 404) return null;
    throw err;
  }
  return {
    mmrRoot: hexToBytes(res.mmr_root),
    startSeq: BigInt(res.start_seq),
    leafCount: BigInt(res.leaf_count),
    providerSignature: res.provider_signature,
  };
}

/**
 * `GET /mmr_proof`: the MMR leaf at `leafIndex` (counted from the bucket's
 * current `start_seq`) and its inclusion proof. Check it with
 * `verifyMmrProof` against a trusted MMR root. Reader-gated like
 * {@link getCommitment}: pass a member as `opts.signer` on a private bucket.
 */
export async function getMmrProof(
  providerUrl: string,
  bucketId: bigint | number,
  leafIndex: bigint | number,
  opts: ProviderHttpOpts & { signer?: ChainSigner } = {},
): Promise<MmrProof> {
  const res = await providerFetch(providerUrl, "/mmr_proof", {
    params: { bucket_id: bucketId, leaf_index: leafIndex },
    sign: opts.signer ? { signer: opts.signer.signer, bucketId } : undefined,
    retries: opts.retries ?? IDEMPOTENT_RETRIES,
    fetch: opts.fetch,
    signal: opts.signal,
  });
  if (!Array.isArray(res.items))
    throw new Error("Provider /mmr_proof response has no `items`: the provider node is older than this client");
  if (typeof res.leaf?.leaf_version !== "number")
    throw new Error("Provider /mmr_proof leaf has no `leaf_version`: the provider node is older than this client");
  return {
    leaf: {
      leafVersion: res.leaf.leaf_version,
      dataRoot: hexToBytes(res.leaf.data_root),
      dataSize: BigInt(res.leaf.data_size),
      totalSize: BigInt(res.leaf.total_size),
    },
    items: (res.items as string[]).map(hexToBytes),
  };
}

/** Thrown when a provider returns data that does not match the requested blob. */
export class BlobVerificationError extends Error {
  constructor(dataRoot: string, reason: string) {
    super(`Blob ${dataRoot} failed verification: ${reason}`);
    this.name = "BlobVerificationError";
  }
}

/** Thrown when a blob is larger than `ReadBlobOpts.maxSize`. */
export class BlobTooLargeError extends BlobVerificationError {
  constructor(dataRoot: string, reason: string) {
    super(dataRoot, reason);
    this.name = "BlobTooLargeError";
  }
}

export interface ReadBlobOpts extends ProviderHttpOpts {
  /**
   * The blob's size in bytes, when known. With a size, the chunks come from
   * `GET /read` with Merkle proofs; without one, from a `GET /node` walk of
   * the blob's tree. Both paths verify every chunk.
   */
  size?: bigint | number;
  /**
   * Largest blob in bytes to accept. The `GET /node` walk stops at a tree
   * deeper than a blob of this size needs and at chunks that add up to more
   * than this; a `size` above it fails before any request.
   */
  maxSize?: bigint | number;
}

/**
 * Read a whole blob by its CID and check it: every chunk must hash to its
 * place in the blob's Merkle tree with root `dataRoot`, and every
 * chunk except the last must be full. Throws {@link BlobVerificationError}
 * on a mismatch.
 */
export async function readBlob(
  providerUrl: string,
  bucketId: bigint,
  dataRoot: string | Uint8Array,
  opts: ReadBlobOpts = {},
): Promise<Uint8Array> {
  const root = typeof dataRoot === "string" ? hexToBytes(dataRoot) : dataRoot;
  if (opts.maxSize !== undefined && opts.size !== undefined && BigInt(opts.size) > BigInt(opts.maxSize)) {
    throw new BlobTooLargeError(toHex(root), `size ${opts.size} exceeds the limit of ${opts.maxSize} bytes`);
  }
  const chunks =
    opts.size === undefined
      ? await walkBlob(providerUrl, bucketId, root, opts)
      : await readBlobWithProofs(providerUrl, bucketId, root, opts.size, opts);
  const rootHex = toHex(root);
  chunks.forEach((c, i) => {
    const last = i === chunks.length - 1;
    if (last ? c.length > DEFAULT_CHUNK_SIZE : c.length !== DEFAULT_CHUNK_SIZE) {
      throw new BlobVerificationError(rootHex, `chunk ${i} has ${c.length} bytes`);
    }
  });
  return concatBytes(...chunks);
}

/** `GET /read` in windows; each chunk is checked against its Merkle proof. */
async function readBlobWithProofs(
  providerUrl: string,
  bucketId: bigint,
  root: Uint8Array,
  size: bigint | number,
  opts: ProviderHttpOpts,
): Promise<Uint8Array[]> {
  const rootHex = toHex(root);
  const total = chunkCount(size);
  const out: Uint8Array[] = [];
  for (let start = 0; start < total; start += READ_WINDOW_CHUNKS) {
    const count = Math.min(READ_WINDOW_CHUNKS, total - start);
    const res = await providerFetch(providerUrl, "/read", {
      params: {
        bucket_id: bucketId,
        data_root: rootHex,
        offset: BigInt(start) * BigInt(DEFAULT_CHUNK_SIZE),
        length: BigInt(count) * BigInt(DEFAULT_CHUNK_SIZE),
      },
      retries: opts.retries ?? IDEMPOTENT_RETRIES,
      fetch: opts.fetch,
      signal: opts.signal,
    });
    const got = res.chunks as { data: string; proof: string[] }[];
    if (got.length !== count) {
      throw new BlobVerificationError(rootHex, `provider returned ${got.length} of ${count} chunks at ${start}`);
    }
    got.forEach((c, k) => {
      const data = base64ToBytes(c.data);
      const siblings = c.proof.map(hexToBytes);
      if (!verifyChunkProof(computeCid(data), start + k, siblings, root, total)) {
        throw new BlobVerificationError(rootHex, `chunk ${start + k} does not match its proof`);
      }
      out.push(data);
    });
  }
  const expected = BigInt(size);
  const actual = out.reduce((n, c) => n + BigInt(c.length), 0n);
  if (actual !== expected) {
    throw new BlobVerificationError(rootHex, `expected ${expected} bytes, got ${actual}`);
  }
  return out;
}

/**
 * Walk the blob's tree with `GET /node`. Every node must hash to its id (a
 * chunk as a leaf, an internal node over its two children), and the chunks
 * must rebuild `root`.
 */
async function walkBlob(providerUrl: string, bucketId: bigint, root: Uint8Array, opts: ReadBlobOpts): Promise<Uint8Array[]> {
  const rootHex = toHex(root);
  const limit = createLimiter(BLOB_CONCURRENCY);
  const maxSize = opts.maxSize === undefined ? undefined : BigInt(opts.maxSize);
  // A tree over n chunks has depth ceil(log2(n)).
  let maxDepth = Number.POSITIVE_INFINITY;
  if (maxSize !== undefined) {
    const maxChunks = chunkCount(maxSize);
    maxDepth = 0;
    while (2 ** maxDepth < maxChunks) maxDepth++;
  }
  let received = 0n;
  const walk = async (hash: Uint8Array, depth: number): Promise<Uint8Array[]> => {
    const hashHex = toHex(hash);
    if (depth > maxDepth) {
      throw new BlobTooLargeError(rootHex, `tree is deeper than a blob of ${maxSize} bytes`);
    }
    const body = await limit(() =>
      providerFetch(providerUrl, "/node", {
        params: { bucket_id: bucketId, hash: hashHex },
        retries: opts.retries ?? IDEMPOTENT_RETRIES,
        fetch: opts.fetch,
        signal: opts.signal,
      }),
    );
    const node = parseNode(body);
    if (!node || !bytesEq(nodeHash(node), hash)) {
      throw new BlobVerificationError(rootHex, `node ${hashHex} does not match its hash`);
    }
    if ("data" in node) {
      received += BigInt(node.data.length);
      if (maxSize !== undefined && received > maxSize) {
        throw new BlobTooLargeError(rootHex, `blob exceeds the limit of ${maxSize} bytes`);
      }
      return [node.data];
    }
    const parts = await Promise.all(node.children.map((c) => walk(c, depth + 1)));
    return parts.flat();
  };
  const chunks = await walk(root, 0);
  if (!bytesEq(merkleRoot(chunks.map((c) => computeCid(c))), root)) {
    throw new BlobVerificationError(rootHex, "chunks do not rebuild the root");
  }
  return chunks;
}
