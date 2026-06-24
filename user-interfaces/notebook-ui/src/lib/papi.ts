// SPDX-License-Identifier: GPL-3.0-only

/** PAPI chain client + dev-seed signer derivation. */

import { sr25519CreateDerive } from "@polkadot-labs/hdkd";
import {
  DEV_PHRASE,
  entropyToMiniSecret,
  mnemonicToEntropy,
} from "@polkadot-labs/hdkd-helpers";
import { createClient } from "polkadot-api";
import { getWsProvider } from "polkadot-api/ws";
import { getPolkadotSigner } from "polkadot-api/signer";
import { parachain } from "@polkadot-api/descriptors";

export type Signer = ReturnType<typeof getPolkadotSigner>;

const devMiniSecret = entropyToMiniSecret(mnemonicToEntropy(DEV_PHRASE));
const deriveSr25519 = sr25519CreateDerive(devMiniSecret);

export interface SignerInfo {
  signer: Signer;
  publicKey: Uint8Array;
  seed: string;
}

/** Derive a sr25519 signer from a dev SURI (e.g. "//Alice"). Synchronous —
 * no `cryptoWaitReady` needed because hdkd is pure JS. */
export function deriveSigner(seed: string): SignerInfo {
  const keypair = deriveSr25519(seed);
  return {
    signer: getPolkadotSigner(keypair.publicKey, "Sr25519", keypair.sign),
    publicKey: keypair.publicKey,
    seed,
  };
}

export function connectChain(wsUrl: string) {
  const client = createClient(getWsProvider(wsUrl));
  const api = client.getTypedApi(parachain);
  return { client, api };
}
