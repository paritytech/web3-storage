/**
 * PAPI-native crypto helpers: seed -> sr25519 keypair, SS58 encode, hex.
 *
 * Replaces the @polkadot/keyring + @polkadot/util(-crypto) trio. URI parsing
 * mirrors @polkadot/keyring's addFromUri so existing dev-account seeds
 * (//Alice etc.) and custom mnemonic entries keep producing identical
 * addresses.
 */
import { sr25519CreateDerive } from "@polkadot-labs/hdkd";
import {
  DEV_PHRASE,
  entropyToMiniSecret,
  mnemonicToEntropy,
} from "@polkadot-labs/hdkd-helpers";

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

export function toHex(bytes: Uint8Array): string {
  let h = "0x";
  for (const b of bytes) h += b.toString(16).padStart(2, "0");
  return h;
}
