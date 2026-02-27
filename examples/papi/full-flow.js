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
 *   - Provider node running at http://127.0.0.1:3333
 *   - Descriptors generated: npm run papi:generate
 *
 * Usage: node full-flow.js [chain_ws] [provider_url]
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
const BUCKET_ID = 1n;

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

async function registerProvider(api, alice) {
  const existing = await api.query.StorageProvider.Providers.getValue(alice.address);
  if (existing) {
    console.log("  Provider already registered");
    return;
  }
  console.log("  Registering provider (Alice)...");
  const multiaddr = new TextEncoder().encode("/ip4/127.0.0.1/tcp/3333");
  await api.tx.StorageProvider.register_provider({
    multiaddr: Binary.fromBytes(multiaddr),
    public_key: Binary.fromBytes(alice.publicKey),
    stake: 1_000_000_000_000_000n, // 1000 tokens
  }).signAndSubmit(alice.signer);
  console.log("  Provider registered");
}

async function createBucket(api, bob) {
  const existing = await api.query.StorageProvider.Buckets.getValue(BUCKET_ID);
  if (existing) {
    console.log("  Bucket already exists");
    return;
  }
  console.log("  Creating bucket...");
  await api.tx.StorageProvider.create_bucket({
    min_providers: 1,
  }).signAndSubmit(bob.signer);
  console.log("  Bucket created");
}

async function createAgreement(api, alice, bob) {
  const existing = await api.query.StorageProvider.StorageAgreements.getValue(
    BUCKET_ID,
    alice.address
  );
  if (existing) {
    console.log("  Agreement already exists");
    return;
  }
  console.log("  Requesting agreement (Bob)...");
  await api.tx.StorageProvider.request_primary_agreement({
    bucket_id: BUCKET_ID,
    provider: alice.address,
    max_bytes: 1073741824n, // 1 GB
    duration: 100_000,
    max_payment: 100_000_000_000n,
  }).signAndSubmit(bob.signer);
  console.log("  Agreement requested");

  console.log("  Accepting agreement (Alice)...");
  await api.tx.StorageProvider.accept_agreement({
    bucket_id: BUCKET_ID,
  }).signAndSubmit(alice.signer);
  console.log("  Agreement accepted");
}

