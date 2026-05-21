import { sr25519CreateDerive } from "@polkadot-labs/hdkd";
import {
  DEV_PHRASE,
  entropyToMiniSecret,
  mnemonicToEntropy,
} from "@polkadot-labs/hdkd-helpers";
import { getPolkadotSigner, type PolkadotSigner } from "polkadot-api/signer";
import { fromBufferToBase58 } from "@polkadot-api/substrate-bindings";

// System parachain `SS58_PREFIX` is 0
const SS58_PREFIX = 0;
const toSs58 = fromBufferToBase58(SS58_PREFIX);

const entropy = mnemonicToEntropy(DEV_PHRASE);
const miniSecret = entropyToMiniSecret(entropy);
const derive = sr25519CreateDerive(miniSecret);

export type DevAccountName =
  | "Alice"
  | "Bob"
  | "Charlie"
  | "Dave"
  | "Eve"
  | "Ferdie";

export interface DevSigner {
  name: DevAccountName;
  address: string;
  publicKey: Uint8Array;
  signer: PolkadotSigner;
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
  const keypair = derive(`//${name}`);
  return {
    name,
    address: toSs58(keypair.publicKey),
    publicKey: keypair.publicKey,
    signer: getPolkadotSigner(keypair.publicKey, "Sr25519", (input) =>
      keypair.sign(input),
    ),
  };
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
