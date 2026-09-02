// SPDX-License-Identifier: Apache-2.0

/**
 * Provider-node HTTP helpers — the off-chain half of flows the pallet
 * wrappers complete. Platform-neutral (btoa/atob-based base64), so browser
 * consumers can typecheck and use these directly.
 */

import { blake2b256 } from "@polkadot-labs/hdkd-helpers";
import {
  base64ToBytes,
  bytesToBase64,
  signProviderRequest,
  type ProviderRequestSigner,
} from "@web3-storage/core";

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
  /** The replay-nonce counter is bootstrapped, so /negotiate can issue quotes. */
  nonce_counter_ready: boolean;
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
  provider_registration_info: { price_per_byte: string | number | bigint } | null;
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
 */
export async function putChunk(
  providerUrl: string,
  bucketId: bigint | number,
  data: Uint8Array | string,
  signer: ChainSigner,
): Promise<PutChunkResult> {
  const sign = { signer: signer.signer, bucketId };
  const bytes = data instanceof Uint8Array ? data : new TextEncoder().encode(data);
  const cid = blake2b256(bytes);
  const hash = toHex(cid);
  await providerFetch(providerUrl, "/node", {
    method: "PUT",
    body: {
      bucket_id: Number(bucketId),
      hash,
      data: bytesToBase64(bytes),
      children: null,
    },
    sign,
  });
  return { hash, cid, size: BigInt(bytes.length), data: bytes };
}

/**
 * PUT a chunk to the provider and request an MMR commitment. Returns the
 * chunk hash, original bytes, and the /commit response (mmr_root,
 * leaf_indices, start_seq, provider_signature).
 *
 * `signer` authenticates the `PUT /node` and `POST /commit` requests; it must
 * hold a Writer/Admin role on `bucketId` (the provider always enforces this).
 */
export async function uploadChunk(
  providerUrl: string,
  bucketId: bigint | number,
  data: Uint8Array | string,
  signer: ChainSigner,
): Promise<{ hash: string; data: Uint8Array; commit: any }> {
  const sign = { signer: signer.signer, bucketId };
  const bytes = data instanceof Uint8Array ? data : new TextEncoder().encode(data);
  const hash = toHex(blake2b256(bytes));
  await providerFetch(providerUrl, "/node", {
    method: "PUT",
    body: {
      bucket_id: Number(bucketId),
      hash,
      data: bytesToBase64(bytes),
      children: null,
    },
    sign,
  });
  const commit = await providerFetch(providerUrl, "/commit", {
    method: "POST",
    body: { bucket_id: Number(bucketId), data_roots: [hash] },
    sign,
  });
  return { hash, data: bytes, commit };
}

export interface DeleteDataResponse {
  mmr_root: string;
  start_seq: number;
  leaf_count: number;
  provider_signature: string;
  nonce: number;
}

/**
 * POST /delete — Layer 0 prune (Admin only): drops every leaf below
 * `newStartSeq` from the bucket's MMR and returns the provider-signed
 * post-prune commitment. The response is exactly the `ck` argument of
 * `submitClientCheckpoint`. The deletion becomes canonical once that
 * checkpoint lands, and the provider physically erases the pruned bytes
 * (returning the quota headroom) once it also holds the admin's deletion
 * authorization — see `confirmDeletion` and the composed
 * `pruneAndCheckpoint`. Frozen buckets are refused (403).
 */
export async function deleteData(
  providerUrl: string,
  bucketId: bigint | number,
  newStartSeq: bigint | number,
  signer: ChainSigner,
): Promise<DeleteDataResponse> {
  return providerFetch(providerUrl, "/delete", {
    method: "POST",
    body: {
      bucket_id: Number(bucketId),
      new_start_seq: Number(newStartSeq),
    },
    sign: { signer: signer.signer, bucketId },
  });
}

/**
 * SCALE-encode the deletion `CommitmentPayload` the pallet's `Deleted`
 * challenge defense verifies: `version(1) | bucket_id u64 LE | mmr_root |
 * start_seq u64 LE | leaf_count u64 LE (pinned 0)` — 57 bytes.
 */
export function encodeDeletionPayload(
  bucketId: bigint | number,
  mmrRootHex: string,
  newStartSeq: bigint | number,
): Uint8Array {
  const out = new Uint8Array(57);
  const view = new DataView(out.buffer);
  out[0] = 1; // CommitmentPayload::CURRENT_VERSION
  view.setBigUint64(1, BigInt(bucketId), true);
  const root = mmrRootHex.startsWith("0x") ? mmrRootHex.slice(2) : mmrRootHex;
  for (let i = 0; i < 32; i++) out[9 + i] = parseInt(root.substr(i * 2, 2), 16);
  view.setBigUint64(41, BigInt(newStartSeq), true);
  view.setBigUint64(49, 0n, true); // leaf_count pinned to 0 for deletions
  return out;
}

