// SPDX-License-Identifier: Apache-2.0

/**
 * In-memory provider node for unit tests. Implements the Layer 0 routes the
 * blob helpers and the file-system tree use (`PUT/GET /node`, `POST
 * /commit`, `GET /read`, `GET /commitment`, `GET /mmr_proof`) with the same
 * checks and JSON shapes as `provider-node/src/api.rs`. Signatures are not
 * verified; a write only needs an `Authorization` header. Test-only: not
 * exported from the package.
 */

import {
  base64ToBytes,
  bytesToBase64,
  computeCid,
  DEFAULT_CHUNK_SIZE,
  hashChildren,
  hexToBytes,
  toHex,
  u64le,
} from "@web3-storage/core";

const ZERO = toHex(new Uint8Array(32));

interface StoredNode {
  data: Uint8Array;
  children: string[] | null;
}

interface Leaf {
  dataRoot: string;
  dataSize: bigint;
  totalSize: bigint;
}

export interface FakeCall {
  method: string;
  path: string;
  query: Record<string, string>;
  body?: any;
}

function json(status: number, body: unknown): Response {
  return new Response(JSON.stringify(body), { status, headers: { "content-type": "application/json" } });
}

function leafHash(leaf: Leaf): Uint8Array {
  const bytes = new Uint8Array(48);
  bytes.set(hexToBytes(leaf.dataRoot), 0);
  bytes.set(u64le(leaf.dataSize), 32);
  bytes.set(u64le(leaf.totalSize), 40);
  return computeCid(bytes);
}

/** Levels of a perfect binary tree over `leaves` (length a power of two), bottom-up. */
function levels(leaves: Uint8Array[]): Uint8Array[][] {
  const out = [leaves];
  while (out[out.length - 1].length > 1) {
    const prev = out[out.length - 1];
    const next: Uint8Array[] = [];
    for (let i = 0; i < prev.length; i += 2) next.push(hashChildren(prev[i], prev[i + 1]));
    out.push(next);
  }
  return out;
}

/** MMR over `hashes` as in `crates/providers/storage/src/mmr.rs`: peaks of perfect subtrees, largest first. */
function mmrPeaks(hashes: Uint8Array[]): { start: number; size: number; levels: Uint8Array[][] }[] {
  const out = [];
  let start = 0;
  let remaining = hashes.length;
  while (remaining > 0) {
    let size = 1;
    while (size * 2 <= remaining) size *= 2;
    out.push({ start, size, levels: levels(hashes.slice(start, start + size)) });
    start += size;
    remaining -= size;
  }
  return out;
}

function bag(peaks: Uint8Array[]): Uint8Array {
  let acc: Uint8Array | null = null;
  for (let i = peaks.length - 1; i >= 0; i--) acc = acc ? hashChildren(peaks[i], acc) : peaks[i];
  return acc ?? new Uint8Array(32);
}

export class FakeProvider {
  readonly nodes = new Map<string, StoredNode>();
  readonly buckets = new Map<string, Leaf[]>();
  readonly calls: FakeCall[] = [];

  /** Replace a stored node's data without changing its key (simulates a bad provider). */
  tamper(hash: string, data: Uint8Array): void {
    const node = this.nodes.get(hash.toLowerCase());
    if (!node) throw new Error(`no node ${hash}`);
    node.data = data;
  }

  /** Data roots committed to a bucket, in leaf order. */
  committed(bucketId: bigint | number): string[] {
    return (this.buckets.get(String(bucketId)) ?? []).map((l) => l.dataRoot);
  }

  readonly fetch = (async (input: RequestInfo | URL, init?: RequestInit): Promise<Response> => {
    const url = new URL(String(input));
    const method = (init?.method ?? "GET").toUpperCase();
    const query = Object.fromEntries(url.searchParams.entries());
    const body = init?.body ? JSON.parse(String(init.body)) : undefined;
    this.calls.push({ method, path: url.pathname, query, body });
    const headers = new Headers(init?.headers);
    const authed = (headers.get("authorization") ?? "").startsWith("Web3Storage ");
    const route = `${method} ${url.pathname}`;
    switch (route) {
      case "PUT /node":
        return authed ? this.putNode(body) : json(401, { error: "unauthorized" });
      case "POST /commit":
        return authed ? this.commit(body) : json(401, { error: "unauthorized" });
      case "GET /node":
        return this.getNode(query.hash);
      case "GET /read":
        return this.read(query.data_root, BigInt(query.offset), BigInt(query.length));
      case "GET /commitment":
        return this.commitment(query.bucket_id);
      case "GET /mmr_proof":
        return this.mmrProof(query.bucket_id, Number(query.leaf_index));
      default:
        return json(404, { error: `no route ${route}` });
    }
  }) as typeof fetch;

