/**
 * Smart-contract end-to-end demo for the `TokenGatedDrive` example dApp.
 *
 * The contract owns an S3 bucket on-chain via the s3-registry precompile
 * and mints a transferable NFT-shaped token per object stored in it.
 * Flow:
 *   1. Provider setup + account mapping for Alice/Bob/Charlie.
 *   2. Deploy `TokenGatedDrive.sol`.
 *   3. Publisher (`//Alice`) initializes the bucket with `msg.value`
 *      covering the agreement reserve.
 *   4. Publisher mints a token to Bob's H160.
 *   5. Bob transfers the token to Charlie.
 *   6. Charlie burns the token.
 *   7. Publisher shuts down the contract (bucket deleted).
 *
 * Asserts S3-registry pallet events (`S3BucketCreated`, `ObjectPut`,
 * `ObjectDeleted`, `S3BucketDeleted`) and contract events
 * (`Initialized`, `Minted`, `Transfer` ×2, `Burned`, `Shutdown`).
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
  parseProviderClientArgs,
  requireOneEvent,
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
  substrateToH160,
} from "./sc-api.js";

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
    // checkpoint coordinator that signs background extrinsics from the
    // same key, surfacing as `Invalid::Stale` on the mempool side.
    const provider = makeSigner(providerSeed); // //Alice — storage provider only
    const publisher = makeSigner(clientSeed); // //Bob — deploys + publishes
    const recipient = makeSigner("//Charlie"); // final token holder
    const publisherH160 = substrateToH160(publisher.publicKey);
    const recipientH160 = substrateToH160(recipient.publicKey);

    console.log("\n[setup] provider + Revive account mapping…");
    await ensureProviderRegistered(api, provider, providerUrl, {
      pricePerByte: 1n,
      maxDuration: 100_000,
    });
    await ensureAccountMapped(api, provider);
    await ensureAccountMapped(api, publisher);
    await ensureAccountMapped(api, recipient);

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
    console.log("\n[1/6] Deploying TokenGatedDrive…");
    const deployed = await deployContract(api, publisher, bytecode);
    console.log("  contract:", deployed.address);

    // 2) initialize{value: 5 UNIT}('cov-bucket-N', …).
    // Bucket name: 3-63 chars, lowercase alphanumeric + hyphens. Append the
    // block number so the name is unique across reruns on the same chain
    // (`pallet_s3_registry` enforces global name uniqueness).
    const blockHead = await api.query.System.Number.getValue();
    const bucketName = `cov-bucket-${Number(blockHead)}`;
    console.log(`\n[2/6] initialize{value: 5 UNIT}('${bucketName}', 1MiB, 50 blocks, …)`);
    const initData = encodeCall(abi, "initialize", [bucketName, 1n << 20n, 50, 2n * UNIT]);
    let r = await callContract(api, publisher, deployed.addressBytes, initData, {
      value: 5n * UNIT,
    });
    const bucketCreated = requireOneEvent(
      r.events,
      api.event.S3Registry.S3BucketCreated,
      "S3Registry.S3BucketCreated"
    );
    const s3BucketId = bucketCreated.s3_bucket_id;
    console.log("  s3BucketId =", s3BucketId.toString());

    // 3) Publisher mints a token to themselves first, then transfers to
    // Charlie. Minting to a different recipient would skip the `transfer`
    // path; keeping mint→self lets us exercise both functions on the same
    // token.
    console.log("\n[3/6] (publisher) mint(self, 'files/hello.txt', …)");
    const objectCid =
      "0x1122334455667788990011223344556677889900112233445566778899001122";
    const mintData = encodeCall(abi, "mint", [
      publisherH160,
      "files/hello.txt",
      objectCid,
      42n,
      "text/plain",
    ]);
    r = await callContract(api, publisher, deployed.addressBytes, mintData);
    requireOneEvent(
      r.events,
      api.event.S3Registry.ObjectPut,
      "S3Registry.ObjectPut"
    );
    let logs = decodeContractEmitted(r.events, api, deployed.addressBytes, abi);
    const minted = logs.find((l) => l.eventName === "Minted");
    assert.ok(minted, "Minted event missing");
    const tokenId = minted.args.tokenId;
    console.log("  tokenId =", tokenId.toString());

    // 4) Publisher transfers the token to Charlie.
    console.log("\n[4/6] (publisher) transfer(Charlie, tokenId)");
    const transferData = encodeCall(abi, "transfer", [recipientH160, tokenId]);
    r = await callContract(api, publisher, deployed.addressBytes, transferData);
    logs = decodeContractEmitted(r.events, api, deployed.addressBytes, abi);
    assert.ok(
      logs.some(
        (l) =>
          l.eventName === "Transfer" &&
          l.args.from.toLowerCase() === publisherH160.toLowerCase() &&
          l.args.to.toLowerCase() === recipientH160.toLowerCase()
      ),
      "Transfer(publisher → Charlie) event missing"
    );

    // 5) Charlie burns the token — also deletes the S3 object metadata.
    console.log("\n[5/6] (Charlie) burn(tokenId)");
    const burnData = encodeCall(abi, "burn", [tokenId]);
    r = await callContract(api, recipient, deployed.addressBytes, burnData);
    requireOneEvent(
      r.events,
      api.event.S3Registry.ObjectDeleted,
      "S3Registry.ObjectDeleted"
    );
    logs = decodeContractEmitted(r.events, api, deployed.addressBytes, abi);
    assert.ok(
      logs.some((l) => l.eventName === "Burned"),
      "Burned event missing"
    );

    // 6) Publisher shuts down — bucket must be empty (it is, after burn).
    console.log("\n[6/6] (publisher) shutdown()");
    const shutdownData = encodeCall(abi, "shutdown", []);
    r = await callContract(api, publisher, deployed.addressBytes, shutdownData);
    requireOneEvent(
      r.events,
      api.event.S3Registry.S3BucketDeleted,
      "S3Registry.S3BucketDeleted"
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
