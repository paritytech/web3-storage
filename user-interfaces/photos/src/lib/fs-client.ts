// SPDX-License-Identifier: GPL-3.0-only
//
// Browser `/fs` client — the M6 port of `scripts/lib/fs-client.ts`. Drives the
// provider's path-based directory API (mkdir / put / get / ls / index_root) over
// `httpFetch`, exactly like drive-ui's `drive-client.ts`. No auth headers: dev
// providers run `/fs` auth disabled, and the Writer grant from `createLibrary`
// (M5) covers the auth-enabled case (signed `/fs` requests are M8 scope).
//
// `resolveFsContext` turns a `driveId` into the `{ providerUrl, bucketId }` the
// `/fs` calls need, reusing the same chain reads as drive-ui:
//   driveId → DriveRegistry.Drives.bucket_id → resolveProviderEndpoint(bucket).

import { httpFetch, resolveProviderEndpoint, type ParachainApi } from '@web3-storage/papi'
import { computeDataRoot, metadataMerkleRoot, type MerkleEntry } from '@/lib/merkle'

/** A resolved drive's `/fs` endpoint + bucket — cached per library in state. */
export interface FsContext {
  providerUrl: string
  bucketId: bigint
}

/** Parsed `PUT /fs/{bucketId}/file` response. */
export interface PutFileResult {
  /** Provider-computed content root, `0x`-prefixed lowercase hex. */
  dataRoot: `0x${string}`
  size: number
  leafIndex: number
}

/** One `GET /fs/{bucketId}/ls` entry. Note: the listing carries no `data_root`. */
export interface LsEntry {
  name: string
  path: string
  entryType: 'file' | 'directory'
  size: number
  mtime: number
}

/** `GET /fs/{bucketId}/index_root` response (used only as a sanity cross-check). */
export interface IndexRoot {
  bucketId: number
  metadataMerkleRoot: `0x${string}`
  fileCount: number
  dirCount: number
  totalSize: number
}

/**
 * Resolve a drive's `/fs` context from chain state. Mirrors drive-ui's
 * `getDrive` (`DriveRegistry.Drives` → `bucket_id`) + `getProviderUrl`
 * (`resolveProviderEndpoint`).
 */
export async function resolveFsContext(api: ParachainApi, driveId: bigint): Promise<FsContext> {
  const drive = await api.query.DriveRegistry.Drives.getValue(driveId)
  if (!drive) throw new Error(`Drive ${driveId} not found on chain`)
  const bucketId = drive.bucket_id as bigint
  const providerUrl = await resolveProviderEndpoint(api, bucketId)
  return { providerUrl, bucketId }
}

function fsBase({ providerUrl, bucketId }: FsContext): string {
  return `${providerUrl}/fs/${bucketId}`
}

/** `POST …/mkdir?path=` — create a directory (album). Idempotent on existing dirs. */
export async function mkdir(ctx: FsContext, path: string): Promise<void> {
  const res = await httpFetch(`${fsBase(ctx)}/mkdir?path=${encodeURIComponent(path)}`, {
    method: 'POST',
  })
  if (!res.ok) throw new Error(`mkdir ${path} failed: ${res.status} ${await res.text().catch(() => '')}`)
}

/** `PUT …/file?path=` — write a blob; returns the provider-computed `data_root`. */
export async function putFile(
  ctx: FsContext,
  path: string,
  data: Uint8Array,
  contentType = 'application/octet-stream',
): Promise<PutFileResult> {
  const res = await httpFetch(`${fsBase(ctx)}/file?path=${encodeURIComponent(path)}`, {
    method: 'PUT',
    headers: { 'content-type': contentType },
    body: data as Uint8Array<ArrayBuffer>,
  })
  if (!res.ok) throw new Error(`PUT ${path} failed: ${res.status} ${await res.text().catch(() => '')}`)
  const json = (await res.json()) as { data_root: `0x${string}`; size: number | string; leaf_index: number | string }
  return { dataRoot: json.data_root, size: Number(json.size), leafIndex: Number(json.leaf_index) }
}