  private putNode(body: { bucket_id: number; hash: string; data: string; children: string[] | null }): Response {
    const hash = body.hash.toLowerCase();
    const data = base64ToBytes(body.data);
    if (toHex(computeCid(data)) !== hash) return json(400, { error: "invalid_hash" });
    const children = body.children?.map((c) => c.toLowerCase()) ?? null;
    const missing = (children ?? []).filter((c) => c !== ZERO && !this.nodes.has(c));
    if (missing.length) return json(400, { error: "children_missing", details: { missing } });
    if (!this.buckets.has(String(body.bucket_id))) this.buckets.set(String(body.bucket_id), []);
    if (!this.nodes.has(hash)) this.nodes.set(hash, { data, children });
    return json(200, { stored: true });
  }

  private treeSize(root: string): bigint {
    let size = 0n;
    const stack = [root];
    while (stack.length) {
      const node = this.nodes.get(stack.pop()!);
      if (!node) continue;
      if (node.children) stack.push(...node.children);
      else size += BigInt(node.data.length);
    }
    return size;
  }

  private commit(body: { bucket_id: number; data_roots: string[] }): Response {
    const roots = body.data_roots.map((r) => r.toLowerCase());
    const missing = roots.find((r) => !this.nodes.has(r));
    if (missing) return json(404, { error: "root_not_found", details: { data_root: missing } });
    const leaves = this.buckets.get(String(body.bucket_id));
    if (!leaves) return json(404, { error: "bucket_not_found" });
    const start = leaves.length;
    for (const dataRoot of roots) {
      const dataSize = this.treeSize(dataRoot);
      const totalSize = (leaves[leaves.length - 1]?.totalSize ?? 0n) + dataSize;
      leaves.push({ dataRoot, dataSize, totalSize });
    }
    return json(200, {
      mmr_root: toHex(this.mmrRoot(leaves)),
      start_seq: 0,
      leaf_count: leaves.length,
      leaf_indices: roots.map((_, i) => start + i),
      provider_signature: "0x00",
    });
  }

  private mmrRoot(leaves: Leaf[]): Uint8Array {
    return bag(mmrPeaks(leaves.map(leafHash)).map((p) => p.levels[p.levels.length - 1][0]));
  }

  private getNode(hash: string): Response {
    const node = this.nodes.get(hash.toLowerCase());
    if (!node) return json(404, { error: "not_found" });
    return json(200, { hash, data: bytesToBase64(node.data), children: node.children });
  }

  /** Chunk hashes under `root` in order, skipping zero hashes (`collect_chunk_hashes`). */
  private chunkHashes(root: string): string[] {
    const out: string[] = [];
    const stack = [root];
    while (stack.length) {
      const hash = stack.pop()!;
      if (hash === ZERO) continue;
      const node = this.nodes.get(hash);
      if (!node) continue;
      if (node.children) stack.push(...[...node.children].reverse());
      else out.push(hash);
    }
    return out;
  }

  private read(dataRoot: string, offset: bigint, length: bigint): Response {
    const hashes = this.chunkHashes(dataRoot.toLowerCase());
    const size = BigInt(DEFAULT_CHUNK_SIZE);
    const start = Number(offset / size);
    const end = Number((offset + length + size - 1n) / size);
    let padded = 1;
    while (padded < hashes.length) padded *= 2;
    const tree = levels([
      ...hashes.map(hexToBytes),
      ...Array.from({ length: padded - hashes.length }, () => new Uint8Array(32)),
    ]);
    const chunks = [];
    for (let i = start; i < end && i < hashes.length; i++) {
      const proof: string[] = [];
      let idx = i;
      for (const level of tree.slice(0, -1)) {
        proof.push(toHex(level[idx ^ 1]));
        idx >>= 1;
      }
      const data = this.nodes.get(hashes[i])!.data;
      chunks.push({ hash: toHex(computeCid(data)), data: bytesToBase64(data), proof });
    }
    return json(200, { chunks });
  }

  private commitment(bucketId: string): Response {
    const leaves = this.buckets.get(bucketId);
    if (!leaves) return json(404, { error: "bucket_not_found" });
    return json(200, {
      bucket_id: Number(bucketId),
      mmr_root: toHex(this.mmrRoot(leaves)),
      start_seq: 0,
      leaf_count: leaves.length,
      provider_signature: "0x00",
    });
  }

  private mmrProof(bucketId: string, leafIndex: number): Response {
    const leaves = this.buckets.get(bucketId);
    if (!leaves) return json(404, { error: "bucket_not_found" });
    const leaf = leaves[leafIndex];
    if (!leaf) return json(404, { error: "not_found" });
    const peaks = mmrPeaks(leaves.map(leafHash));
    const peak = peaks.find((p) => leafIndex >= p.start && leafIndex < p.start + p.size)!;
    const siblings: string[] = [];
    const path: boolean[] = [];
    let idx = leafIndex - peak.start;
    for (const level of peak.levels.slice(0, -1)) {
      siblings.push(toHex(level[idx ^ 1]));
      path.push(idx % 2 === 1);
      idx >>= 1;
    }
    return json(200, {
      leaf: { data_root: leaf.dataRoot, data_size: Number(leaf.dataSize), total_size: Number(leaf.totalSize) },
      proof: { peaks: peaks.map((p) => toHex(p.levels[p.levels.length - 1][0])), siblings, path },
    });
  }
}
