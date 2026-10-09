// SPDX-License-Identifier: Apache-2.0

/**
 * Smart-contract end-to-end demo for the `SharedTeamDrive` example dApp.
 *
 * The contract owns a drive (a plain Layer 0 bucket) via the
 * storage-provider precompile.
 * Flow:
 *   1. Provider setup + account mapping.
 *   2. Deploy `SharedTeamDrive.sol`.
 *   3. Admin (`//Bob`) creates the team with `msg.value` covering the
 *      payment reserve.
 *   4. Admin invites Charlie (Writer).
 *   5. Admin kicks Charlie.
 *
 * Asserts pallet events (`BucketCreated`, `MemberSet`, `MemberRemoved`) and
 * contract events (`TeamCreated`, `Invited`, `Kicked`) fire for each step.
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
  hexToBytes,
  makeSigner,
  requireOneEvent,
  toHex,
  waitForBlockProduction,
  waitForChainReady,
  waitForNextBlock,
} from "@web3-storage/sdk";
import {
  callContract,
  decodeContractEmitted,
  deployContract,
  encodeCall,
  ensureAccountMapped,
} from "@web3-storage/sdk/revive";
import { h160ToSubstrate, negotiatePrecompileTerms, SolRole } from "./sc-support.js";
import {
  ensureSoleAcceptingProvider,
  parseProviderClientArgs,
} from "./support.js";

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
    const PRICE_PER_BYTE = 1n;
    await ensureProviderRegistered(api, provider, providerUrl, {
      pricePerByte: PRICE_PER_BYTE,
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
    console.log("\n[1/3] Deploying SharedTeamDrive…");
    const deployed = await deployContract(api, client, bytecode);
    console.log("  contract:", deployed.address);
    const assertContractEvent = (events: Parameters<typeof decodeContractEmitted>[0], name: string) =>
      assert.ok(
        decodeContractEmitted(events, api, deployed.addressBytes, abi).some(
          (l) => l.eventName === name
        ),
        `${name} event not emitted`
      );

    // 2) createTeam{value: 10 UNIT} — the contract becomes the bucket admin,
    //    so the terms are negotiated with the contract's substrate-mapped
    //    account as owner; msg.value funds that account's payment reserve.
    console.log("\n[2/3] createTeam{value: 10 UNIT}(provider, terms[1MiB×50], sig)");
    const contractAccount = h160ToSubstrate(deployed.addressBytes);
    const signed = await negotiatePrecompileTerms(api, providerUrl, contractAccount, {
      maxBytes: 1n << 20n, // 1 MiB capacity
      duration: 50,
      pricePerByte: PRICE_PER_BYTE,
    });
    const createData = encodeCall(abi, "createTeam", [
      toHex(provider.publicKey),
      signed.terms,
      signed.signature,
    ]);
    let r = await callContract(api, client, deployed.addressBytes, createData, {
      value: 10n * UNIT,
    });
    const bucketCreated = requireOneEvent(
      r.events,
      api.event.StorageProvider.BucketCreated,
      "StorageProvider.BucketCreated"
    );
    console.log("  bucketId =", bucketCreated.bucket_id.toString());
    assertContractEvent(r.events, "TeamCreated");

    // 3) invite Charlie as Writer, then kick him.
    console.log("\n[3/3] invite(Charlie, Writer)");
    const inviteData = encodeCall(abi, "invite", [toHex(member.publicKey), SolRole.Writer]);
    r = await callContract(api, client, deployed.addressBytes, inviteData);
    requireOneEvent(
      r.events,
      api.event.StorageProvider.MemberSet,
      "StorageProvider.MemberSet"
    );
    assertContractEvent(r.events, "Invited");

    // kick Charlie.
    console.log("        kick(Charlie)");
    const kickData = encodeCall(abi, "kick", [toHex(member.publicKey)]);
    r = await callContract(api, client, deployed.addressBytes, kickData);
    requireOneEvent(
      r.events,
      api.event.StorageProvider.MemberRemoved,
      "StorageProvider.MemberRemoved"
    );
    assertContractEvent(r.events, "Kicked");

    console.log("\n✅ SharedTeamDrive flow completed");
  } finally {
    papi.destroy();
  }
}

main().catch((e) => {
  console.error(e);
  process.exit(1);
});
