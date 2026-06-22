// SPDX-License-Identifier: GPL-3.0-only
//
// Deploy the Photos contract once per network and print its H160 address.
// Usage: tsx scripts/deploy-contract.ts [chain_ws] [deployer_seed]
//   defaults: ws://127.0.0.1:2222  //Alice
//
// Requires a running chain and a built artifact (`build:contract`).

import { connect, makeSigner, waitForChainReady, waitForNextBlock } from "./lib/papi.js";
import { deployContract, ensureAccountMapped } from "./lib/contract.js";
import { loadArtifact } from "./lib/photos.js";

// pnpm forwards a literal `--` into argv; drop it.
const args = process.argv.slice(2).filter((a) => a !== "--");
const chainWs = args[0] || "ws://127.0.0.1:2222";
const deployerSeed = args[1] || "//Alice";

async function main() {
  console.log("=== Deploy Photos contract ===");
  console.log(" chain   :", chainWs);
  console.log(" deployer:", deployerSeed);

  const { abi: _abi, bin } = await loadArtifact();
  const { papi, api } = connect(chainWs);
  try {
    await waitForChainReady(api);
    await waitForNextBlock(papi);

    const deployer = makeSigner(deployerSeed);
    await ensureAccountMapped(api, deployer);

    const deployed = await deployContract(api, deployer, bin);
    console.log("\n✅ Photos deployed");
    console.log("  address (H160):", deployed.address);
    console.log("\nSet this as `photosContract` in the network config.");
  } finally {
    papi.destroy();
  }
}

main().catch((e) => {
  console.error(e);
  process.exit(1);
});
