/**
 * PAPI-based integration test for web3-storage.
 *
 * Replaces the bash demo orchestration with a single script that:
 *  1. Sets up provider, bucket, and agreement (on-chain)
 *  2. Uploads data to the provider (HTTP) and verifies it
 *  3. Submits two challenges and responds to both
 *  4. Asserts exactly 2 ChallengeDefended events
 *
 * Prerequisites:
 *   - Parachain running at ws://127.0.0.1:2222
 *   - Provider node running at the specified URL
 *   - Descriptors generated: npm run papi:generate
 *
 * Usage: node full-flow.js [chain_ws] [provider_url] [provider_seed] [client_seed]
 *
 * Arguments:
 *   chain_ws       - WebSocket URL for parachain (default: ws://127.0.0.1:2222)
 *   provider_url   - HTTP URL for provider node (default: http://127.0.0.1:3333)
 *   provider_seed  - Provider identity seed (default: //Alice)
 *   client_seed    - Client/challenger identity seed (default: //Bob)
 */

import { createClient } from "polkadot-api";
import { getWsProvider } from "polkadot-api/ws-provider";
import { getPolkadotSigner } from "polkadot-api/signer";
import { Binary, Enum } from "@polkadot-api/substrate-bindings";
import { Keyring } from "@polkadot/keyring";
import { cryptoWaitReady, blake2AsU8a } from "@polkadot/util-crypto";
import { parachain } from "@polkadot-api/descriptors";
import assert from "node:assert";

// ---------------------------------------------------------------------------
// Config
// ---------------------------------------------------------------------------

const CHAIN_WS = process.argv[2] || "ws://127.0.0.1:2222";
const PROVIDER_URL = process.argv[3] || "http://127.0.0.1:3333";
const PROVIDER_SEED = process.argv[4] || "//Alice";
const CLIENT_SEED = process.argv[5] || "//Bob";

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

function makeSigner(seed) {
  const keyring = new Keyring({ type: "sr25519" });
  const account = keyring.addFromUri(seed);
  return {
    signer: getPolkadotSigner(account.publicKey, "Sr25519", (input) =>
      account.sign(input)
    ),
    address: account.address,
    publicKey: account.publicKey,
  };
}

function toHex(bytes) {
  return (
    "0x" +
    Array.from(bytes instanceof Uint8Array ? bytes : new Uint8Array(bytes))
      .map((b) => b.toString(16).padStart(2, "0"))
      .join("")
  );
}

function hexToBytes(hex) {
  const h = hex.startsWith("0x") ? hex.slice(2) : hex;
  const bytes = new Uint8Array(h.length / 2);
  for (let i = 0; i < bytes.length; i++) {
    bytes[i] = parseInt(h.substr(i * 2, 2), 16);
  }
  return bytes;
}

async function providerFetch(path, opts = {}) {
  const url = new URL(path, PROVIDER_URL);
  if (opts.params) {
    for (const [k, v] of Object.entries(opts.params))
      url.searchParams.set(k, v);
  }
  const resp = await fetch(url, {
    method: opts.method || "GET",
    headers: opts.body ? { "Content-Type": "application/json" } : undefined,
    body: opts.body ? JSON.stringify(opts.body) : undefined,
  });
  if (!resp.ok) throw new Error(`${path}: ${resp.status} ${await resp.text()}`);
  return resp.json();
}

// ---------------------------------------------------------------------------
// Steps
// ---------------------------------------------------------------------------

async function registerProvider(api, provider) {
  const existing = await api.query.StorageProvider.Providers.getValue(provider.address);
  if (existing) {
    console.log("  Provider already registered");
    // Ensure settings have nonzero price for payment testing
    await updateProviderSettings(api, provider);
    return;
  }
  console.log("  Registering provider (%s)...", PROVIDER_SEED);
  // Use the provider URL port as part of the multiaddr
  const providerUrlObj = new URL(PROVIDER_URL);
  const multiaddr = new TextEncoder().encode(`/ip4/127.0.0.1/tcp/${providerUrlObj.port}`);
  await api.tx.StorageProvider.register_provider({
    multiaddr: Binary.fromBytes(multiaddr),
    public_key: Binary.fromBytes(provider.publicKey),
    stake: 1_000_000_000_000_000n, // 1000 tokens
  }).signAndSubmit(provider.signer);
  console.log("  Provider registered");
  await updateProviderSettings(api, provider);
}

