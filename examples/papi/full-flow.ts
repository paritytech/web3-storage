// SPDX-License-Identifier: Apache-2.0

/**
 * PAPI-based integration test for web3-storage.
 *
 * Single-script orchestration of the end-to-end Layer 0 flow:
 *  1. Setup provider, bucket, and agreement (on-chain)
 *  2. Upload data to the provider (HTTP) and verify it
 *  3. Submit two challenges and respond to both
 *  4. Assert exactly 2 ChallengeDefended events
 *  5. Wait for agreement expiry, claim payment, assert provider earned
 *
 * Prerequisites:
 *   - Parachain running at ws://127.0.0.1:2222
 *   - Provider node running at the specified URL
 *   - Workspace deps installed: pnpm install (descriptors come from the tracked metadata)
 *
 * Usage: node full-flow.js [chain_ws] [provider_url] [provider_seed] [client_seed]
 */

import assert from "node:assert";
import type { PolkadotClient } from "polkadot-api";
import {
  challengeCheckpoint,
  challengeOffchain,
  connect,
  downloadChunk,
  endAgreement,
  ensureProviderRegistered,
  establishStorageAgreement,
  fetchChallengeProof,
  fetchCheckpointSignature,
  makeSigner,
  negotiateTerms,
  READ_OPTS,
  requireOneEvent,
  respondToChallenge,
  submitClientCheckpoint,
  uploadChunk,
  waitForBlock,
  waitForBlockProduction,
  waitForChainReady,
  waitForNextBlock,
  type ChainSigner,
  type ParachainApi,
} from "@web3-storage/sdk";
import { parseProviderClientArgs } from "./support.js";

const {
  chainWs: CHAIN_WS,
  providerUrl: PROVIDER_URL,
  providerSeed: PROVIDER_SEED,
  clientSeed: CLIENT_SEED,
} = parseProviderClientArgs();

/**
 * Negotiate provider-signed terms over HTTP, then redeem them on-chain.
 * The bucket + primary agreement are opened atomically inside
 * `establish_storage_agreement` — no separate create_bucket step.
 */
async function setupAgreement(
  api: ParachainApi,
  providerUrl: string,
  client: ChainSigner,
  provider: ChainSigner
): Promise<bigint> {
  const maxBytes = 1_073_741_824n; // 1 GiB
  // Must outlast Step 6's checkpoint challenge, which requires a live agreement:
  // the finalized establish + off-chain challenge land it at ~block 16, so 15
  // expired too soon. 30 leaves ~14 blocks of margin for finality jitter.
  const duration = 30;
  console.log(
    "  Negotiating signed terms with provider (max_bytes=%s, duration=%d)...",
    maxBytes,
    duration
  );
  const signed = await negotiateTerms(providerUrl, {
    owner: client.address,
    max_bytes: maxBytes,
    duration,
    price_per_byte: 1n,
    replica_params: null,
    bucket_id: null,
  });
  console.log(
    "  Provider signed terms: nonce=%s, valid_until=%s",
    signed.terms.nonce,
    signed.terms.valid_until
  );
  console.log("  Redeeming on-chain via establish_storage_agreement...");
  // Finalize: the immediate upload reads bucket membership from the provider's
  // finalized view, so an in-block establish would race it.
  const { bucketId } = await establishStorageAgreement(api, client, provider, signed, {
    mode: "finalized",
  });
  console.log("  Bucket %s opened with primary agreement", bucketId);
  return bucketId;
}

async function uploadAndVerify(api: ParachainApi, bucketId: bigint, signer: ChainSigner) {
  const payload = `Hello, Web3 Storage! [${new Date().toISOString()}] provider=${PROVIDER_SEED}`;
  const nonce = Number(await api.query.System.Number.getValue());
  const { hash, data, commit } = await uploadChunk(PROVIDER_URL, bucketId, payload, nonce, signer);
  console.log("  Uploaded %d bytes, mmr_root=%s", data.length, commit.mmr_root);

  const downloaded = await downloadChunk(PROVIDER_URL, hash);
  assert.deepStrictEqual(
    downloaded,
    data,
    "Downloaded data does not match uploaded data"
  );
  console.log("  Upload verified (%d bytes)", data.length);

  return {
    leafIndex: commit.leaf_indices[0],
    mmrRoot: commit.mmr_root,
    startSeq: commit.start_seq,
    leafCount: commit.leaf_count,
    providerSignature: commit.provider_signature,
    nonce: commit.nonce,
  };
}

