// SPDX-License-Identifier: GPL-3.0-only

// A client-maintained mirror of a drive's entry set: the source of truth for the
// anchored metadata Merkle root, updated on upload/edit/delete so the root never
// needs a full re-download. File leaves are keyed on locally verified byte length,
// never a provider-reported size, so the anchor commits only to what the client saw.

import { metadataMerkleRoot, toHex, type MerkleEntry } from "@web3-storage/sdk";

const ZERO_ROOT = new Uint8Array(32);

interface IndexEntry extends MerkleEntry {
  isDir: boolean;
}

/** In-memory drive index. Not persisted — rebuilt per process run. */
export class LocalIndex {
  private readonly byPath = new Map<string, IndexEntry>();

  /** Record an uploaded or edited file (same path = replace). */
  setFile(path: string, dataRoot: Uint8Array, size: bigint): void {
    this.byPath.set(path, { path, dataRoot, size, isDir: false });
  }

  /** Record a created directory (zero data root, zero size). */
  setDir(path: string): void {
    this.byPath.set(path, { path, dataRoot: ZERO_ROOT, size: 0n, isDir: true });
  }

  remove(path: string): void {
    this.byPath.delete(path);
  }

  /** The entry as it contributes to the Merkle root, or undefined if absent. */
  get(path: string): MerkleEntry | undefined {
    const e = this.byPath.get(path);
    return e && { path: e.path, dataRoot: e.dataRoot, size: e.size };
  }

  /** Paths of files only (directories excluded) — the spot-check candidates. */
  filePaths(): string[] {
    return [...this.byPath.values()].filter((e) => !e.isDir).map((e) => e.path);
  }

  /** All entries as `MerkleEntry[]` (`metadataMerkleRoot` sorts them). */
  entries(): MerkleEntry[] {
    return [...this.byPath.values()].map((e) => ({ path: e.path, dataRoot: e.dataRoot, size: e.size }));
  }

  /** The drive's metadata Merkle root over the current entry set, as `0x` hex. */
  root(): `0x${string}` {
    return toHex(metadataMerkleRoot(this.entries())) as `0x${string}`;
  }
}
