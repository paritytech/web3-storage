// SPDX-License-Identifier: Apache-2.0

/**
 * In-memory provider node for unit tests. Implements the Layer 0 routes the
 * blob helpers and the file-system tree use (`PUT/GET /node`, `POST
 * /commit`, `GET /read`, `GET /commitment`, `GET /mmr_proof`) with the same
 * checks and JSON shapes as `provider-node/src/api.rs`. Signatures are not
 * verified; a write and a Reader-gated read (`GET /commitment`, `GET
 * /mmr_proof`) only need an `Authorization` header, as on a private bucket.
 * Test-only: not exported from the package.
 */

import {
  bagPeaks,
  base64ToBytes,
  bytesToBase64,
  computeCid,
  DEFAULT_CHUNK_SIZE,
  hashChildren,
  hexToBytes,
  LEAF_VERSION,
  nodeHash,
  parseNode,
  toHex,
  u64le,
} from "@web3-storage/core";

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
  const bytes = new Uint8Array(49);
  bytes[0] = LEAF_VERSION;
  bytes.set(hexToBytes(leaf.dataRoot), 1);
  bytes.set(u64le(leaf.dataSize), 33);
  bytes.set(u64le(leaf.totalSize), 41);
  return computeCid(bytes);
}

/** Levels of the Merkle tree over `leaves`, bottom-up; an odd last node moves up unchanged. */
function levels(leaves: Uint8Array[]): Uint8Array[][] {
  const out = [leaves];
  while (out[out.length - 1].length > 1) {
    const prev = out[out.length - 1];
    const next: Uint8Array[] = [];
    for (let i = 0; i < prev.length; i += 2) {
      next.push(i + 1 < prev.length ? hashChildren(prev[i], prev[i + 1]) : prev[i]);
    }
    out.push(next);
  }
  return out;
}

/** MMR over `hashes` as in `crates/primitives/storage/src/mmr.rs`: peaks of perfect subtrees, largest first. */
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
        return this.getNode(query);
      case "GET /read":
        return this.read(query);
      case "GET /commitment":
        return authed ? this.commitment(query.bucket_id) : json(401, { error: "unauthorized" });
      case "GET /mmr_proof":
        return authed ? this.mmrProof(query.bucket_id, Number(query.leaf_index)) : json(401, { error: "unauthorized" });
      default:
        return json(404, { error: `no route ${route}` });
    }
  }) as typeof fetch;

  private putNode(body: { bucket_id: number; hash: string; data?: string | null; children?: string[] | null }): Response {
    const hash = body.hash.toLowerCase();
    const parsed = parseNode(body);
    if (!parsed) return json(400, { error: "invalid_node" });
    if (toHex(nodeHash(parsed)) !== hash) return json(400, { error: "invalid_hash" });
    const children = "children" in parsed ? parsed.children.map(toHex) : null;
    const missing = (children ?? []).filter((c) => !this.nodes.has(c));
    if (missing.length) return json(400, { error: "children_missing", details: { missing } });
    if (!this.buckets.has(String(body.bucket_id))) this.buckets.set(String(body.bucket_id), []);
    if (!this.nodes.has(hash)) this.nodes.set(hash, { data: "data" in parsed ? parsed.data : new Uint8Array(0), children });
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
    // Empty data has no chunk tree: a root whose last chunk is empty is rejected.
    const emptyRoot = roots.find((r) => this.nodes.get(this.chunkHashes(r).at(-1)!)!.data.length === 0);
    if (emptyRoot) return json(400, { error: "empty_data", details: { data_root: emptyRoot } });
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
    return bagPeaks(mmrPeaks(leaves.map(leafHash)).map((p) => p.levels[p.levels.length - 1][0]));
  }

  /** `require_bucket`: the query names a bucket, and the provider has data for it. */
  private requireBucket(bucketId: string | undefined): Response | null {
    if (bucketId === undefined) return json(400, { error: "bucket_id_required" });
    return this.buckets.has(bucketId) ? null : json(404, { error: "bucket_not_found" });
  }

  private getNode(query: Record<string, string>): Response {
    const hash = query.hash;
    const bucketError = this.requireBucket(query.bucket_id);
    if (bucketError) return bucketError;
    const node = this.nodes.get(hash.toLowerCase());
    if (!node) return json(404, { error: "not_found" });
    return json(
      200,
      node.children ? { hash, data: null, children: node.children } : { hash, data: bytesToBase64(node.data), children: null },
    );
  }

  /** Chunk hashes under `root` in order (`collect_leaves`). */
  private chunkHashes(root: string): string[] {
    const out: string[] = [];
    const stack = [root];
    while (stack.length) {
      const hash = stack.pop()!;
      const node = this.nodes.get(hash);
      if (!node) continue;
      if (node.children) stack.push(...[...node.children].reverse());
      else out.push(hash);
    }
    return out;
  }

  private read(query: Record<string, string>): Response {
    const bucketError = this.requireBucket(query.bucket_id);
    if (bucketError) return bucketError;
    const dataRoot = query.data_root;
    const offset = BigInt(query.offset);
    const length = BigInt(query.length);
    const hashes = this.chunkHashes(dataRoot.toLowerCase());
    const size = BigInt(DEFAULT_CHUNK_SIZE);
    const start = Number(offset / size);
    const end = Number((offset + length + size - 1n) / size);
    const tree = levels(hashes.map(hexToBytes));
    const chunks = [];
    for (let i = start; i < end && i < hashes.length; i++) {
      const proof: string[] = [];
      let idx = i;
      for (const level of tree.slice(0, -1)) {
        // An odd last node moves up without a sibling.
        if ((idx ^ 1) < level.length) proof.push(toHex(level[idx ^ 1]));
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
    const peakIndex = peaks.findIndex((p) => leafIndex >= p.start && leafIndex < p.start + p.size);
    const peakRoots = peaks.map((p) => p.levels[p.levels.length - 1][0]);
    const items = peakRoots.slice(0, peakIndex);
    let idx = leafIndex - peaks[peakIndex].start;
    for (const level of peaks[peakIndex].levels.slice(0, -1)) {
      items.push(level[idx ^ 1]);
      idx >>= 1;
    }
    if (peakIndex + 1 < peaks.length) items.push(bagPeaks(peakRoots.slice(peakIndex + 1)));
    return json(200, {
      leaf: {
        leaf_version: LEAF_VERSION,
        data_root: leaf.dataRoot,
        data_size: Number(leaf.dataSize),
        total_size: Number(leaf.totalSize),
      },
      items: items.map(toHex),
    });
  }
}
