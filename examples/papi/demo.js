/**
 * PAPI-based integration test for web3-storage.
 *
 * Replaces the bash demo orchestration with a single script that:
 *  1. Sets up provider, bucket, and agreement (on-chain)
 *  2. Uploads data to the provider (HTTP)
 *  3. Submits two challenges and responds to both
 *  4. Asserts exactly 2 ChallengeDefended events
 *
 * Prerequisites:
 *   - Parachain running at ws://127.0.0.1:9944
 *   - Provider node running at http://127.0.0.1:3000
 *   - Descriptors generated: npm run papi:generate
 *
 * Usage: node demo.js [chain_ws] [provider_url]
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

const CHAIN_WS = process.argv[2] || "ws://127.0.0.1:9944";
const PROVIDER_URL = process.argv[3] || "http://127.0.0.1:3000";
const BUCKET_ID = 1n;
const ALICE_SS58 = "5GrwvaEF5zXb26Fz9rcQpDWS57CtERHpNehXCPcNoHGKutQY";

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

/** Wait for a condition, polling at interval. */
async function waitFor(description, fn, { intervalMs = 2000, timeoutMs = 120_000 } = {}) {
  const deadline = Date.now() + timeoutMs;
  while (Date.now() < deadline) {
    try {
      const result = await fn();
      if (result) return result;
    } catch { /* keep polling */ }
    await new Promise((r) => setTimeout(r, intervalMs));
  }
  throw new Error(`Timeout waiting for: ${description}`);
}

// ---------------------------------------------------------------------------
// Main
// ---------------------------------------------------------------------------

