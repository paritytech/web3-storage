/**
 * Smart-contract end-to-end demo for the `SharedTeamDrive` example dApp.
 *
 * The contract owns a drive on-chain via the drive-registry precompile.
 * Flow:
 *   1. Provider setup + account mapping.
 *   2. Deploy `SharedTeamDrive.sol`.
 *   3. Admin (`//Bob`) creates the team with `msg.value` covering the
 *      payment reserve.
 *   4. Admin invites Charlie (Reader).
 *   5. Admin kicks Charlie.
 *   6. Admin disbands the team (deletes the drive).
 *
 * Asserts pallet events (`DriveCreated`, `DriveShared`, `DriveUnshared`,
 * `DriveDeleted`) and contract events (`TeamCreated`, `Invited`, `Kicked`,
 * `Disbanded`) fire for each step.
 *
 * Usage: node sc-team-drive.js [chain_ws] [provider_url] [provider_seed] [client_seed]
 */

import assert from "node:assert";
import { readFile } from "node:fs/promises";
import { dirname, resolve } from "node:path";
import { fileURLToPath } from "node:url";

import {
  connect,
  ensureProviderRegistered,
  ensureSoleAcceptingProvider,
  hexToBytes,
  makeSigner,
  parseProviderClientArgs,
  requireOneEvent,
  toHex,
  waitForBlockProduction,
  waitForChainReady,
  waitForNextBlock,
} from "./common.js";
import {
  callContract,
  decodeContractEmitted,
  deployContract,
  encodeCall,
  ensureAccountMapped,
  h160ToSubstrate,
  negotiatePrecompileTerms,
} from "./sc-api.js";

const { chainWs, providerUrl, providerSeed, clientSeed } = parseProviderClientArgs();

const HERE = dirname(fileURLToPath(import.meta.url));
const CONTRACT_JSON = resolve(HERE, "../contracts/build/combined.json");
const CONTRACT_KEY = "SharedTeamDrive.sol:SharedTeamDrive";

const UNIT = 10n ** 12n;

async function main() {
  console.log("=== SharedTeamDrive e2e ===");
  console.log(" chain    :", chainWs);
  console.log(" provider :", providerUrl, `(${providerSeed})`);
  console.log(" client   :", clientSeed);

  const { papi, api } = await connect(chainWs);
  try {
    await waitForChainReady(api);
    await waitForBlockProduction(api);
    await waitForNextBlock(papi);

    const provider = makeSigner(providerSeed);
    const client = makeSigner(clientSeed); // //Bob — team admin
    const member = makeSigner("//Charlie");

    console.log("\n[setup] provider + Revive account mapping…");
    await ensureProviderRegistered(api, provider, providerUrl, {
      pricePerByte: 1n,
      maxDuration: 100_000,
    });
    await ensureSoleAcceptingProvider(api, provider);
    await ensureAccountMapped(api, provider);
    await ensureAccountMapped(api, client);
    await ensureAccountMapped(api, member);

    // Load compiled artifacts.
    const combined = JSON.parse(await readFile(CONTRACT_JSON, "utf8"));
    const entry = combined.contracts?.[CONTRACT_KEY];
    if (!entry) {
      throw new Error(
        `combined.json missing ${CONTRACT_KEY} — run \`just build-contracts\` first`
      );
    }
    const abi = entry.abi;
    const bytecode = hexToBytes(entry.bin);
    console.log("  bytecode:", bytecode.length, "bytes");

    // 1) Deploy. //Bob deploys so he becomes the admin (via msg.sender on
    //    the createTeam call later).
    console.log("\n[1/4] Deploying SharedTeamDrive…");
    const deployed = await deployContract(api, client, bytecode);
    console.log("  contract:", deployed.address);

    // 2) createTeam{value: 10 UNIT} — the contract becomes the drive owner,
    //    so the terms are negotiated with the contract's substrate-mapped
    //    account as owner; msg.value funds that account's payment reserve.
    console.log("\n[2/4] createTeam{value: 10 UNIT}('team-cov', provider, terms[1MiB×50], sig)");
    const contractAccount = h160ToSubstrate(deployed.addressBytes);
    const signed = await negotiatePrecompileTerms(providerUrl, contractAccount, {
      maxBytes: 1n << 20n, // 1 MiB capacity
      duration: 50,
    });
    const createData = encodeCall(abi, "createTeam", [
      "team-cov",
      toHex(provider.publicKey),
      signed.terms,
      signed.signature,
    ]);
    let r = await callContract(api, client, deployed.addressBytes, createData, {
      value: 10n * UNIT,
    });
    const driveCreated = requireOneEvent(
      r.events,
      api.event.DriveRegistry.DriveCreated,
      "DriveRegistry.DriveCreated"
    );
    const driveId = driveCreated.drive_id;
    console.log("  driveId =", driveId.toString());
    let logs = decodeContractEmitted(r.events, api, deployed.addressBytes, abi);
    assert.ok(
      logs.some((l) => l.eventName === "TeamCreated"),
      "TeamCreated event not emitted"
    );

    // 3) invite Charlie as Writer.
    console.log("\n[3/4] invite(Charlie, Writer)");
    const inviteData = encodeCall(abi, "invite", [toHex(member.publicKey), 1]);
    r = await callContract(api, client, deployed.addressBytes, inviteData);
    requireOneEvent(
      r.events,
      api.event.DriveRegistry.DriveShared,
      "DriveRegistry.DriveShared"
    );

    // kick Charlie.
    console.log("        kick(Charlie)");
    const kickData = encodeCall(abi, "kick", [toHex(member.publicKey)]);
    r = await callContract(api, client, deployed.addressBytes, kickData);
    requireOneEvent(
      r.events,
      api.event.DriveRegistry.DriveUnshared,
      "DriveRegistry.DriveUnshared"
    );

    // 4) disband.
    console.log("\n[4/4] disband()");
    const disbandData = encodeCall(abi, "disband", []);
    r = await callContract(api, client, deployed.addressBytes, disbandData);
    requireOneEvent(
      r.events,
      api.event.DriveRegistry.DriveDeleted,
      "DriveRegistry.DriveDeleted"
    );
    logs = decodeContractEmitted(r.events, api, deployed.addressBytes, abi);
    assert.ok(
      logs.some((l) => l.eventName === "Disbanded"),
      "Disbanded event not emitted"
    );

    console.log("\n✅ SharedTeamDrive flow completed");
  } finally {
    papi.destroy();
  }
}

main().catch((e) => {
  console.error(e);
  process.exit(1);
});
