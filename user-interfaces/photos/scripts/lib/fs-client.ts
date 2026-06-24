// SPDX-License-Identifier: GPL-3.0-only
//
// Provider `/fs` operations for the headless flow — a port of the bits of
// `user-interfaces/drive-ui/src/lib/drive-client.ts` (`createDirectory`,
// `uploadFile`, `downloadFile`, `listDirectory`) as standalone functions over
// the provider's path-based API. No auth headers: the local dev provider runs
// `/fs` auth disabled (the M1 Writer grant covers the auth-enabled case,
// exercised by the browser in M6). Reused unchanged by the UI later.

import { httpFetch } from "@web3-storage/sdk";

import { computeDataRoot, type MerkleEntry } from "./merkle.js";

/** Parsed `PUT /fs/{bucketId}/file` response. */
export interface PutFileResult {
  /** Provider-computed content root, `0x`-prefixed lowercase hex. */
  dataRoot: `0x${string}`;
  size: number;
  leafIndex: number;
}

/** One `GET /fs/{bucketId}/ls` entry. Note: the listing carries no `data_root`. */
export interface LsEntry {
  name: string;
  path: string;
  entryType: "file" | "directory";
  size: number;
  mtime: number;
}

/** `GET /fs/{bucketId}/index_root` response (used only as a sanity cross-check). */
export interface IndexRoot {
  bucketId: number;
  metadataMerkleRoot: `0x${string}`;
  fileCount: number;
  dirCount: number;
  totalSize: number;
}

function fsBase(providerUrl: string, bucketId: bigint): string {
  return `${providerUrl}/fs/${bucketId}`;
}

/** `POST …/mkdir?path=` — create a directory (album). */
export async function mkdir(providerUrl: string, bucketId: bigint, path: string): Promise<void> {
  const res = await httpFetch(`${fsBase(providerUrl, bucketId)}/mkdir?path=${encodeURIComponent(path)}`, { method: "POST" });
  if (!res.ok) throw new Error(`mkdir ${path} failed: ${res.status} ${await res.text().catch(() => "")}`);
}

/** `PUT …/file?path=` — write a blob; returns the provider-computed `data_root`. */
export async function putFile(
  providerUrl: string,
  bucketId: bigint,
  path: string,
  data: Uint8Array,
  contentType = "application/octet-stream",
): Promise<PutFileResult> {
  const res = await httpFetch(`${fsBase(providerUrl, bucketId)}/file?path=${encodeURIComponent(path)}`, {
    method: "PUT",
    headers: { "content-type": contentType },
    body: data as Uint8Array<ArrayBuffer>,
  });
  if (!res.ok) throw new Error(`PUT ${path} failed: ${res.status} ${await res.text().catch(() => "")}`);
  const json: any = await res.json();
  return { dataRoot: json.data_root, size: Number(json.size), leafIndex: Number(json.leaf_index) };
}

/** `GET …/file?path=` — read a blob's raw bytes. */
export async function downloadFile(providerUrl: string, bucketId: bigint, path: string): Promise<Uint8Array> {
  const res = await httpFetch(`${fsBase(providerUrl, bucketId)}/file?path=${encodeURIComponent(path)}`);
  if (!res.ok) throw new Error(`GET ${path} failed: ${res.status}`);
  return new Uint8Array(await res.arrayBuffer());
}

/** `GET …/ls?path=&recursive=` — list entries (optionally the full subtree). */
export async function listDir(providerUrl: string, bucketId: bigint, path: string, recursive = false): Promise<LsEntry[]> {
  const params = new URLSearchParams({ path, recursive: String(recursive) });
  const res = await httpFetch(`${fsBase(providerUrl, bucketId)}/ls?${params.toString()}`);
  if (!res.ok) throw new Error(`ls ${path} failed: ${res.status}`);
  const json: any = await res.json();
  return (json.entries ?? []).map((e: any) => ({
    name: e.name,
    path: e.path,
    entryType: e.entry_type as "file" | "directory",
    size: Number(e.size ?? 0),
    mtime: Number(e.mtime ?? 0),
  }));
}

/** `GET …/index_root` — the provider's view of the drive's metadata root. */
export async function indexRoot(providerUrl: string, bucketId: bigint): Promise<IndexRoot> {
  const res = await httpFetch(`${fsBase(providerUrl, bucketId)}/index_root`);
  if (!res.ok) throw new Error(`index_root failed: ${res.status}`);
  const json: any = await res.json();
  return {
    bucketId: Number(json.bucket_id),
    metadataMerkleRoot: json.metadata_merkle_root,
    fileCount: Number(json.file_count),
    dirCount: Number(json.dir_count),
    totalSize: Number(json.total_size),
  };
}

/**
 * Enumerate the drive's full entry set as `MerkleEntry[]`, trusting nothing the
 * provider claims about content: list the whole tree, then for each file
 * download its bytes and recompute `data_root` locally (directories use a zero
 * root). This is the input to `metadataMerkleRoot` for client-side verification.
 */
export async function enumerateEntries(providerUrl: string, bucketId: bigint): Promise<MerkleEntry[]> {
  const listing = await listDir(providerUrl, bucketId, "/", true);
  return Promise.all(
    listing.map(async (e) => {
      if (e.entryType !== "file") return { path: e.path, dataRoot: new Uint8Array(32), size: 0n };
      const bytes = await downloadFile(providerUrl, bucketId, e.path);
      return { path: e.path, dataRoot: computeDataRoot(bytes), size: BigInt(e.size) };
    }),
  );
}