async function main() {
  await cryptoWaitReady();

  const alice = makeSigner("//Alice"); // provider
  const bob = makeSigner("//Bob"); // client / admin / challenger

  console.log("Connecting to chain:", CHAIN_WS);
  console.log("Provider URL:", PROVIDER_URL);

  const client = createClient(getWsProvider(CHAIN_WS));
  const api = client.getTypedApi(parachain);

  // Collect ChallengeDefended events from finalized blocks
  const defendedEvents = [];
  const eventSub = api.event.StorageProvider.ChallengeDefended.watch().subscribe(
    (event) => {
      console.log("  >> ChallengeDefended event:", {
        deadline: event.payload.challenge_id.deadline,
        index: event.payload.challenge_id.index,
        response_time: event.payload.response_time_blocks,
      });
      defendedEvents.push(event);
    }
  );

  try {
    // ========================================================================
    // Step 1: Setup - register provider, create bucket, agreement
    // ========================================================================
    console.log("\n=== Step 1: Setup ===");

    // 1a. Register a provider (Alice) if not registered
    const providerInfo = await api.query.StorageProvider.Providers.getValue(
      alice.address
    );
    if (!providerInfo) {
      console.log("  Registering provider (Alice)...");
      const multiaddr = new TextEncoder().encode("/ip4/127.0.0.1/tcp/3000");
      await api.tx.StorageProvider.register_provider({
        multiaddr: Binary.fromBytes(multiaddr),
        public_key: Binary.fromBytes(alice.publicKey),
        stake: 1_000_000_000_000_000n, // 1000 tokens
      }).signAndSubmit(alice.signer);
      console.log("  Provider registered");
    } else {
      console.log("  Provider already registered");
    }

    // 1b. Create bucket (Bob) if not exists
    const bucketInfo = await api.query.StorageProvider.Buckets.getValue(
      BUCKET_ID
    );
    if (!bucketInfo) {
      console.log("  Creating bucket...");
      await api.tx.StorageProvider.create_bucket({
        min_providers: 1,
      }).signAndSubmit(bob.signer);
      console.log("  Bucket created");
    } else {
      console.log("  Bucket already exists");
    }

    // 1c. Request + accept agreement if not exists
    const agreement = await api.query.StorageProvider.StorageAgreements.getValue(
      BUCKET_ID,
      alice.address
    );
    if (!agreement) {
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
    } else {
      console.log("  Agreement already exists");
    }

    // ========================================================================
    // Step 2: Upload data to provider
    // ========================================================================
    console.log("\n=== Step 2: Upload data ===");

    const data = new TextEncoder().encode(
      `Hello, Web3 Storage! [${new Date().toISOString()}]`
    );
    const chunkHash = blake2AsU8a(data);
    const chunkHashHex = toHex(chunkHash);
    const dataBase64 = Buffer.from(data).toString("base64");

    // Upload chunk
    console.log("  Uploading chunk (%d bytes)...", data.length);
    await providerFetch("/node", {
      method: "PUT",
      body: {
        bucket_id: Number(BUCKET_ID),
        hash: chunkHashHex,
        data: dataBase64,
        children: null,
      },
    });

    // Commit to MMR (data_root = chunk hash for single-chunk data)
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

    // Verify upload by downloading the chunk back
    console.log("  Verifying upload...");
    const downloaded = await providerFetch("/node", {
      params: { hash: chunkHashHex },
    });
    const downloadedData = Buffer.from(downloaded.data, "base64");
    assert.strictEqual(
      downloadedData.length,
      data.length,
      `Download size mismatch: got ${downloadedData.length}, expected ${data.length}`
    );
    assert.deepStrictEqual(
      downloadedData,
      Buffer.from(data),
      "Downloaded data does not match uploaded data"
    );
    console.log("  Upload verified: data matches (%d bytes)", data.length);

    const leafIndex = commitResp.leaf_indices[0];
    const mmrRootBytes = hexToBytes(commitResp.mmr_root);
    const signatureBytes = hexToBytes(commitResp.provider_signature);

    // ========================================================================
    // Step 3: Off-chain challenge (Bob challenges Alice)
    // ========================================================================
    console.log("\n=== Step 3: Off-chain challenge ===");

    const challenge1Result = await api.tx.StorageProvider.challenge_offchain({
      bucket_id: BUCKET_ID,
      provider: alice.address,
      mmr_root: Binary.fromBytes(mmrRootBytes),
      start_seq: BigInt(commitResp.start_seq),
      leaf_index: BigInt(leafIndex),
      chunk_index: 0n,
      provider_signature: Enum("Sr25519", Binary.fromBytes(signatureBytes)),
    }).signAndSubmit(bob.signer);

    const challenge1Events =
      api.event.StorageProvider.ChallengeCreated.filter(challenge1Result.events);
    assert.strictEqual(challenge1Events.length, 1, "Expected 1 ChallengeCreated from off-chain challenge");
    const challengeId1 = challenge1Events[0].challenge_id;
    console.log("  Challenge created: deadline=%s, index=%s", challengeId1.deadline, challengeId1.index);

    // ========================================================================
    // Step 4: Respond to off-chain challenge (Alice)
    // ========================================================================
    console.log("\n=== Step 4: Respond to off-chain challenge ===");
    await respondToChallenge(api, alice, challengeId1);
    console.log("  Challenge defended");

    // ========================================================================
    // Step 5: Submit on-chain checkpoint (Bob)
    // ========================================================================
    console.log("\n=== Step 5: Submit checkpoint ===");

    const checkpointSig = await providerFetch("/checkpoint-signature", {
      params: { bucket_id: Number(BUCKET_ID) },
    });
    console.log("  Checkpoint mmr_root:", checkpointSig.mmr_root);
    console.log("  Checkpoint leaf_count:", checkpointSig.leaf_count);

    const cpMmrRoot = hexToBytes(checkpointSig.mmr_root);
    const cpSigBytes = hexToBytes(checkpointSig.provider_signature);

    await api.tx.StorageProvider.checkpoint({
      bucket_id: BUCKET_ID,
      mmr_root: Binary.fromBytes(cpMmrRoot),
      start_seq: BigInt(checkpointSig.start_seq),
      leaf_count: BigInt(checkpointSig.leaf_count),
      signatures: [
        [
          alice.address,
          Enum("Sr25519", Binary.fromBytes(cpSigBytes)),
        ],
      ],
    }).signAndSubmit(bob.signer);
    console.log("  Checkpoint submitted");

    // ========================================================================
    // Step 6: On-chain checkpoint challenge (Bob challenges Alice)
    // ========================================================================
    console.log("\n=== Step 6: On-chain checkpoint challenge ===");

    const challenge2Result =
      await api.tx.StorageProvider.challenge_checkpoint({
        bucket_id: BUCKET_ID,
        provider: alice.address,
        leaf_index: BigInt(leafIndex),
        chunk_index: 0n,
      }).signAndSubmit(bob.signer);

    const challenge2Events =
      api.event.StorageProvider.ChallengeCreated.filter(challenge2Result.events);
    assert.strictEqual(challenge2Events.length, 1, "Expected 1 ChallengeCreated from checkpoint challenge");
    const challengeId2 = challenge2Events[0].challenge_id;
    console.log("  Challenge created: deadline=%s, index=%s", challengeId2.deadline, challengeId2.index);

    // ========================================================================
    // Step 7: Respond to checkpoint challenge (Alice)
    // ========================================================================
    console.log("\n=== Step 7: Respond to checkpoint challenge ===");
    await respondToChallenge(api, alice, challengeId2);
    console.log("  Challenge defended");

    // ========================================================================
    // Step 8: Assert 2 ChallengeDefended events
    // ========================================================================
    console.log("\n=== Verifying results ===");

    // Give event subscription a moment to catch up
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

// ---------------------------------------------------------------------------
// Challenge response: fetch proofs from provider and submit on-chain
// ---------------------------------------------------------------------------

async function respondToChallenge(api, provider, challengeId) {
  // Fetch challenge details from storage
  const challenges = await api.query.StorageProvider.Challenges.getValue(
    challengeId.deadline
  );
  if (!challenges) throw new Error("No challenges at deadline " + challengeId.deadline);

  const challenge = challenges[challengeId.index];
  if (!challenge) throw new Error("Challenge index not found: " + challengeId.index);

  const mmrRootHex = toHex(challenge.mmr_root.asBytes());
  const bucketId = challenge.bucket_id;
  const leafIdx = challenge.leaf_index;
  const chunkIdx = challenge.chunk_index;

  // Fetch MMR proof from provider
  const mmrProofResp = await providerFetch("/mmr_proof", {
    params: {
      bucket_id: Number(bucketId),
      leaf_index: Number(leafIdx),
    },
  });

  // Fetch chunk proof from provider
  const chunkProofResp = await providerFetch("/chunk_proof", {
    params: {
      data_root: mmrProofResp.leaf.data_root,
      chunk_index: Number(chunkIdx),
    },
  });

  // Decode chunk data
  const chunkData = Buffer.from(
    chunkProofResp.chunk_data,
    "base64"
  );

  // Build MMR proof
  const mmrProof = {
    peaks: mmrProofResp.proof.peaks.map((h) => Binary.fromBytes(hexToBytes(h))),
    leaf: {
      data_root: Binary.fromBytes(hexToBytes(mmrProofResp.leaf.data_root)),
      data_size: BigInt(mmrProofResp.leaf.data_size),
      total_size: BigInt(mmrProofResp.leaf.total_size),
    },
    leaf_proof: {
      siblings: mmrProofResp.proof.siblings.map((h) =>
        Binary.fromBytes(hexToBytes(h))
      ),
      path: mmrProofResp.proof.path,
    },
  };

  // Build chunk proof
  const chunkProof = {
    siblings: chunkProofResp.proof.siblings.map((h) =>
      Binary.fromBytes(hexToBytes(h))
    ),
    path: chunkProofResp.proof.path,
  };

  // Submit respond_to_challenge
  await api.tx.StorageProvider.respond_to_challenge({
    challenge_id: challengeId,
    response: Enum("Proof", {
      chunk_data: Binary.fromBytes(chunkData),
      mmr_proof: mmrProof,
      chunk_proof: chunkProof,
    }),
  }).signAndSubmit(provider.signer);
}

// ---------------------------------------------------------------------------
// Run
// ---------------------------------------------------------------------------

main().then(() => {
  console.log("\n=== Demo complete! ===");
});
