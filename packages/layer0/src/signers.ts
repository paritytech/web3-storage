// SPDX-License-Identifier: Apache-2.0

/**
 * Sr25519 key derivation and signer construction, PAPI-native (hdkd — no
 * @polkadot/* packages, no async cryptoWaitReady).
 */

import { sr25519CreateDerive } from "@polkadot-labs/hdkd";
import {
  DEV_PHRASE,
  entropyToMiniSecret,
  mnemonicToEntropy,
} from "@polkadot-labs/hdkd-helpers";
import { getPolkadotSigner, type PolkadotSigner } from "polkadot-api/signer";

import { toSs58 } from "./address.js";

export interface Keypair {
  publicKey: Uint8Array;
  sign(input: Uint8Array): Uint8Array;
}

/**
 * A signer bundle: the PAPI `PolkadotSigner` plus the identity details every
 * caller ends up needing alongside it. Pallet wrappers take this shape so
 * call sites never juggle `(signer, address, publicKey)` triples.
 */
export interface ChainSigner {
  signer: PolkadotSigner;
  address: string;
  publicKey: Uint8Array;
  /** SURI the signer was derived from, when known (e.g. "//Alice"). */
  seed?: string;
  /** Dev-account name, when this is one of the well-known dev signers. */
  name?: DevAccountName;
  /**
   * Raw keypair, when locally derived. Required for provider request signing
   * (raw sr25519 over the auth message — PolkadotSigner.signBytes may wrap).
   * The provider always enforces auth, so a signer without a raw keypair
   * (e.g. a wallet-extension signer) cannot sign provider requests and its
   * uploads are rejected.
   */
  keypair?: Keypair;
}

/**
 * Derive an sr25519 keypair from a SURI. Mirrors @polkadot/keyring's
 * `addFromUri` grammar so existing seeds keep producing identical addresses:
 * `//Alice` (dev phrase + path), `<mnemonic>`, or `<mnemonic>//path`.
 */
export function seedToKeypair(seed: string): Keypair {
  const trimmed = seed.trim();
  let mnemonic: string;
  let derivationPath: string;
  if (trimmed.startsWith("//")) {
    mnemonic = DEV_PHRASE;
    derivationPath = trimmed;
  } else {
    const sepIdx = trimmed.indexOf("//");
    if (sepIdx === -1) {
      mnemonic = trimmed;
      derivationPath = "";
    } else {
      mnemonic = trimmed.slice(0, sepIdx).trim();
      derivationPath = trimmed.slice(sepIdx);
    }
  }
  const entropy = mnemonicToEntropy(mnemonic);
  const miniSecret = entropyToMiniSecret(entropy);
  const derive = sr25519CreateDerive(miniSecret);
  return derive(derivationPath);
}

/** Build a {@link ChainSigner} from a SURI (e.g. "//Alice"). */
export function makeSigner(seed: string): ChainSigner {
  const keypair = seedToKeypair(seed);
  return {
    signer: getPolkadotSigner(keypair.publicKey, "Sr25519", (input) =>
      keypair.sign(input),
    ),
    address: toSs58(keypair.publicKey),
    publicKey: keypair.publicKey,
    seed,
    keypair,
  };
}

export type DevAccountName =
  | "Alice"
  | "Bob"
  | "Charlie"
  | "Dave"
  | "Eve"
  | "Ferdie";

export interface DevSigner extends ChainSigner {
  name: DevAccountName;
  seed: string;
}

const NAMES: DevAccountName[] = [
  "Alice",
  "Bob",
  "Charlie",
  "Dave",
  "Eve",
  "Ferdie",
];

function build(name: DevAccountName): DevSigner {
  return { ...makeSigner(`//${name}`), name, seed: `//${name}` };
}

export const devSigners: Record<DevAccountName, DevSigner> = Object.fromEntries(
  NAMES.map((n) => [n, build(n)]),
) as Record<DevAccountName, DevSigner>;

export const Alice = devSigners.Alice;
export const Bob = devSigners.Bob;
export const Charlie = devSigners.Charlie;
export const Dave = devSigners.Dave;
export const Eve = devSigners.Eve;
export const Ferdie = devSigners.Ferdie;

export function getDevSigner(name: DevAccountName): DevSigner {
  return devSigners[name];
}