/** `GET …/file?path=` — read a blob's raw bytes. */
export async function downloadFile(ctx: FsContext, path: string): Promise<Uint8Array> {
  const res = await httpFetch(`${fsBase(ctx)}/file?path=${encodeURIComponent(path)}`)
  if (!res.ok) throw new Error(`GET ${path} failed: ${res.status}`)
  return new Uint8Array(await res.arrayBuffer())
}

/** `GET …/file?path=` — read a blob's bytes *and* its stored content type (for re-PUT on rename). */
export async function downloadFileWithType(
  ctx: FsContext,
  path: string,
): Promise<{ bytes: Uint8Array; contentType: string }> {
  const res = await httpFetch(`${fsBase(ctx)}/file?path=${encodeURIComponent(path)}`)
  if (!res.ok) throw new Error(`GET ${path} failed: ${res.status}`)
  const contentType = res.headers.get('content-type') || 'application/octet-stream'
  return { bytes: new Uint8Array(await res.arrayBuffer()), contentType }
}

/** `DELETE …/file?path=` — remove a path from the FS index (the blob lingers in the MMR). */
export async function deleteFile(ctx: FsContext, path: string): Promise<void> {
  const res = await httpFetch(`${fsBase(ctx)}/file?path=${encodeURIComponent(path)}`, {
    method: 'DELETE',
  })
  if (!res.ok) throw new Error(`DELETE ${path} failed: ${res.status} ${await res.text().catch(() => '')}`)
}

/** `GET …/ls?path=&recursive=` — list entries (optionally the full subtree). */
export async function listDir(ctx: FsContext, path: string, recursive = false): Promise<LsEntry[]> {
  const params = new URLSearchParams({ path, recursive: String(recursive) })
  const res = await httpFetch(`${fsBase(ctx)}/ls?${params.toString()}`)
  if (!res.ok) throw new Error(`ls ${path} failed: ${res.status}`)
  const json = (await res.json()) as { entries?: Array<Record<string, unknown>> }
  return (json.entries ?? []).map((e) => ({
    name: String(e.name),
    path: String(e.path),
    entryType: (e.entry_type as 'file' | 'directory') ?? 'file',
    size: Number(e.size ?? 0),
    mtime: Number(e.mtime ?? 0),
  }))
}

/** `GET …/index_root` — the provider's view of the drive's metadata root. */
export async function indexRoot(ctx: FsContext): Promise<IndexRoot> {
  const res = await httpFetch(`${fsBase(ctx)}/index_root`)
  if (!res.ok) throw new Error(`index_root failed: ${res.status}`)
  const json = (await res.json()) as Record<string, unknown>
  return {
    bucketId: Number(json.bucket_id),
    metadataMerkleRoot: json.metadata_merkle_root as `0x${string}`,
    fileCount: Number(json.file_count),
    dirCount: Number(json.dir_count),
    totalSize: Number(json.total_size),
  }
}

/** Per-file content root + size, cached across a session so a re-anchor needn't re-download. */
export interface CachedDataRoot {
  dataRoot: Uint8Array
  size: bigint
}

/**
 * Recompute the drive's metadata Merkle root from a fresh recursive listing.
 * Directories contribute a zero root; each file's `data_root` is taken from
 * `cache` (seeded by `putFile` responses we locally verified) and only
 * downloaded + re-hashed when absent — reproducing the headless
 * `enumerateEntries` semantics without re-downloading just-uploaded photos.
 */
export async function recomputeRoot(
  ctx: FsContext,
  cache: Map<string, CachedDataRoot>,
): Promise<Uint8Array> {
  const listing = await listDir(ctx, '/', true)
  const entries: MerkleEntry[] = await Promise.all(
    listing.map(async (e): Promise<MerkleEntry> => {
      if (e.entryType !== 'file') return { path: e.path, dataRoot: new Uint8Array(32), size: 0n }
      const cached = cache.get(e.path)
      if (cached) return { path: e.path, dataRoot: cached.dataRoot, size: cached.size }
      const bytes = await downloadFile(ctx, e.path)
      const dataRoot = computeDataRoot(bytes)
      cache.set(e.path, { dataRoot, size: BigInt(e.size) })
      return { path: e.path, dataRoot, size: BigInt(e.size) }
    }),
  )
  return metadataMerkleRoot(entries)
}
