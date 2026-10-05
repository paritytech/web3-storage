// SPDX-License-Identifier: Apache-2.0

/**
 * Content-addressing verification. CIDs in this system are the Merkle leaf
 * hash of the chunk bytes (`blake2b-256(0x00 ++ data)`); a single-chunk blob's
 * data_root equals its chunk hash
 * (see the single-leaf case in crates/providers/storage/src/backend/mod.rs
 * `build_padded_merkle_tree`), so whole payloads up to
 * DEFAULT_CHUNK_SIZE can be verified directly against an on-chain CID.
 * Multi-chunk payloads need a Merkle DAG walk (Rust-client parity) — not
 * implemented here; callers surface those as "unverified".
 */

import { blake2b256 } from "@polkadot-labs/hdkd-helpers";

import { asHex, toHex } from "./bytes.js";

/** Mirror of DEFAULT_CHUNK_SIZE in crates/primitives/storage/src/lib.rs. */
export const DEFAULT_CHUNK_SIZE = 256 * 1024;

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

/** Prefix of a Merkle leaf preimage; internal nodes use a different prefix. */
const LEAF_PREFIX = 0x00;

/**
 * Hash a Merkle leaf: `blake2b-256(0x00 ++ data)`. Mirrors `hash_leaf` in
 * crates/primitives/storage/src/lib.rs. The prefix keeps the bytes of an
 * internal node from hashing to the same value as a leaf.
 */
export function hashLeaf(data: Uint8Array): Uint8Array {
  const preimage = new Uint8Array(1 + data.length);
  preimage[0] = LEAF_PREFIX;
  preimage.set(data, 1);
  return blake2b256(preimage);
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