async function updateProviderSettings(api, provider) {
  console.log("  Updating provider settings (price_per_byte=1)...");
  const settings = {
    min_duration: 10,
    max_duration: 100_000,
    price_per_byte: 1n,
    accepting_primary: true,
    replica_sync_price: undefined,
    accepting_extensions: true,
    max_capacity: 0n,
  };
  // Try named param first (PAPI typed), fall back to positional
  try {
    await api.tx.StorageProvider.update_provider_settings({ settings }).signAndSubmit(provider.signer);
  } catch {
    await api.tx.StorageProvider.update_provider_settings(settings).signAndSubmit(provider.signer);
  }
  console.log("  Settings updated");
}

async function createBucket(api, client) {
  console.log("  Creating bucket...");
  const result = await api.tx.StorageProvider.create_bucket({
    min_providers: 1,
  }).signAndSubmit(client.signer);

  // Extract bucket ID from BucketCreated event
  const events = api.event.StorageProvider.BucketCreated.filter(result.events);
  if (events.length === 0) {
    throw new Error("No BucketCreated event found");
  }
  const bucketId = events[0].bucket_id;
  console.log("  Bucket created with ID:", bucketId);
  return bucketId;
}

async function createAgreement(api, provider, client, bucketId) {
  const existing = await api.query.StorageProvider.StorageAgreements.getValue(
    bucketId,
    provider.address
  );
  if (existing) {
    console.log("  Agreement already exists");
    return;
  }
  // Short duration (30 blocks ≈ 3 min) with nonzero payment for earnings testing.
  // payment = price_per_byte(1) * max_bytes(1GB) * duration(30) = 30 * 1073741824 ≈ 32 billion planck
  const agreementMaxBytes = 1073741824n; // 1 GB
  const agreementDuration = 5;
  const maxPayment = agreementMaxBytes * BigInt(agreementDuration) * 2n; // 2x buffer
  console.log("  Requesting agreement (%s), duration=%d blocks, maxPayment=%s...", CLIENT_SEED, agreementDuration, maxPayment);
  await api.tx.StorageProvider.request_primary_agreement({
    bucket_id: bucketId,
    provider: provider.address,
    max_bytes: agreementMaxBytes,
    duration: agreementDuration,
    max_payment: maxPayment,
  }).signAndSubmit(client.signer);
  console.log("  Agreement requested");

  console.log("  Accepting agreement (%s)...", PROVIDER_SEED);
  await api.tx.StorageProvider.accept_agreement({
    bucket_id: bucketId,
  }).signAndSubmit(provider.signer);
  console.log("  Agreement accepted");
}

async function uploadData(api, bucketId) {
  const data = new TextEncoder().encode(
    `Hello, Web3 Storage! [${new Date().toISOString()}] provider=${PROVIDER_SEED}`
  );
  const chunkHash = blake2AsU8a(data);
  const chunkHashHex = toHex(chunkHash);

  console.log("  Uploading chunk (%d bytes) to bucket %s...", data.length, bucketId);
  await providerFetch("/node", {
    method: "PUT",
    body: {
      bucket_id: Number(bucketId),
      hash: chunkHashHex,
      data: Buffer.from(data).toString("base64"),
      children: null,
    },
  });

  console.log("  Committing to MMR...");
  const commitResp = await providerFetch("/commit", {
    method: "POST",
    body: {
      bucket_id: Number(bucketId),
      data_roots: [chunkHashHex],
    },
  });
  console.log("  MMR root:", commitResp.mmr_root);
  console.log("  Leaf indices:", commitResp.leaf_indices);

  console.log("  Verifying upload...");
  const downloaded = await providerFetch("/node", {
    params: { hash: chunkHashHex },
  });
  const downloadedData = Buffer.from(downloaded.data, "base64");
  assert.deepStrictEqual(
    downloadedData,
    Buffer.from(data),
    "Downloaded data does not match uploaded data"
  );
  console.log("  Upload verified: data matches (%d bytes)", data.length);

  return {
    leafIndex: commitResp.leaf_indices[0],
    mmrRoot: commitResp.mmr_root,
    startSeq: commitResp.start_seq,
    providerSignature: commitResp.provider_signature,
  };
}

