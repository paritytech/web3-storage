// SPDX-License-Identifier: Apache-2.0

/**
 * Content-addressing verification. CIDs in this system are the Merkle leaf
 * hash of the chunk bytes (`blake2b-256(0x00 ++ data)`); a single-chunk blob's
 * data_root equals its chunk hash
 * (see the single-leaf case of `merkle_root` in
 * crates/primitives/storage/src/merkle.rs), so whole payloads up to
 * DEFAULT_CHUNK_SIZE can be verified directly against an on-chain CID.
 * Multi-chunk payloads need a Merkle DAG walk (Rust-client parity) — not
 * implemented here; callers surface those as "unverified".
 */

import { asHex, hexToBytes, toHex } from "./bytes.js";
import { hashChildren, hashLeaf } from "./merkle.js";

export class CidMismatchError extends Error {
  readonly expected: string;
  readonly actual: string;
  constructor(expected: string, actual: string) {
    super(
      `Content does not match its CID (expected ${expected}, got ${actual}) — ` +
        `the provider served corrupted or substituted bytes`,
    );
    this.name = "CidMismatchError";
    this.expected = expected;
    this.actual = actual;
  }
}

/** Content id of `data`: its Merkle leaf hash, which is the data_root of a single-chunk blob. */
export function computeCid(data: Uint8Array): Uint8Array {
  return hashLeaf(data);
}

/** Throw {@link CidMismatchError} unless `data` hashes to `expectedCid`. */
export function verifyCid(data: Uint8Array, expectedCid: string | Uint8Array): void {
  const expected = asHex(expectedCid).toLowerCase();
  const actual = toHex(computeCid(data)).toLowerCase();
  if (expected !== actual) {
    throw new CidMismatchError(expected, actual);
  }
}

/**
 * Hash of a downloaded node: a leaf hash over `data` for a chunk, or a node
 * hash over exactly two `children` for an internal node. Returns null when the
 * node has both or neither, or a wrong child count.
 */
export function nodeHash(data: Uint8Array | null, children: string[] | null): Uint8Array | null {
  if (data && !children) return hashLeaf(data);
  if (children && !data && children.length === 2) {
    const [left, right] = children.map(hexToBytes);
    return hashChildren(left, right);
  }
  return null;
}
