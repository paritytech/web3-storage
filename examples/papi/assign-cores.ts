// SPDX-License-Identifier: Apache-2.0

// Assign N coretime cores to a parachain on the relay chain via sudo.
//
// Usage:
//   node --import tsx assign-cores.ts <relay_ws> <para_id> <num_cores> [seed]
//
// Defaults: ws://127.0.0.1:9900, 4000, 3, //Alice.
//
// Mirrors what `polkadot-bulletin-chain`'s `examples/assign_cores.js` does for
// the Bulletin testnet: sudo-batches `Coretime::assign_core(core_idx, 0, [(Task(paraId), 57600)], None)`
// for each core. Required because zombienet's `num_cores=N` only sets the
// scheduler-params budget — actual assignment of cores to a para still needs
// a `Coretime::assign_core` extrinsic on the relay.
//
// Uses PAPI's untyped API (`client.getUnsafeApi()`) so we don't need to
// pre-generate relay-chain descriptors. The shape of the call is discovered
// from the running chain's metadata.

import { createClient } from "polkadot-api";
import { getWsProvider } from "polkadot-api/ws";
import { getPolkadotSigner } from "polkadot-api/signer";
import { sr25519CreateDerive } from "@polkadot-labs/hdkd";
import {
  DEV_PHRASE,
  entropyToMiniSecret,
  mnemonicToEntropy,
  ss58Address,
} from "@polkadot-labs/hdkd-helpers";

function parseArgs(argv: string[]) {
  return {
    relayWs: argv[2] || "ws://127.0.0.1:9900",
    paraId: Number(argv[3] || 4000),
    numCores: Number(argv[4] || 3),
    seed: argv[5] || "//Alice",
  };
}

function makeSigner(seed: string) {
  const devMiniSecret = entropyToMiniSecret(mnemonicToEntropy(DEV_PHRASE));
  const deriveSr25519 = sr25519CreateDerive(devMiniSecret);
  const keyPair = deriveSr25519(seed);
  return {
    signer: getPolkadotSigner(keyPair.publicKey, "Sr25519", keyPair.sign),
    address: ss58Address(keyPair.publicKey),
  };
}

async function main() {
  const { relayWs, paraId, numCores, seed } = parseArgs(process.argv);
  console.log(`Assigning ${numCores} coretime cores to para ${paraId} via ${relayWs} (sudo: ${seed})`);

  const client = createClient(getWsProvider(relayWs));
  const api = client.getUnsafeApi();
  const { signer, address } = makeSigner(seed);
  console.log(`Sudo signer address: ${address}`);

  // Compose the inner calls as their decoded RuntimeCall form so PAPI can
  // re-encode them inside Utility.batch_all / Sudo.sudo without a typed
  // descriptor for the relay chain.
  const innerCalls = Array.from(
    { length: numCores },
    (_, core) =>
      api.tx.Coretime.assign_core({
        core,
        begin: 0,
        assignment: [[{ type: "Task", value: paraId }, 57600]],
        end_hint: undefined,
      }).decodedCall
  );

  const batchAll = api.tx.Utility.batch_all({ calls: innerCalls }).decodedCall;
  const sudoTx = api.tx.Sudo.sudo({ call: batchAll });

  console.log("Submitting sudo(batch_all(assign_core x N))...");
  const result = await sudoTx.signAndSubmit(signer);
  if (!result.ok) {
    console.error("Sudo call failed:");
    console.error(JSON.stringify(result, null, 2));
    process.exit(1);
  }
  console.log(`Included in block ${result.block.hash} at index ${result.block.index}`);
  for (const e of result.events) {
    console.log(`  event: ${e.type}.${e.value.type}`);
  }
  client.destroy();
}

main().catch((e) => {
  console.error(e);
  process.exit(1);
});