async function challengeOffchain(api, provider, client, upload, bucketId) {
  console.log("  Submitting challenge_offchain with:");
  console.log("    bucket_id:", bucketId);
  console.log("    provider:", provider.address);
  console.log("    mmr_root:", upload.mmrRoot);
  console.log("    start_seq:", upload.startSeq);
  console.log("    leaf_index:", upload.leafIndex);
  console.log("    provider_signature:", upload.providerSignature.slice(0, 20) + "...");

  const result = await api.tx.StorageProvider.challenge_offchain({
    bucket_id: bucketId,
    provider: provider.address,
    mmr_root: Binary.fromBytes(hexToBytes(upload.mmrRoot)),
    start_seq: BigInt(upload.startSeq),
    leaf_index: BigInt(upload.leafIndex),
    chunk_index: 0n,
    provider_signature: Enum("Sr25519", Binary.fromBytes(hexToBytes(upload.providerSignature))),
  }).signAndSubmit(client.signer);

  // Check for extrinsic failure
  const failedEvents = api.event.System.ExtrinsicFailed.filter(result.events);
  if (failedEvents.length > 0) {
    console.log("  ERROR: Extrinsic failed!");
    for (const e of failedEvents) {
      console.log("    dispatch_error:", JSON.stringify(e.dispatch_error, null, 2));
    }
  }

  const events = api.event.StorageProvider.ChallengeCreated.filter(result.events);
  assert.strictEqual(events.length, 1, "Expected 1 ChallengeCreated from off-chain challenge");
  const challengeId = events[0].challenge_id;
  console.log("  Challenge created: deadline=%s, index=%s", challengeId.deadline, challengeId.index);
  return challengeId;
}

async function submitCheckpoint(api, provider, client, bucketId) {
  const checkpointSig = await providerFetch("/checkpoint-signature", {
    params: { bucket_id: Number(bucketId) },
  });
  console.log("  Checkpoint mmr_root:", checkpointSig.mmr_root);
  console.log("  Checkpoint leaf_count:", checkpointSig.leaf_count);

  await api.tx.StorageProvider.checkpoint({
    bucket_id: bucketId,
    mmr_root: Binary.fromBytes(hexToBytes(checkpointSig.mmr_root)),
    start_seq: BigInt(checkpointSig.start_seq),
    leaf_count: BigInt(checkpointSig.leaf_count),
    signatures: [
      [provider.address, Enum("Sr25519", Binary.fromBytes(hexToBytes(checkpointSig.provider_signature)))],
    ],
  }).signAndSubmit(client.signer);
  console.log("  Checkpoint submitted");
}

async function challengeCheckpoint(api, provider, client, leafIndex, bucketId) {
  const result = await api.tx.StorageProvider.challenge_checkpoint({
    bucket_id: bucketId,
    provider: provider.address,
    leaf_index: BigInt(leafIndex),
    chunk_index: 0n,
  }).signAndSubmit(client.signer);

  const events = api.event.StorageProvider.ChallengeCreated.filter(result.events);
  assert.strictEqual(events.length, 1, "Expected 1 ChallengeCreated from checkpoint challenge");
  const challengeId = events[0].challenge_id;
  console.log("  Challenge created: deadline=%s, index=%s", challengeId.deadline, challengeId.index);
  return challengeId;
}