/**
 * POST /delete/confirm — hand the provider the admin's signed deletion
 * authorization (the `Deleted` challenge defense). Requires a raw-signing
 * keypair on the signer: wallet-extension signers wrap messages in
 * `<Bytes>…</Bytes>`, which the pallet's verifier does not accept.
 */
export async function confirmDeletion(
  providerUrl: string,
  bucketId: bigint | number,
  ck: { mmr_root: string; start_seq: number | string },
  admin: ChainSigner,
): Promise<void> {
  if (!admin.keypair) {
    throw new Error(
      "confirmDeletion requires a raw-signing keypair (admin.keypair); " +
        "wallet-extension signers cannot produce the pallet's deletion signature",
    );
  }
  const payload = encodeDeletionPayload(bucketId, ck.mmr_root, BigInt(ck.start_seq));
  const raw = admin.keypair.sign(payload);
  // SCALE MultiSignature: variant 1 = Sr25519, then the 64 signature bytes.
  const multi = new Uint8Array(65);
  multi[0] = 1;
  multi.set(raw, 1);
  await providerFetch(providerUrl, "/delete/confirm", {
    method: "POST",
    body: {
      bucket_id: Number(bucketId),
      mmr_root: ck.mmr_root,
      new_start_seq: Number(ck.start_seq),
      admin: toHex(admin.publicKey),
      signature: toHex(multi),
    },
    sign: { signer: admin.signer, bucketId },
  });
}

export interface BucketUsage {
  /**
   * Bytes physically on the provider's disk and charged to this bucket,
   * including pruned data stashed until its checkpoint lands and the
   * deletion receipt is held. Decreases when the GC physically erases.
   */
  usedBytes: bigint;
  /** Quota from the chain agreement (meaningful only when `quotaSynced`). */
  maxBytes: bigint;
  /**
   * False while the provider still reports the unlimited sentinel
   * (`u64::MAX`), i.e. it has not yet synced a quota from an agreement.
   */
  quotaSynced: boolean;
}

/**
 * Read one bucket's usage against its paid quota from `GET /buckets`.
 * Throws if the provider does not know the bucket.
 */
export async function fetchBucketUsage(
  providerUrl: string,
  bucketId: bigint | number,
): Promise<BucketUsage> {
  const resp = await providerFetch(providerUrl, "/buckets");
  const bucket = (resp.buckets ?? []).find(
    (b: any) => BigInt(b.bucket_id) === BigInt(bucketId),
  );
  if (!bucket) throw new Error(`bucket ${bucketId} not found on provider`);
  // u64::MAX survives JSON as an imprecise float; anything >= 2^63 can only
  // be the sentinel (real quotas are far below 2^53, where JSON is exact).
  const quotaSynced = Number(bucket.max_bytes) < 2 ** 63;
  return {
    usedBytes: BigInt(bucket.used_bytes),
    maxBytes: BigInt(bucket.max_bytes),
    quotaSynced,
  };
}

export async function downloadChunk(
  providerUrl: string,
  chunkHashHex: string,
): Promise<Uint8Array> {
  const downloaded = await providerFetch(providerUrl, "/node", {
    params: { hash: chunkHashHex },
  });
  return base64ToBytes(downloaded.data);
}

export async function fetchCheckpointSignature(
  providerUrl: string,
  bucketId: bigint | number,
): Promise<any> {
  return providerFetch(providerUrl, "/checkpoint-signature", {
    params: { bucket_id: bucketId },
  });
}

/**
 * Build the proof payload for `respond_to_challenge` by reading the challenge
 * from chain state and fetching MMR + chunk proofs from the provider node.
 */
export async function fetchChallengeProof(
  api: ParachainApi,
  providerUrl: string,
  challengeId: { deadline: number; index: number },
): Promise<any> {
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
  });
  const chunk = await providerFetch(providerUrl, "/chunk_proof", {
    params: {
      data_root: mmr.leaf.data_root,
      chunk_index: challenge.target.chunk_index,
    },
  });

  return {
    chunk_data: base64ToBytes(chunk.chunk_data),
    mmr_proof: {
      peaks: mmr.proof.peaks.map((h: string) => asHex(h)),
      leaf: {
        data_root: asHex(mmr.leaf.data_root),
        data_size: BigInt(mmr.leaf.data_size),
        total_size: BigInt(mmr.leaf.total_size),
      },
      leaf_proof: {
        siblings: mmr.proof.siblings.map((h: string) => asHex(h)),
        path: mmr.proof.path,
      },
    },
    chunk_proof: {
      siblings: chunk.proof.siblings.map((h: string) => asHex(h)),
      path: chunk.proof.path,
    },
  };
}
