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
 *   - Descriptors generated: npm run papi:generate
 *
 * Usage: node full-flow.js [chain_ws] [provider_url] [provider_seed] [client_seed]
 */

import assert from "node:assert";
import {
  acceptAgreement,
  challengeCheckpoint,
  challengeOffchain,
  createBucket,
  downloadChunk,
  endAgreement,
  fetchChallengeProof,
  fetchCheckpointSignature,
  requestPrimaryAgreement,
  respondToChallenge,
  submitClientCheckpoint,
  uploadChunk,
} from "./api.js";
import {
  connect,
  ensureProviderRegistered,
  makeSigner,
  parseProviderClientArgs,
  waitForBlock,
  waitForBlockProduction,
  waitForChainReady,
  waitForNextBlock,
} from "./common.js";

const {
  chainWs: CHAIN_WS,
  providerUrl: PROVIDER_URL,
  providerSeed: PROVIDER_SEED,
  clientSeed: CLIENT_SEED,
} = parseProviderClientArgs();

async function setupAgreement(api, client, provider, bucketId) {
  const existing = await api.query.StorageProvider.StorageAgreements.getValue(
    bucketId,
    provider.address
  );
  if (existing) {
    console.log("  Agreement already exists");
    return;
  }
  const maxBytes = 1_073_741_824n; // 1 GiB
  const duration = 50;
  console.log(
    "  Requesting agreement (%s), duration=%d blocks...",
    client.seed,
    duration
  );
  await requestPrimaryAgreement(api, client, provider, bucketId, {
    max_bytes: maxBytes,
    duration,
    max_payment: maxBytes * BigInt(duration) * 2n,
  });
  console.log("  Accepting agreement (%s)...", provider.seed);
  await acceptAgreement(api, provider, bucketId);
  console.log("  Agreement accepted");
}

async function uploadAndVerify(api, bucketId) {
  const payload = `Hello, Web3 Storage! [${new Date().toISOString()}] provider=${PROVIDER_SEED}`;
  // Snapshot the current block as the nonce — both the provider's signed
  // CommitmentPayload and the eventual `challenge_offchain` extrinsic share
  // this value so the pallet's recency check passes.
  const nonce = Number(await api.query.System.Number.getValue());
  const { hash, data, commit } = await uploadChunk(
    PROVIDER_URL,
    bucketId,
    payload,
    nonce
  );
  console.log("  Uploaded %d bytes, mmr_root=%s", data.length, commit.mmr_root);

  const downloaded = await downloadChunk(PROVIDER_URL, hash);
  assert.deepStrictEqual(
    downloaded,
    Buffer.from(data),
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

async function claimPaymentAfterExpiry(api, papi, provider, client, bucketId) {
  const agreement = await api.query.StorageProvider.StorageAgreements.getValue(
    bucketId,
    provider.address
  );
  const expiresAt = Number(agreement.expires_at);
  console.log("  Agreement expires at block:", expiresAt);

  const freeBefore = (await api.query.System.Account.getValue(provider.address))
    .data.free;
  console.log("  Provider balance before:", freeBefore.toString());

  console.log("  Waiting for agreement to expire...");
  await waitForBlock(papi, expiresAt);
  await endAgreement(api, client, provider, bucketId, "Pay");

  const freeAfter = (await api.query.System.Account.getValue(provider.address))
    .data.free;
  const earned = freeAfter - freeBefore;
  console.log("  Provider balance after:", freeAfter.toString());
  console.log("  Earned from agreement:", earned.toString());
  assert.ok(earned > 0n, `Expected provider to earn tokens, got ${earned}`);
  console.log("PASSED: Provider received payment!");
}

function watchDefendedEvents(api) {
  const events = [];
  const sub = api.event.StorageProvider.ChallengeDefended.watch().subscribe(
    (event) => {
      console.log("  >> ChallengeDefended event:", {
        deadline: event.payload.challenge_id.deadline,
        index: event.payload.challenge_id.index,
      });
      events.push(event);
    }
  );
  return { events, unsubscribe: () => sub.unsubscribe() };
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
  const defended = watchDefendedEvents(api);

  try {
    console.log("\n=== Step 1: Setup ===");
    await ensureProviderRegistered(api, provider, PROVIDER_URL);
    const bucketId = await createBucket(api, client);
    console.log("  Bucket created with ID:", bucketId);
    await setupAgreement(api, client, provider, bucketId);

    console.log("\n=== Step 2: Upload data ===");
    const upload = await uploadAndVerify(api, bucketId);

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
    await respondToChallenge(api, provider, offchainId, offchainProof);
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
    await respondToChallenge(api, provider, checkpointId, checkpointProof);
    console.log("  Challenge defended");

    console.log("\n=== Verifying challenge results ===");
    await new Promise((r) => setTimeout(r, 3000));
    console.log("ChallengeDefended events: %d (expected: 2)", defended.events.length);
    assert.strictEqual(
      defended.events.length,
      2,
      `Expected 2 ChallengeDefended events, got ${defended.events.length}`
    );
    console.log("PASSED: Both challenges were defended!");

    console.log("\n=== Step 8: Wait for agreement expiry & claim payment ===");
    await claimPaymentAfterExpiry(api, papi, provider, client, bucketId);
  } catch (err) {
    console.error("\nERROR:", err.message || err);
    if (err.stack) console.error(err.stack);
    process.exitCode = 1;
  } finally {
    defended.unsubscribe();
    papi.destroy();
  }
}

main().then(() => console.log("\n=== Demo complete! ==="));