async function respondToChallenge(api, provider, challengeId, bucketId) {
  const challenges = await api.query.StorageProvider.Challenges.getValue(
    challengeId.deadline
  );
  if (!challenges) throw new Error("No challenges at deadline " + challengeId.deadline);

  const challenge = challenges[challengeId.index];
  if (!challenge) throw new Error("Challenge index not found: " + challengeId.index);

  const challengeBucketId = challenge.bucket_id;
  const leafIdx = challenge.leaf_index;
  const chunkIdx = challenge.chunk_index;

  const mmrProofResp = await providerFetch("/mmr_proof", {
    params: { bucket_id: Number(challengeBucketId), leaf_index: Number(leafIdx) },
  });

  const chunkProofResp = await providerFetch("/chunk_proof", {
    params: { data_root: mmrProofResp.leaf.data_root, chunk_index: Number(chunkIdx) },
  });

  await api.tx.StorageProvider.respond_to_challenge({
    challenge_id: challengeId,
    response: Enum("Proof", {
      chunk_data: Binary.fromBytes(Buffer.from(chunkProofResp.chunk_data, "base64")),
      mmr_proof: {
        peaks: mmrProofResp.proof.peaks.map((h) => Binary.fromBytes(hexToBytes(h))),
        leaf: {
          data_root: Binary.fromBytes(hexToBytes(mmrProofResp.leaf.data_root)),
          data_size: BigInt(mmrProofResp.leaf.data_size),
          total_size: BigInt(mmrProofResp.leaf.total_size),
        },
        leaf_proof: {
          siblings: mmrProofResp.proof.siblings.map((h) => Binary.fromBytes(hexToBytes(h))),
          path: mmrProofResp.proof.path,
        },
      },
      chunk_proof: {
        siblings: chunkProofResp.proof.siblings.map((h) => Binary.fromBytes(hexToBytes(h))),
        path: chunkProofResp.proof.path,
      },
    }),
  }).signAndSubmit(provider.signer);
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

  const papi = createClient(getWsProvider(CHAIN_WS));
  const api = papi.getTypedApi(parachain);

  const defendedEvents = [];
  const eventSub = api.event.StorageProvider.ChallengeDefended.watch().subscribe(
    (event) => {
      console.log("  >> ChallengeDefended event:", {
        deadline: event.payload.challenge_id.deadline,
        index: event.payload.challenge_id.index,
      });
      defendedEvents.push(event);
    }
  );

  try {
    console.log("\n=== Step 1: Setup ===");
    await registerProvider(api, provider);
    const bucketId = await createBucket(api, client);
    await createAgreement(api, provider, client, bucketId);

    console.log("\n=== Step 2: Upload data ===");
    const upload = await uploadData(api, bucketId);

    console.log("\n=== Step 3: Off-chain challenge ===");
    const challengeId1 = await challengeOffchain(api, provider, client, upload, bucketId);

    console.log("\n=== Step 4: Respond to off-chain challenge ===");
    await respondToChallenge(api, provider, challengeId1, bucketId);
    console.log("  Challenge defended");

    console.log("\n=== Step 5: Submit checkpoint ===");
    await submitCheckpoint(api, provider, client, bucketId);

    console.log("\n=== Step 6: On-chain checkpoint challenge ===");
    const challengeId2 = await challengeCheckpoint(api, provider, client, upload.leafIndex, bucketId);

    console.log("\n=== Step 7: Respond to checkpoint challenge ===");
    await respondToChallenge(api, provider, challengeId2, bucketId);
    console.log("  Challenge defended");

    console.log("\n=== Verifying challenge results ===");
    await new Promise((r) => setTimeout(r, 3000));
    console.log("ChallengeDefended events: %d (expected: 2)", defendedEvents.length);
    assert.strictEqual(
      defendedEvents.length,
      2,
      `Expected 2 ChallengeDefended events, got ${defendedEvents.length}`
    );
    console.log("PASSED: Both challenges were defended!");

    console.log("\n=== Step 8: Wait for agreement expiry & claim payment ===");
    // Get agreement details to find expiry block
    const agreement = await api.query.StorageProvider.StorageAgreements.getValue(
      bucketId,
      provider.address
    );
    const expiresAt = Number(agreement.expires_at);
    console.log("  Agreement expires at block:", expiresAt);

    // Get provider balance before payment
    const balanceBefore = await api.query.System.Account.getValue(provider.address);
    const freeBefore = balanceBefore.data.free;
    console.log("  Provider balance before:", freeBefore.toString());

    // Wait for expiry
    console.log("  Waiting for agreement to expire...");
    await new Promise((resolve) => {
      const sub = papi.finalizedBlock$.subscribe((block) => {
        if (block.number % 5 === 0) {
          console.log("    Block %d / %d", block.number, expiresAt);
        }
        if (block.number > expiresAt) {
          sub.unsubscribe();
          resolve();
        }
      });
    });

    // Owner (Bob) ends agreement with Pay action
    console.log("  Ending agreement with Pay action (%s)...", CLIENT_SEED);
    await api.tx.StorageProvider.end_agreement({
      bucket_id: bucketId,
      provider: provider.address,
      action: Enum("Pay"),
    }).signAndSubmit(client.signer);
    console.log("  Agreement ended with payment");

    // Check provider balance after payment
    const balanceAfter = await api.query.System.Account.getValue(provider.address);
    const freeAfter = balanceAfter.data.free;
    const earned = freeAfter - freeBefore;
    console.log("  Provider balance after:", freeAfter.toString());
    console.log("  Earned from agreement:", earned.toString());
    assert.ok(earned > 0n, `Expected provider to earn tokens, got ${earned}`);
    console.log("PASSED: Provider received payment!");
  } catch (err) {
    console.error("\nERROR:", err.message || err);
    if (err.stack) console.error(err.stack);
    process.exitCode = 1;
  } finally {
    eventSub.unsubscribe();
    papi.destroy();
  }
}

main().then(() => {
  console.log("\n=== Demo complete! ===");
});
