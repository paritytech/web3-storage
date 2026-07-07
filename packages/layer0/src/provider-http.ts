// SPDX-License-Identifier: Apache-2.0

/**
 * Provider-node HTTP helpers — the off-chain half of flows the pallet
 * wrappers complete. Platform-neutral (btoa/atob-based base64), so browser
 * consumers can typecheck and use these directly.
 */

import { blake2b256 } from "@polkadot-labs/hdkd-helpers";
import { base64ToBytes, bytesToBase64 } from "@web3-storage/core";

import { asHex, toHex, type ParachainApi } from "./address.js";
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
  const resp = await fetch(url, {
    method: opts.method || "GET",
    headers: opts.body ? { "Content-Type": "application/json" } : undefined,
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
  provider_registration_info: any | null;
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
 */
export async function putChunk(
  providerUrl: string,
  bucketId: bigint | number,
  data: Uint8Array | string,
): Promise<PutChunkResult> {
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
  });
  return { hash, cid, size: BigInt(bytes.length), data: bytes };
}

/**
 * PUT a chunk to the provider and request an MMR commitment. Returns the
 * chunk hash, original bytes, and the /commit response (mmr_root,
 * leaf_indices, start_seq, provider_signature).
 */
export async function uploadChunk(
  providerUrl: string,
  bucketId: bigint | number,
  data: Uint8Array | string,
  nonce: bigint | number,
): Promise<{ hash: string; data: Uint8Array; commit: any }> {
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
  });
  // `nonce` is the block at which the caller intends to submit the extrinsic
  // consuming the resulting `provider_signature`. The pallet rejects signatures
  // whose nonce is older than `MaxNonceAge`, so thread it into /commit.
  const commit = await providerFetch(providerUrl, "/commit", {
    method: "POST",
    body: { bucket_id: Number(bucketId), data_roots: [hash], nonce: Number(nonce) },
  });
  return { hash, data: bytes, commit };
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
  nonce: bigint | number,
): Promise<any> {
  // The provider signs the CommitmentPayload including `nonce`; pass the block
  // the caller will submit at so the on-chain recency check passes.
  return providerFetch(providerUrl, "/checkpoint-signature", {
    params: { bucket_id: bucketId, nonce: Number(nonce) },
  });
}

export async function fetchCheckpointDuty(
  providerUrl: string,
  bucketId: bigint | number,
): Promise<any> {
  return providerFetch(providerUrl, "/checkpoint/duty", {
    params: { bucket_id: bucketId },
  });
}

export async function signCheckpointProposal(
  providerUrl: string,
  bucketId: bigint | number,
  duty: { mmr_root: string; start_seq: number | string; leaf_count: number | string },
  window: number | bigint,
): Promise<any> {
  return providerFetch(providerUrl, "/checkpoint/sign", {
    method: "POST",
    body: {
      bucket_id: Number(bucketId),
      mmr_root: duty.mmr_root,
      start_seq: duty.start_seq,
      leaf_count: duty.leaf_count,
      window: Number(window),
    },
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
