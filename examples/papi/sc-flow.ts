// SPDX-License-Identifier: Apache-2.0

/**
 * Smart-contract end-to-end demo for web3-storage (issue #83).
 *
 *   JS  ─►  pallet_revive.call (v2)  ─►  StorageMarketplace (PolkaVM)
 *                                         │
 *                                         └─►  storage-provider precompile (0x…0901)
 *                                              │
 *                                              └─►  pallet_storage_provider  ─►  provider HTTP node
 *
 * Same end-state as `full-flow.js` (provider earned tokens from a paid-out
 * agreement), but the bucket lifecycle is mediated by a Solidity contract
 * instead of direct extrinsics.
 *
 * Prerequisites:
 *   - Chain at ws://127.0.0.1:2222 with `pallet_revive` wired in.
 *   - Provider node running and reachable at `providerUrl`.
 *   - `examples/contracts/build/StorageMarketplace.{bin,abi}` exist
 *     (run `just build-contracts` first).
 *   - Workspace deps installed via `pnpm install` (descriptors include Revive).
 *
 * Usage: node sc-flow.js [chain_ws] [provider_url] [provider_seed] [client_seed]
 */

import assert from "node:assert";
import { readFile } from "node:fs/promises";
import { dirname, resolve } from "node:path";
import { fileURLToPath } from "node:url";

