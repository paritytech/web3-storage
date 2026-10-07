// SPDX-License-Identifier: Apache-2.0

/**
 * Smart-contract end-to-end demo for the `TokenGatedDrive` example dApp.
 *
 * The contract owns a private Layer 0 bucket via the storage-provider
 * precompile and mints a transferable token per object key. A token holder
 * is a `Reader` member of the bucket.
 * Flow:
 *   1. Provider setup + account mapping for Bob/Charlie/Dave.
 *   2. Deploy `TokenGatedDrive.sol`.
 *   3. Publisher (`//Bob`) initializes the bucket with `msg.value` covering
 *      the agreement reserve; Bob's account becomes a `Writer`.
 *   4. Publisher mints a token to Charlie: Charlie becomes a `Reader`.
 *   5. Charlie transfers the token to Dave: membership moves to Dave.
 *   6. Dave burns the token: Dave loses membership.
 *
 * Asserts storage-provider pallet events (`BucketCreated`, `MemberSet`,
 * `MemberRemoved`), contract events (`Initialized`, `Minted`, `Transfer`,
 * `Burned`) and the final bucket member list.
 *
 * Usage: node sc-token-gated.js [chain_ws] [provider_url] [provider_seed] [client_seed]
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
  READ_OPTS,
  requireOneEvent,
  sameAddress,
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
import { h160ToSubstrate, negotiatePrecompileTerms } from "./sc-support.js";
import {
  ensureSoleAcceptingProvider,
  parseProviderClientArgs,
} from "./support.js";

const { chainWs, providerUrl, providerSeed, clientSeed } = parseProviderClientArgs();

const HERE = dirname(fileURLToPath(import.meta.url));
const CONTRACT_JSON = resolve(HERE, "../contracts/build/combined.json");
const CONTRACT_KEY = "TokenGatedDrive.sol:TokenGatedDrive";

const UNIT = 10n ** 12n;

async function main() {
  console.log("=== TokenGatedDrive e2e ===");
  console.log(" chain    :", chainWs);
  console.log(" provider :", providerUrl, `(${providerSeed})`);
  console.log(" client   :", clientSeed);

  const { papi, api } = await connect(chainWs);
  try {
    await waitForChainReady(api);
    await waitForBlockProduction(api);
    await waitForNextBlock(papi);

    // Publisher is the demo's "client" account (default //Bob), NOT the
    // storage provider. Using the provider account here would race the
    // provider node's background workers that sign extrinsics from the
    // same key, surfacing as `Invalid::Stale` on the mempool side.
    const provider = makeSigner(providerSeed); // //Alice — storage provider only
    const publisher = makeSigner(clientSeed); // //Bob — deploys + publishes
    const firstHolder = makeSigner("//Charlie");
    const secondHolder = makeSigner("//Dave");

    console.log("\n[setup] provider + Revive account mapping…");
    const PRICE_PER_BYTE = 1n;
    await ensureProviderRegistered(api, provider, providerUrl, {
      pricePerByte: PRICE_PER_BYTE,
      maxDuration: 100_000,
    });
    await ensureSoleAcceptingProvider(api, provider);
    await ensureAccountMapped(api, provider);
    await ensureAccountMapped(api, publisher);
    await ensureAccountMapped(api, firstHolder);
    await ensureAccountMapped(api, secondHolder);

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

    // 1) Deploy (signed by Bob — he becomes publisher in `initialize`).
    console.log("\n[1/5] Deploying TokenGatedDrive…");
    const deployed = await deployContract(api, publisher, bytecode);
    console.log("  contract:", deployed.address);
    const contractEvent = (events: Parameters<typeof decodeContractEmitted>[0], name: string) => {
      const log = decodeContractEmitted(events, api, deployed.addressBytes, abi).find(
        (l) => l.eventName === name
      );
      assert.ok(log, `${name} event missing`);
      return log.args as any;
    };

    // 2) initialize{value: 5 UNIT} — terms negotiated with the contract's
    // substrate-mapped account as owner; msg.value funds that account's
    // payment reserve. Bob's account becomes a Writer so it can upload.
    console.log("\n[2/5] initialize{value: 5 UNIT}(Bob, provider, terms[1MiB×50], sig)");
    const contractAccount = h160ToSubstrate(deployed.addressBytes);
    const signed = await negotiatePrecompileTerms(providerUrl, contractAccount, {
      maxBytes: 1n << 20n,
      duration: 50,
      pricePerByte: PRICE_PER_BYTE,
    });
    const initData = encodeCall(abi, "initialize", [
      toHex(publisher.publicKey),
      toHex(provider.publicKey),
      signed.terms,
      signed.signature,
    ]);
    let r = await callContract(api, publisher, deployed.addressBytes, initData, {
      value: 5n * UNIT,
    });
    const bucketCreated = requireOneEvent(
      r.events,
      api.event.StorageProvider.BucketCreated,
      "StorageProvider.BucketCreated"
    );
    const bucketId = bucketCreated.bucket_id;
    console.log("  bucketId =", bucketId.toString());
    requireOneEvent(r.events, api.event.StorageProvider.MemberSet, "StorageProvider.MemberSet");
    contractEvent(r.events, "Initialized");

    // 3) Publisher mints a token to Charlie, who becomes a Reader.
    console.log("\n[3/5] (publisher) mint(Charlie, 'files/hello.txt')");
    const mintData = encodeCall(abi, "mint", [toHex(firstHolder.publicKey), "files/hello.txt"]);
    r = await callContract(api, publisher, deployed.addressBytes, mintData);
    requireOneEvent(r.events, api.event.StorageProvider.MemberSet, "StorageProvider.MemberSet");
    const tokenId = contractEvent(r.events, "Minted").tokenId;
    console.log("  tokenId =", tokenId.toString());

    // 4) Charlie transfers the token to Dave; membership moves with it.
    console.log("\n[4/5] (Charlie) transfer(Dave, tokenId)");
    const transferData = encodeCall(abi, "transfer", [toHex(secondHolder.publicKey), tokenId]);
    r = await callContract(api, firstHolder, deployed.addressBytes, transferData);
    requireOneEvent(
      r.events,
      api.event.StorageProvider.MemberRemoved,
      "StorageProvider.MemberRemoved"
    );
    requireOneEvent(r.events, api.event.StorageProvider.MemberSet, "StorageProvider.MemberSet");
    const transfer = contractEvent(r.events, "Transfer");
    assert.strictEqual(String(transfer.from).toLowerCase(), toHex(firstHolder.publicKey).toLowerCase());
    assert.strictEqual(String(transfer.to).toLowerCase(), toHex(secondHolder.publicKey).toLowerCase());

    // 5) Dave burns the token and loses membership.
    console.log("\n[5/5] (Dave) burn(tokenId)");
    const burnData = encodeCall(abi, "burn", [tokenId]);
    r = await callContract(api, secondHolder, deployed.addressBytes, burnData);
    requireOneEvent(
      r.events,
      api.event.StorageProvider.MemberRemoved,
      "StorageProvider.MemberRemoved"
    );
    contractEvent(r.events, "Burned");

    // Remaining members: the contract (Admin) and Bob (Writer).
    const bucket = (await api.query.StorageProvider.Buckets.getValue(bucketId, READ_OPTS))!;
    const roles = bucket.members.map((m: { account: string; role: { type: string } }) => ({
      account: m.account,
      role: m.role.type,
    }));
    assert.strictEqual(roles.length, 2, `expected 2 members, got ${JSON.stringify(roles)}`);
    assert.ok(
      roles.some((m) => sameAddress(m.account, contractAccount.address) && m.role === "Admin"),
      "contract should be Admin"
    );
    assert.ok(
      roles.some((m) => sameAddress(m.account, publisher.address) && m.role === "Writer"),
      "publisher should be Writer"
    );

    console.log("\n✅ TokenGatedDrive flow completed");
  } finally {
    papi.destroy();
  }
}

main().catch((e) => {
  console.error(e);
  process.exit(1);
});
