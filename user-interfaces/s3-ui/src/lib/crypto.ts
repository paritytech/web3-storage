// SPDX-License-Identifier: GPL-3.0-only

/**
 * PAPI-native crypto helpers: seed -> sr25519 keypair, SS58 encode/validate.
 *
 * Replaces @polkadot/keyring + @polkadot/util-crypto. URI parsing mirrors
 * @polkadot/keyring's addFromUri so dev-account seeds (//Alice etc.) keep
 * producing identical addresses.
 */
import { sr25519CreateDerive } from "@polkadot-labs/hdkd";
import {
  DEV_PHRASE,
  entropyToMiniSecret,
  mnemonicToEntropy,
} from "@polkadot-labs/hdkd-helpers";
import { getSs58AddressInfo } from "@polkadot-api/substrate-bindings";

// SS58 encoding (prefix + helper) lives in the shared PAPI package so every UI
// renders addresses with the same, runtime-derived prefix.
export { toSs58 } from "@web3-storage/papi";

export interface Keypair {
  publicKey: Uint8Array;
  sign(input: Uint8Array): Uint8Array;
}

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

export function isValidSs58(address: string): boolean {
  return getSs58AddressInfo(address).isValid;
}