async function uploadData(api) {
  const data = new TextEncoder().encode(
    `Hello, Web3 Storage! [${new Date().toISOString()}]`
  );
  const chunkHash = blake2AsU8a(data);
  const chunkHashHex = toHex(chunkHash);

  console.log("  Uploading chunk (%d bytes)...", data.length);
  await providerFetch("/node", {
    method: "PUT",
    body: {
      bucket_id: Number(BUCKET_ID),
      hash: chunkHashHex,
      data: Buffer.from(data).toString("base64"),
      children: null,
    },
  });

  console.log("  Committing to MMR...");
  const commitResp = await providerFetch("/commit", {
    method: "POST",
    body: {
      bucket_id: Number(BUCKET_ID),
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

async function challengeOffchain(api, alice, bob, upload) {
  console.log("  Submitting challenge_offchain with:");
  console.log("    bucket_id:", BUCKET_ID);
  console.log("    provider:", alice.address);
  console.log("    mmr_root:", upload.mmrRoot);
  console.log("    start_seq:", upload.startSeq);
  console.log("    leaf_index:", upload.leafIndex);
  console.log("    provider_signature:", upload.providerSignature.slice(0, 20) + "...");

  const result = await api.tx.StorageProvider.challenge_offchain({
    bucket_id: BUCKET_ID,
    provider: alice.address,
    mmr_root: Binary.fromBytes(hexToBytes(upload.mmrRoot)),
    start_seq: BigInt(upload.startSeq),
    leaf_index: BigInt(upload.leafIndex),
    chunk_index: 0n,
    provider_signature: Enum("Sr25519", Binary.fromBytes(hexToBytes(upload.providerSignature))),
  }).signAndSubmit(bob.signer);

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

async function submitCheckpoint(api, alice, bob) {
  const checkpointSig = await providerFetch("/checkpoint-signature", {
    params: { bucket_id: Number(BUCKET_ID) },
  });
  console.log("  Checkpoint mmr_root:", checkpointSig.mmr_root);
  console.log("  Checkpoint leaf_count:", checkpointSig.leaf_count);

  await api.tx.StorageProvider.checkpoint({
    bucket_id: BUCKET_ID,
    mmr_root: Binary.fromBytes(hexToBytes(checkpointSig.mmr_root)),
    start_seq: BigInt(checkpointSig.start_seq),
    leaf_count: BigInt(checkpointSig.leaf_count),
    signatures: [
      [alice.address, Enum("Sr25519", Binary.fromBytes(hexToBytes(checkpointSig.provider_signature)))],
    ],
  }).signAndSubmit(bob.signer);
  console.log("  Checkpoint submitted");
}

async function challengeCheckpoint(api, alice, bob, leafIndex) {
  const result = await api.tx.StorageProvider.challenge_checkpoint({
    bucket_id: BUCKET_ID,
    provider: alice.address,
    leaf_index: BigInt(leafIndex),
    chunk_index: 0n,
  }).signAndSubmit(bob.signer);

  const events = api.event.StorageProvider.ChallengeCreated.filter(result.events);
  assert.strictEqual(events.length, 1, "Expected 1 ChallengeCreated from checkpoint challenge");
  const challengeId = events[0].challenge_id;
  console.log("  Challenge created: deadline=%s, index=%s", challengeId.deadline, challengeId.index);
  return challengeId;
}

async function respondToChallenge(api, provider, challengeId) {
  const challenges = await api.query.StorageProvider.Challenges.getValue(
    challengeId.deadline
  );
  if (!challenges) throw new Error("No challenges at deadline " + challengeId.deadline);

  const challenge = challenges[challengeId.index];
  if (!challenge) throw new Error("Challenge index not found: " + challengeId.index);

  const bucketId = challenge.bucket_id;
  const leafIdx = challenge.leaf_index;
  const chunkIdx = challenge.chunk_index;

  const mmrProofResp = await providerFetch("/mmr_proof", {
    params: { bucket_id: Number(bucketId), leaf_index: Number(leafIdx) },
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

  const alice = makeSigner("//Alice"); // provider
  const bob = makeSigner("//Bob"); // client / challenger

  console.log("Connecting to chain:", CHAIN_WS);
  console.log("Provider URL:", PROVIDER_URL);

  const client = createClient(getWsProvider(CHAIN_WS));
  const api = client.getTypedApi(parachain);

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
    await registerProvider(api, alice);
    await createBucket(api, bob);
    await createAgreement(api, alice, bob);

    console.log("\n=== Step 2: Upload data ===");
    const upload = await uploadData(api);

    console.log("\n=== Step 3: Off-chain challenge ===");
    const challengeId1 = await challengeOffchain(api, alice, bob, upload);

    console.log("\n=== Step 4: Respond to off-chain challenge ===");
    await respondToChallenge(api, alice, challengeId1);
    console.log("  Challenge defended");

    console.log("\n=== Step 5: Submit checkpoint ===");
    await submitCheckpoint(api, alice, bob);

    console.log("\n=== Step 6: On-chain checkpoint challenge ===");
    const challengeId2 = await challengeCheckpoint(api, alice, bob, upload.leafIndex);

    console.log("\n=== Step 7: Respond to checkpoint challenge ===");
    await respondToChallenge(api, alice, challengeId2);
    console.log("  Challenge defended");

    console.log("\n=== Verifying results ===");
    await new Promise((r) => setTimeout(r, 3000));
    console.log("ChallengeDefended events: %d (expected: 2)", defendedEvents.length);
    assert.strictEqual(
      defendedEvents.length,
      2,
      `Expected 2 ChallengeDefended events, got ${defendedEvents.length}`
    );
    console.log("PASSED: Both challenges were defended!");
  } catch (err) {
    console.error("\nERROR:", err.message || err);
    if (err.stack) console.error(err.stack);
    process.exitCode = 1;
  } finally {
    eventSub.unsubscribe();
    client.destroy();
  }
}

main().then(() => {
  console.log("\n=== Demo complete! ===");
});