async function claimPaymentAfterExpiry(api: ParachainApi, papi: PolkadotClient, provider: ChainSigner, client: ChainSigner, bucketId: bigint) {
  const agreement = (await api.query.StorageProvider.StorageAgreements.getValue(
    bucketId,
    provider.address,
    READ_OPTS
  ))!;
  const expiresAt = Number(agreement.expires_at);
  console.log("  Agreement expires at block:", expiresAt);

  const freeBefore = (
    await api.query.System.Account.getValue(provider.address, READ_OPTS)
  ).data.free;
  console.log("  Provider balance before:", freeBefore.toString());

  console.log("  Waiting for agreement to expire...");
  await waitForBlock(papi, expiresAt);
  await endAgreement(api, client, provider, bucketId, "Pay");

  const freeAfter = (
    await api.query.System.Account.getValue(provider.address, READ_OPTS)
  ).data.free;
  const earned = freeAfter - freeBefore;
  console.log("  Provider balance after:", freeAfter.toString());
  console.log("  Earned from agreement:", earned.toString());
  assert.ok(earned > 0n, `Expected provider to earn tokens, got ${earned}`);
  console.log("PASSED: Provider received payment!");
}

// Read ChallengeDefended from the respond tx's own in-block events. A
// background `api.event...watch()` only sees finalized blocks, so a count taken
// right after responding would read 0.
function recordDefended(api: ParachainApi, result: { events: unknown[] }, label: string) {
  const event = requireOneEvent(
    result.events,
    api.event.StorageProvider.ChallengeDefended,
    label
  );
  console.log("  >> ChallengeDefended event:", {
    deadline: event.challenge_id.deadline,
    index: event.challenge_id.index,
  });
  return event;
}

async function main() {
  const provider = makeSigner(PROVIDER_SEED);
  const client = makeSigner(CLIENT_SEED);

  console.log("Connecting to chain:", CHAIN_WS);
  console.log("Provider URL:", PROVIDER_URL);
  console.log("Provider seed:", PROVIDER_SEED, "=>", provider.address);
  console.log("Client seed:", CLIENT_SEED, "=>", client.address);

  const { papi, api } = await connect(CHAIN_WS);
  await waitForChainReady(api);
  await waitForBlockProduction(api);
  await waitForNextBlock(papi);
  const defendedEvents = [];

  try {
    console.log("\n=== Step 1: Setup ===");
    await ensureProviderRegistered(api, provider, PROVIDER_URL);
    const bucketId = await setupAgreement(api, PROVIDER_URL, client, provider);

    console.log("\n=== Step 2: Upload data ===");
    const upload = await uploadAndVerify(api, bucketId, client);

    console.log("\n=== Step 3: Off-chain challenge ===");
    const offchainId = await challengeOffchain(
      api,
      client,
      provider,
      bucketId,
      upload
    );
    console.log(
      "  Challenge created: deadline=%s, index=%s",
      offchainId.deadline,
      offchainId.index
    );

    console.log("\n=== Step 4: Respond to off-chain challenge ===");
    const offchainProof = await fetchChallengeProof(api, PROVIDER_URL, offchainId);
    const offchainResp = await respondToChallenge(
      api,
      provider,
      offchainId,
      offchainProof
    );
    defendedEvents.push(
      recordDefended(api, offchainResp, "ChallengeDefended (offchain)")
    );
    console.log("  Challenge defended");

    console.log("\n=== Step 5: Submit checkpoint ===");
    const ckNonce = Number(await api.query.System.Number.getValue());
    const ck = await fetchCheckpointSignature(PROVIDER_URL, bucketId, ckNonce);
    console.log("  Checkpoint mmr_root:", ck.mmr_root);
    console.log("  Checkpoint leaf_count:", ck.leaf_count);
    await submitClientCheckpoint(api, client, provider, bucketId, ck);
    console.log("  Checkpoint submitted");

    console.log("\n=== Step 6: On-chain checkpoint challenge ===");
    const checkpointId = await challengeCheckpoint(
      api,
      client,
      provider,
      bucketId,
      upload.leafIndex
    );
    console.log(
      "  Challenge created: deadline=%s, index=%s",
      checkpointId.deadline,
      checkpointId.index
    );

    console.log("\n=== Step 7: Respond to checkpoint challenge ===");
    const checkpointProof = await fetchChallengeProof(
      api,
      PROVIDER_URL,
      checkpointId
    );
    const checkpointResp = await respondToChallenge(
      api,
      provider,
      checkpointId,
      checkpointProof
    );
    defendedEvents.push(
      recordDefended(api, checkpointResp, "ChallengeDefended (checkpoint)")
    );
    console.log("  Challenge defended");

    console.log("\n=== Verifying challenge results ===");
    console.log(
      "ChallengeDefended events: %d (expected: 2)",
      defendedEvents.length
    );
    assert.strictEqual(
      defendedEvents.length,
      2,
      `Expected 2 ChallengeDefended events, got ${defendedEvents.length}`
    );
    console.log("PASSED: Both challenges were defended!");

    console.log("\n=== Step 8: Wait for agreement expiry & claim payment ===");
    await claimPaymentAfterExpiry(api, papi, provider, client, bucketId);
  } catch (err) {
    console.error("\nERROR:", (err as Error).message || err);
    if ((err as Error).stack) console.error((err as Error).stack);
    process.exitCode = 1;
  } finally {
    papi.destroy();
  }
}

main().then(() => console.log("\n=== Demo complete! ==="));
