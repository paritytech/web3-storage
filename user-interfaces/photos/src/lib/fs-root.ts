// SPDX-License-Identifier: GPL-3.0-only
//
// Client-side drive enumeration + metadata-root recompute over the shared
// FileSystemClient. These aren't part of the SDK: they trust nothing the
// provider claims about content — they download every file and re-hash it
// locally. Kept free of any browser/chain-client coupling (the caller passes
// the FileSystemClient) so the headless scripts can import them too.

import { computeDataRoot, metadataMerkleRoot, type MerkleEntry } from '@web3-storage/sdk'
import type { FileSystemClient } from '@web3-storage/sdk/fs'
import type { LocalIndex } from './local-index'

/**
 * Enumerate the drive's full entry set as `MerkleEntry[]`, trusting nothing the
 * provider claims about content: list the whole tree, then for each file
 * download its bytes and recompute `data_root` locally (directories use a zero
 * root). The input to `metadataMerkleRoot` for client-side verification.
 */
export async function enumerateEntries(
  client: FileSystemClient,
  bucketId: bigint,
): Promise<MerkleEntry[]> {
  const listing = await client.listDirectory(bucketId, '/', { recursive: true })
  return Promise.all(
    listing.map(async (e): Promise<MerkleEntry> => {
      if (e.entryType !== 'file') return { path: e.path, dataRoot: new Uint8Array(32), size: 0n }
      const bytes = await client.downloadFile(bucketId, e.path)
      return { path: e.path, dataRoot: computeDataRoot(bytes), size: BigInt(e.size) }
    }),
  )
}

/**
 * Recompute the drive's metadata Merkle root from a fresh recursive listing, and
 * seed `index` from it. Directories contribute a zero root; each file's `data_root`
 * is taken from the index (seeded by uploads we locally verified) and only
 * downloaded + re-hashed when absent — no re-download of just-uploaded photos.
 *
 * This is the fallback path used when the persisted index doesn't match the
 * on-chain anchor (cold cache or a drive mutated elsewhere). It fully repopulates
 * `index` — files *and* directories — so the caller can then treat it as
 * authoritative and anchor future roots from `index.root()` alone.
 */
export async function recomputeRoot(
  client: FileSystemClient,
  bucketId: bigint,
  index: LocalIndex,
): Promise<Uint8Array> {
  const listing = await client.listDirectory(bucketId, '/', { recursive: true })
  const entries: MerkleEntry[] = await Promise.all(
    listing.map(async (e): Promise<MerkleEntry> => {
      if (e.entryType !== 'file') {
        index.setDir(e.path)
        return { path: e.path, dataRoot: new Uint8Array(32), size: 0n }
      }
      const cached = index.getFile(e.path)
      if (cached) return { path: e.path, dataRoot: cached.dataRoot, size: cached.size }
      const bytes = await client.downloadFile(bucketId, e.path)
      const dataRoot = computeDataRoot(bytes)
      index.setFile(e.path, dataRoot, BigInt(e.size))
      return { path: e.path, dataRoot, size: BigInt(e.size) }
    }),
  )
  return metadataMerkleRoot(entries)
}
