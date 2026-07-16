// Lower paseo storage-backed timeout params live via `sudo.sudo(system.setStorage(...))`.
//
// These four params are declared as `parameter_types! { pub storage X }` in
// runtimes/web3-storage-paseo/src/storage.rs, so they are read from unhashed
// storage at key `twox_128(b":X:")`, falling back to the in-wasm default. Writing
// the keys below overrides the defaults with no runtime upgrade.
//
// Usage (on toaster):
//   export PASEO_WS="wss://<paseo-previewnet-rpc>"
//   export SUDO_SURI="//Alice"          # the chain's sudo key SURI
//   node scripts/paseo-lower-timeouts-setstorage.mjs
//
import { createClient } from "polkadot-api";
import { getWsProvider } from "polkadot-api/ws-provider";
import { getPolkadotSigner } from "polkadot-api/signer";
import { Binary } from "polkadot-api";
import { sr25519CreateDerive } from "@polkadot-labs/hdkd";
import {
  DEV_PHRASE,
  entropyToMiniSecret,
  mnemonicToEntropy,
} from "@polkadot-labs/hdkd-helpers";

// twox_128(b":Name:") keys + SCALE u32-LE values for the lowered timeouts.
const ITEMS = [
  ["ChallengeTimeout", "0xf9d872bee99dc2ce8d32b673102353a4", "0x0a000000"], // 10
  ["SettlementTimeout", "0x97b9a1825097a548d5a5699955279fdf", "0x05000000"], // 5
  ["RequestTimeout", "0x730dd35d9a030a9a567a2fb5fffca445", "0x03000000"], // 3
  ["DeregisterAnnouncementPeriod", "0x5803c94d7fc04f403fe3ad457090ef3f", "0x0a000000"], // 10
];

const WS = process.env.PASEO_WS ?? "ws://127.0.0.1:2222";
const SURI = process.env.SUDO_SURI ?? "//Alice";

const deriveSr25519 = sr25519CreateDerive(
  entropyToMiniSecret(mnemonicToEntropy(DEV_PHRASE)),
);
const kp = deriveSr25519(SURI);
const signer = getPolkadotSigner(kp.publicKey, "Sr25519", kp.sign);

const client = createClient(getWsProvider(WS));
// Generated descriptors for the paseo runtime; adjust the import name to match
// whatever `npx papi add` registered for this endpoint.
const { paseo } = await import("@polkadot-api/descriptors");
const api = client.getTypedApi(paseo);

const items = ITEMS.map(([, k, v]) => [Binary.fromHex(k), Binary.fromHex(v)]);
const inner = api.tx.System.set_storage({ items });
const call = api.tx.Sudo.sudo({ call: inner.decodedCall });

console.log("Submitting sudo.setStorage for:", ITEMS.map((i) => i[0]).join(", "));
const result = await call.signAndSubmit(signer);
console.log("Included in block:", result.block.hash, "ok:", result.ok);
client.destroy();
