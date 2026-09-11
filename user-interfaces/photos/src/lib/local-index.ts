// SPDX-License-Identifier: GPL-3.0-only
//
// A client-maintained mirror of a drive's entry set: the source of truth for the
// anchored metadata Merkle root, updated on upload/edit/delete so the root never
// needs a full re-download. File leaves are keyed on locally verified byte length,
// never a provider-reported size, so the anchor commits only to what the client saw.
//
// Single implementation shared across the photos package: the browser app imports
// it as `@/lib/local-index`, and the headless flow (`scripts/photos-flow.ts`)
// cross-imports this same file. Kept in-package (not hoisted to the SDK) on purpose
// — it's contingent on the current rebuild-from-scratch metadata tree and would be
// largely replaced by proof-based root updates against a position-stable
// authenticated map. Its only dependency is `@web3-storage/sdk`, which both the app
// and the scripts already use.
//
// `root()` returns raw bytes: the browser anchor path (`rootToBytes32`) consumes
// them directly, and the script hex-encodes via `toHex`. Serialization
// (`toJSON`/`fromJSON`) backs the browser's per-drive IndexedDB persistence; the
// script uses the index purely in-memory.

import { hexToBytes, metadataMerkleRoot, toHex, type MerkleEntry } from '@web3-storage/sdk'

const ZERO_ROOT = new Uint8Array(32)

interface IndexEntry extends MerkleEntry {
  isDir: boolean
}

/** A file entry's contribution to the root — the shape `recomputeRoot` seeds/reads. */
export interface CachedDataRoot {
  dataRoot: Uint8Array
  size: bigint
}

/** The serialized form of one entry (bytes → hex, bigint → decimal string). */
export interface SerializedIndexEntry {
  path: string
  isDir: boolean
  dataRoot: `0x${string}`
  size: string
}

/** Client-maintained drive index. Persisted per-drive; revalidated against the anchor. */
export class LocalIndex {
  private readonly byPath = new Map<string, IndexEntry>()

  /** Record an uploaded or edited file (same path = replace). */
  setFile(path: string, dataRoot: Uint8Array, size: bigint): void {
    this.byPath.set(path, { path, dataRoot, size, isDir: false })
  }

  /** Record a created directory (zero data root, zero size). */
  setDir(path: string): void {
    this.byPath.set(path, { path, dataRoot: ZERO_ROOT, size: 0n, isDir: true })
  }

  remove(path: string): void {
    this.byPath.delete(path)
  }

  /** A file entry's cached content root + size, or undefined if absent (or a directory). */
  getFile(path: string): CachedDataRoot | undefined {
    const e = this.byPath.get(path)
    return e && !e.isDir ? { dataRoot: e.dataRoot, size: e.size } : undefined
  }

  /** Paths of files only (directories excluded). */
  filePaths(): string[] {
    return [...this.byPath.values()].filter((e) => !e.isDir).map((e) => e.path)
  }

  /** All entries as `MerkleEntry[]` (`metadataMerkleRoot` sorts them). */
  entries(): MerkleEntry[] {
    return [...this.byPath.values()].map((e) => ({ path: e.path, dataRoot: e.dataRoot, size: e.size }))
  }

  /** The drive's metadata Merkle root over the current entry set, as raw bytes. */
  root(): Uint8Array {
    return metadataMerkleRoot(this.entries())
  }

  toJSON(): SerializedIndexEntry[] {
    return [...this.byPath.values()].map((e) => ({
      path: e.path,
      isDir: e.isDir,
      dataRoot: toHex(e.dataRoot) as `0x${string}`,
      size: e.size.toString(),
    }))
  }

  static fromJSON(data: SerializedIndexEntry[]): LocalIndex {
    const index = new LocalIndex()
    for (const e of data) {
      if (e.isDir) index.setDir(e.path)
      else index.setFile(e.path, hexToBytes(e.dataRoot), BigInt(e.size))
    }
    return index
  }
}
