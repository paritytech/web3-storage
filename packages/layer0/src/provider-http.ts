// SPDX-License-Identifier: Apache-2.0

/**
 * Provider-node HTTP helpers — the off-chain half of flows the pallet
 * wrappers complete. Platform-neutral (btoa/atob-based base64), so browser
 * consumers can typecheck and use these directly.
 */

import {
  base64ToBytes,
  bytesToBase64,
  CidMismatchError,
  DEFAULT_CHUNK_SIZE,
  hashChildren,
  hashLeaf,
  leafChunkCount,
  nodeHash,
  parseNode,
  signProviderRequest,
  type ProviderRequestSigner,
  type TreeNode,
} from "@web3-storage/core";
import { Enum } from "polkadot-api";

import { asHex, toHex, type ParachainApi } from "./address.js";
import type { ChainSigner } from "./signers.js";
import { READ_OPTS } from "./tx.js";

export interface ProviderFetchOpts {
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
  const resp = await fetch(url, {
    method,
    headers: Object.keys(headers).length ? headers : undefined,
    body: opts.body ? JSON.stringify(opts.body) : undefined,
  });
  if (!resp.ok) throw new Error(`${path}: ${resp.status} ${await resp.text()}`);
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