import {
  challengeOffchain,
  connect,
  currentRelayBlock,
  downloadChunk,
  ensureProviderRegistered,
  fetchChallengeProof,
  hexToBytes,
  makeSigner,
  READ_OPTS,
  requireOneEvent,
  respondToChallenge,
  toHex,
  uploadChunk,
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

const {
  chainWs: CHAIN_WS,
  providerUrl: PROVIDER_URL,
  providerSeed: PROVIDER_SEED,
  clientSeed: CLIENT_SEED,
} = parseProviderClientArgs();

const HERE = dirname(fileURLToPath(import.meta.url));
const CONTRACT_JSON = resolve(HERE, "../contracts/build/combined.json");
const CONTRACT_KEY = "StorageMarketplace.sol:StorageMarketplace";

// Storage parameters small enough to compute payment without overshadowing
// the deployer's funded balance, large enough to exercise upload + challenge.
const MAX_BYTES = 1024n; // 1 KiB
const DURATION = 50; // blocks
const UNIT = 10n ** 12n; // matches runtime constant
const FUND_VALUE = 5n * UNIT; // contract gets 5 UNIT; payment ~ 51200 atomic, vast headroom

async function main() {
  console.log("=== Smart-contract end-to-end flow ===");
  console.log(" chain     :", CHAIN_WS);
  console.log(" provider  :", PROVIDER_URL, `(${PROVIDER_SEED})`);
  console.log(" client    :", CLIENT_SEED);

  const { papi, api } = await connect(CHAIN_WS);
  try {
    await waitForChainReady(api);
    await waitForBlockProduction(api);
    await waitForNextBlock(papi);

    const provider = makeSigner(PROVIDER_SEED);
    const client = makeSigner(CLIENT_SEED);

    // 1) Provider setup. Sets accepting_primary + pricePerByte=1 so payment
    //    math is trivial, and silences every other dev-key provider so only
    //    the provider we negotiate with holds agreements. We also have to
    //    register both the deployer and caller with pallet_revive
    //    (`map_account`) — substrate-native accounts can't dispatch contract
    //    calls or be the target of value transfers until they're mapped.
    console.log("\n[1/6] Provider setup + Revive account mapping…");
    const PRICE_PER_BYTE = 1n;
    await ensureProviderRegistered(api, provider, PROVIDER_URL, {
      pricePerByte: PRICE_PER_BYTE,
      maxDuration: 100_000,
    });
    await ensureSoleAcceptingProvider(api, provider);
    await ensureAccountMapped(api, provider);
    await ensureAccountMapped(api, client);

    // 2) Load combined.json (build.sh emits one JSON keyed by `<file>:<contract>`
    //    with `abi` array and `bin` hex string).
    console.log("\n[2/6] Loading marketplace bytecode + ABI…");
    const combined = JSON.parse(await readFile(CONTRACT_JSON, "utf8"));
    const entry = combined.contracts?.[CONTRACT_KEY];
    if (!entry) {
      throw new Error(
        `combined.json missing entry for ${CONTRACT_KEY}; saw keys: ${Object.keys(combined.contracts ?? {}).join(", ")}`
      );
    }
    const abi = entry.abi;
    const bytecode = hexToBytes(entry.bin);
    console.log("  bytecode:", bytecode.length, "bytes");

    // 3) Deploy. //Alice deploys; contract's substrate-mapped account is
    //    derived from its H160 via AccountId32Mapper.
    console.log("\n[3/6] Deploying StorageMarketplace…");
    const deployed = await deployContract(api, provider, bytecode);
    console.log("  contract:", deployed.address);

    // 4) //Bob buys storage. The terms are negotiated off-chain with the
    //    provider using the *contract's* substrate-mapped account as owner
    //    (the contract is the precompile caller); msg.value funds that
    //    account so the precompile can reserve the agreement payment.
    console.log("\n[4/6] buyStorage{value: 5 UNIT}(provider, terms[1KiB×50], sig)…");
    const contractAccount = h160ToSubstrate(deployed.addressBytes);
    const signed = await negotiatePrecompileTerms(PROVIDER_URL, contractAccount, {
      maxBytes: MAX_BYTES,
      duration: DURATION,
      pricePerByte: PRICE_PER_BYTE,
    });
    const buyData = encodeCall(abi, "buyStorage", [
      toHex(provider.publicKey),
      signed.terms,
      signed.signature,
    ]);
    const buyResult = await callContract(api, client, deployed.addressBytes, buyData, {
      value: FUND_VALUE,
    });
    const bucketCreated = requireOneEvent(
      buyResult.events,
      api.event.StorageProvider.BucketCreated,
      "StorageProvider.BucketCreated"
    );
    const bucketId = bucketCreated.bucket_id;
    console.log("  bucketId:", bucketId.toString());

    const contractLogs = decodeContractEmitted(
      buyResult.events,
      api,
      deployed.addressBytes,
      abi
    );
    const bought = contractLogs.find((l) => l.eventName === "BucketBoughtFor");
    assert(bought, "BucketBoughtFor not emitted by contract");
    console.log("  BucketBoughtFor user:", (bought.args as any).user);

    // 5) Off-chain ops are unchanged — they bypass the contract entirely.
    console.log("\n[5/6] Off-chain upload + challenge round-trip…");
    const payload = `Hello via SC! ${new Date().toISOString()}`;
    const uploadNonce = await currentRelayBlock(api);
    const upload = await uploadChunk(PROVIDER_URL, bucketId, payload, uploadNonce);
    const downloaded = await downloadChunk(PROVIDER_URL, upload.hash);
    assert.deepStrictEqual(
      downloaded,
      upload.data,
      "downloaded bytes != uploaded"
    );
    const offchainId = await challengeOffchain(api, client, provider, bucketId, {
      mmrRoot: upload.commit.mmr_root,
      startSeq: upload.commit.start_seq,
      leafCount: upload.commit.leaf_count,
      leafIndex: upload.commit.leaf_indices[0],
      leafCount: upload.commit.leaf_count,
      providerSignature: upload.commit.provider_signature,
      nonce: upload.commit.nonce,
    });
    const proof = await fetchChallengeProof(api, PROVIDER_URL, offchainId);
    await respondToChallenge(api, provider, offchainId, proof);
    console.log("  ChallengeDefended ✓");

    // 6) End the agreement *through the contract*. Only the original buyer
    //    (the marketplace's bucket-owner mapping) can call this.
    console.log("\n[6/6] endMyAgreement via contract…");
    const providerBytes32 = provider.publicKey; // substrate AccountId32 = 32-byte sr25519 pubkey
    const endData = encodeCall(abi, "endMyAgreement", [
      bucketId,
      toHex(providerBytes32),
    ]);
    const freeBefore = (
      await api.query.System.Account.getValue(provider.address, READ_OPTS)
    ).data.free;
    const endResult = await callContract(
      api,
      client,
      deployed.addressBytes,
      endData
    );
    const freeAfter = (
      await api.query.System.Account.getValue(provider.address, READ_OPTS)
    ).data.free;
    const earned = freeAfter - freeBefore;
    console.log("  provider earned:", earned.toString(), "atomic units");
    assert.ok(earned > 0n, `expected provider to earn tokens, got ${earned}`);

    const endContractLogs = decodeContractEmitted(
      endResult.events,
      api,
      deployed.addressBytes,
      abi
    );
    assert.ok(
      endContractLogs.some((l) => l.eventName === "AgreementEnded"),
      "AgreementEnded not emitted by contract"
    );

    console.log("\n✅ Smart-contract flow completed");
  } finally {
    papi.destroy();
  }
}

main().catch((e) => {
  console.error(e);
  process.exit(1);
});
