// SPDX-License-Identifier: GPL-3.0-only
//
// Headless M1 flow: deploy Photos → negotiate terms → createLibrary → assert.
// Mirrors `examples/papi/sc-team-drive.js` but for the Photos contract.
// Later milestones extend this with albums/upload/edit (M2/M3).
//
// Usage: tsx scripts/photos-flow.ts [chain_ws] [provider_url] [provider_seed] [client_seed]
//   defaults: ws://127.0.0.1:2222  http://127.0.0.1:3333  //Alice  //Bob
//
// Preconditions: a running chain + provider node, with the provider already
// registered and `accepting_primary` (e.g. after `just demo`), and a built
// artifact (`build:contract`).

import assert from "node:assert";

import {
  connect,
  isSameAddress,
  makeSigner,
  READ_OPTS,
  requireOneEvent,
  toHex,
  waitForChainReady,
  waitForNextBlock,
} from "@web3-storage/sdk";
import {
  callContract,
  decodeContractEmitted,
  deployContract,
  encodeCall,
  ensureAccountMapped,
  substrateToH160,
} from "@web3-storage/sdk/revive";
import { h160ToSubstrate, negotiatePrecompileTerms } from "./lib/contract.js";
import { loadArtifact, readLibraryOf } from "./lib/photos.js";

// pnpm forwards a literal `--` into argv; drop it.
const args = process.argv.slice(2).filter((a) => a !== "--");
const chainWs = args[0] || "ws://127.0.0.1:2222";
const providerUrl = args[1] || "http://127.0.0.1:3333";
const providerSeed = args[2] || "//Alice";
const clientSeed = args[3] || "//Bob";

const UNIT = 10n ** 12n;
const MAX_BYTES = 1n << 20n; // 1 MiB quota
const DURATION = 50; // blocks
const LIBRARY_NAME = "my-photos";

async function main() {
  console.log("=== Photos M1 flow (createLibrary) ===");
  console.log(" chain    :", chainWs);
  console.log(" provider :", providerUrl, `(${providerSeed})`);
  console.log(" user     :", clientSeed);

  const { abi, bin } = await loadArtifact();
  const { papi, api } = connect(chainWs);
  try {
    await waitForChainReady(api);
    await waitForNextBlock(papi);

    const provider = makeSigner(providerSeed);
    const user = makeSigner(clientSeed);

    // Precondition: provider registered + accepting. Read its locked price.
    const info: any = await api.query.StorageProvider.Providers.getValue(provider.address, READ_OPTS);
    if (!info) {
      throw new Error(`Provider ${providerSeed} (${provider.address}) is not registered. Run \`just demo\` once to register + accept, then retry.`);
    }
    if (!info.settings.accepting_primary) {
      throw new Error(`Provider ${providerSeed} is not accepting_primary. Enable it (updateProviderSettings) and retry.`);
    }
    const pricePerByte: bigint = info.settings.price_per_byte;
    const payment = pricePerByte * MAX_BYTES * BigInt(DURATION);
    const value = payment * 2n + UNIT; // generous buffer; unused reserve stays in the contract (v1)
    console.log(`\n[setup] price_per_byte=${pricePerByte}  payment=${payment}  value=${value}`);

    await ensureAccountMapped(api, provider);
    await ensureAccountMapped(api, user);

    // 1) Deploy. The user deploys + calls, so msg.sender keys their library.
    console.log("\n[1/3] Deploying Photos…");
    const deployed = await deployContract(api, user, bin);
    console.log("  contract:", deployed.address);

    // 2) Negotiate primary terms with the *contract's* mapped account as owner.
    console.log("\n[2/3] Negotiating terms + createLibrary{value}…");
    const contractAccount = h160ToSubstrate(deployed.addressBytes);
    const signed = await negotiatePrecompileTerms(providerUrl, contractAccount, {
      maxBytes: MAX_BYTES,
      duration: DURATION,
      pricePerByte,
    });
    const userAccount = toHex(user.publicKey); // bytes32 substrate AccountId32 → Writer grant
    const createData = encodeCall(abi, "createLibrary", [
      userAccount,
      LIBRARY_NAME,
      toHex(provider.publicKey),
      signed.terms,
      signed.signature,
    ]);
    const r = await callContract(api, user, deployed.addressBytes, createData, { value });

    // 3) Assert on events.
    const driveCreated: any = requireOneEvent(r.events, api.event.DriveRegistry.DriveCreated, "DriveRegistry.DriveCreated");
    const driveShared: any = requireOneEvent(r.events, api.event.DriveRegistry.DriveShared, "DriveRegistry.DriveShared");
    const driveId: bigint = driveCreated.drive_id;
    const bucketId: bigint = driveCreated.bucket_id;
    console.log(`\n[3/3] driveId=${driveId}  bucketId=${bucketId}  owner=${driveCreated.owner}`);

    assert.ok(isSameAddress(driveCreated.owner, contractAccount.address), "drive owner is not the contract account");
    assert.ok(isSameAddress(driveShared.member, user.address), "Writer grant member is not the user");

    const contractLogs = decodeContractEmitted(r.events, api, deployed.address, abi);
    assert.ok(contractLogs.some((l) => l.eventName === "LibraryCreated"), "LibraryCreated event not emitted");

    // Soft cross-check: the user shows up as a member on the underlying bucket.
    const bucket: any = await api.query.StorageProvider.Buckets.getValue(bucketId, READ_OPTS);
    const member = bucket?.members?.find((m: any) => isSameAddress(m.account, user.address));
    console.log("  bucket member role for user:", member ? (member.role?.type ?? member.role) : "(not found)");

    // Unsigned state-detection read (the path the UI uses).
    const lib = await readLibraryOf(api, deployed.addressBytes, substrateToH160(user.publicKey), user.address, abi);
    console.log("  libraryOf(user):", lib);
    assert.ok(lib.exists, "libraryOf.exists is false");
    assert.strictEqual(lib.driveId, driveId, "libraryOf.driveId mismatch");

    console.log("\n✅ Photos M1 flow completed — library created, contract owns the drive, user has Writer access.");
  } finally {
    papi.destroy();
  }
}

main().catch((e) => {
  console.error(e);
  process.exit(1);
});
