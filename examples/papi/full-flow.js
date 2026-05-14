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

import { Binary, Enum } from "@polkadot-api/substrate-bindings";
import { blake2AsU8a, cryptoWaitReady } from "@polkadot/util-crypto";
import assert from "node:assert";
import {
  connect,
  makeSigner,
  parseProviderClientArgs,
  toHex,
  hexToBytes,
  providerFetch,
  ensureProviderRegistered,
  requireOneEvent,
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

// ---------------------------------------------------------------------------
// Step helpers
// ---------------------------------------------------------------------------

async function createBucket(api, client) {
  const result = await api.tx.StorageProvider.create_bucket({
    min_providers: 1,
  }).signAndSubmit(client.signer);
  const event = requireOneEvent(
    result.events,
    api.event.StorageProvider.BucketCreated,
    "BucketCreated"
  );
  console.log("  Bucket created with ID:", event.bucket_id);
  return event.bucket_id;
}

async function requestAgreement(api, client, provider, bucketId, params) {
  const existing = await api.query.StorageProvider.StorageAgreements.getValue(
    bucketId,
    provider.address
  );
  if (existing) {
    console.log("  Agreement already exists");
    return;
  }
  console.log(
    "  Requesting agreement (%s), duration=%d blocks, maxPayment=%s...",
    client.seed,
    params.duration,
    params.max_payment
  );
  await api.tx.StorageProvider.request_primary_agreement({
    bucket_id: bucketId,
    provider: provider.address,
    ...params,
  }).signAndSubmit(client.signer);

  console.log("  Accepting agreement (%s)...", provider.seed);
  await api.tx.StorageProvider.accept_agreement({
    bucket_id: bucketId,
  }).signAndSubmit(provider.signer);
  console.log("  Agreement accepted");
}

async function uploadData(bucketId) {
  const data = new TextEncoder().encode(
    `Hello, Web3 Storage! [${new Date().toISOString()}] provider=${PROVIDER_SEED}`
  );
  const chunkHashHex = toHex(blake2AsU8a(data));

  console.log("  Uploading chunk (%d bytes) to bucket %s...", data.length, bucketId);
  await providerFetch(PROVIDER_URL, "/node", {
    method: "PUT",
    body: {
      bucket_id: Number(bucketId),
      hash: chunkHashHex,
      data: Buffer.from(data).toString("base64"),
      children: null,
    },
  });

  console.log("  Committing to MMR...");
  const commit = await providerFetch(PROVIDER_URL, "/commit", {
    method: "POST",
    body: { bucket_id: Number(bucketId), data_roots: [chunkHashHex] },
  });
  console.log("  MMR root:", commit.mmr_root);
  console.log("  Leaf indices:", commit.leaf_indices);

  await verifyUpload(chunkHashHex, data);

  return {
    leafIndex: commit.leaf_indices[0],
    mmrRoot: commit.mmr_root,
    startSeq: commit.start_seq,
    providerSignature: commit.provider_signature,
  };
}

async function verifyUpload(chunkHashHex, originalData) {
  const downloaded = await providerFetch(PROVIDER_URL, "/node", {
    params: { hash: chunkHashHex },
  });
  const downloadedData = Buffer.from(downloaded.data, "base64");
  assert.deepStrictEqual(
    downloadedData,
    Buffer.from(originalData),
    "Downloaded data does not match uploaded data"
  );
  console.log("  Upload verified: data matches (%d bytes)", originalData.length);
}

async function challengeOffchain(api, provider, client, upload, bucketId) {
  console.log("  Submitting challenge_offchain:");
  console.log("    bucket_id:", bucketId);
  console.log("    provider:", provider.address);
  console.log("    mmr_root:", upload.mmrRoot);

  const result = await api.tx.StorageProvider.challenge_offchain({
    bucket_id: bucketId,
    provider: provider.address,
    mmr_root: Binary.fromBytes(hexToBytes(upload.mmrRoot)),
    start_seq: BigInt(upload.startSeq),
    leaf_index: BigInt(upload.leafIndex),
    chunk_index: 0n,
    provider_signature: Enum(
      "Sr25519",
      Binary.fromBytes(hexToBytes(upload.providerSignature))
    ),
  }).signAndSubmit(client.signer);

  assertNoExtrinsicFailure(api, result);
  const event = requireOneEvent(
    result.events,
    api.event.StorageProvider.ChallengeCreated,
    "ChallengeCreated (offchain)"
  );
  console.log(
    "  Challenge created: deadline=%s, index=%s",
    event.challenge_id.deadline,
    event.challenge_id.index
  );
  return event.challenge_id;
}

async function challengeCheckpoint(api, provider, client, leafIndex, bucketId) {
  const result = await api.tx.StorageProvider.challenge_checkpoint({
    bucket_id: bucketId,
    provider: provider.address,
    leaf_index: BigInt(leafIndex),
    chunk_index: 0n,
  }).signAndSubmit(client.signer);
  const event = requireOneEvent(
    result.events,
    api.event.StorageProvider.ChallengeCreated,
    "ChallengeCreated (checkpoint)"
  );
  console.log(
    "  Challenge created: deadline=%s, index=%s",
    event.challenge_id.deadline,
    event.challenge_id.index
  );
  return event.challenge_id;
}

async function submitCheckpoint(api, provider, client, bucketId) {
  const ck = await providerFetch(PROVIDER_URL, "/checkpoint-signature", {
    params: { bucket_id: Number(bucketId) },
  });
  console.log("  Checkpoint mmr_root:", ck.mmr_root);
  console.log("  Checkpoint leaf_count:", ck.leaf_count);

  await api.tx.StorageProvider.checkpoint({
    bucket_id: bucketId,
    mmr_root: Binary.fromBytes(hexToBytes(ck.mmr_root)),
    start_seq: BigInt(ck.start_seq),
    leaf_count: BigInt(ck.leaf_count),
    signatures: [
      [
        provider.address,
        Enum("Sr25519", Binary.fromBytes(hexToBytes(ck.provider_signature))),
      ],
    ],
  }).signAndSubmit(client.signer);
  console.log("  Checkpoint submitted");
}

async function respondToChallenge(api, provider, challengeId) {
  const proof = await fetchChallengeProof(api, challengeId);
  await api.tx.StorageProvider.respond_to_challenge({
    challenge_id: challengeId,
    response: Enum("Proof", proof),
  }).signAndSubmit(provider.signer);
}

async function fetchChallengeProof(api, challengeId) {
  const challenges = await api.query.StorageProvider.Challenges.getValue(
    challengeId.deadline
  );
  if (!challenges) throw new Error("No challenges at deadline " + challengeId.deadline);
  const challenge = challenges[challengeId.index];
  if (!challenge) throw new Error("Challenge index not found: " + challengeId.index);

  const mmr = await providerFetch(PROVIDER_URL, "/mmr_proof", {
    params: {
      bucket_id: Number(challenge.bucket_id),
      leaf_index: Number(challenge.leaf_index),
    },
  });
  const chunk = await providerFetch(PROVIDER_URL, "/chunk_proof", {
    params: {
      data_root: mmr.leaf.data_root,
      chunk_index: Number(challenge.chunk_index),
    },
  });

  return {
    chunk_data: Binary.fromBytes(Buffer.from(chunk.chunk_data, "base64")),
    mmr_proof: {
      peaks: mmr.proof.peaks.map((h) => Binary.fromBytes(hexToBytes(h))),
      leaf: {
        data_root: Binary.fromBytes(hexToBytes(mmr.leaf.data_root)),
        data_size: BigInt(mmr.leaf.data_size),
        total_size: BigInt(mmr.leaf.total_size),
      },
      leaf_proof: {
        siblings: mmr.proof.siblings.map((h) => Binary.fromBytes(hexToBytes(h))),
        path: mmr.proof.path,
      },
    },
    chunk_proof: {
      siblings: chunk.proof.siblings.map((h) => Binary.fromBytes(hexToBytes(h))),
      path: chunk.proof.path,
    },
  };
}

async function endAgreementWithPay(api, client, provider, bucketId) {
  console.log("  Ending agreement with Pay action (%s)...", client.seed);
  await api.tx.StorageProvider.end_agreement({
    bucket_id: bucketId,
    provider: provider.address,
    action: Enum("Pay"),
  }).signAndSubmit(client.signer);
  console.log("  Agreement ended with payment");
}

async function getFreeBalance(api, who) {
  const acc = await api.query.System.Account.getValue(who.address);
  return acc.data.free;
}

async function claimPaymentAfterExpiry(api, papi, provider, client, bucketId) {
  const agreement = await api.query.StorageProvider.StorageAgreements.getValue(
    bucketId,
    provider.address
  );
  const expiresAt = Number(agreement.expires_at);
  console.log("  Agreement expires at block:", expiresAt);

  const freeBefore = await getFreeBalance(api, provider);
  console.log("  Provider balance before:", freeBefore.toString());

  console.log("  Waiting for agreement to expire...");
  await waitForBlock(papi, expiresAt);

  await endAgreementWithPay(api, client, provider, bucketId);

  const freeAfter = await getFreeBalance(api, provider);
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

function assertNoExtrinsicFailure(api, result) {
  const failed = api.event.System.ExtrinsicFailed.filter(result.events);
  if (failed.length > 0) {
    console.log("  ERROR: Extrinsic failed!");
    for (const e of failed) {
      console.log("    dispatch_error:", JSON.stringify(e.dispatch_error, null, 2));
    }
    throw new Error("Extrinsic failed");
  }
}

// ---------------------------------------------------------------------------
// Main
// ---------------------------------------------------------------------------

async function main() {
  await cryptoWaitReady();

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
    const agreementMaxBytes = 1_073_741_824n; // 1 GiB
    const agreementDuration = 50;
    await requestAgreement(api, client, provider, bucketId, {
      max_bytes: agreementMaxBytes,
      duration: agreementDuration,
      max_payment: agreementMaxBytes * BigInt(agreementDuration) * 2n,
    });

    console.log("\n=== Step 2: Upload data ===");
    const upload = await uploadData(bucketId);

    console.log("\n=== Step 3: Off-chain challenge ===");
    const offchainId = await challengeOffchain(api, provider, client, upload, bucketId);

    console.log("\n=== Step 4: Respond to off-chain challenge ===");
    await respondToChallenge(api, provider, offchainId);
    console.log("  Challenge defended");

    console.log("\n=== Step 5: Submit checkpoint ===");
    await submitCheckpoint(api, provider, client, bucketId);

    console.log("\n=== Step 6: On-chain checkpoint challenge ===");
    const checkpointId = await challengeCheckpoint(api, provider, client, upload.leafIndex, bucketId);

    console.log("\n=== Step 7: Respond to checkpoint challenge ===");
    await respondToChallenge(api, provider, checkpointId);
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
