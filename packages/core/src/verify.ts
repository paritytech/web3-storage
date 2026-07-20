// SPDX-License-Identifier: Apache-2.0

/**
 * Content-addressing verification. CIDs in this system are blake2b-256 over
 * the chunk bytes; a single-chunk blob's data_root equals its chunk hash
 * (see provider-node/src/storage single-leaf case), so whole payloads up to
 * DEFAULT_CHUNK_SIZE can be verified directly against an on-chain CID.
 * Multi-chunk payloads need a Merkle DAG walk (Rust-client parity) — not
 * implemented here; callers surface those as "unverified".
 */

import { blake2b256 } from "@polkadot-labs/hdkd-helpers";

import { asHex, toHex } from "./bytes.js";

/** Mirror of CHUNK_SIZE in primitives/src/lib.rs. */
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

/** blake2b-256 content id of `data`. */
export function computeCid(data: Uint8Array): Uint8Array {
  return blake2b256(data);
}

/** Throw {@link CidMismatchError} unless `data` hashes to `expectedCid`. */
export function verifyCid(data: Uint8Array, expectedCid: string | Uint8Array): void {
  const expected = asHex(expectedCid).toLowerCase();
  const actual = toHex(computeCid(data)).toLowerCase();
  if (expected !== actual) {
    throw new CidMismatchError(expected, actual);
  }
}
