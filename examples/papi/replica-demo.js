// SPDX-License-Identifier: Apache-2.0

/**
 * End-to-end replica sync demo.
 *
 * Flow:
 *  1. Register primary (Alice) + replica (Charlie) on-chain
 *  2. Open a primary storage agreement → bucket_id
 *  3. Open a replica agreement for Charlie on the same bucket
 *  4. Upload data to primary and commit
 *  5. Submit a checkpoint so the chain's MMR root advances
 *  6. Wait for Charlie's replica-sync coordinator to poll and sync
 *  7. Verify Charlie holds the uploaded data
 *
 * Prerequisites:
 *   - Parachain running at ws://127.0.0.1:2222
 *   - Primary provider running:  just start-provider
 *   - Replica provider running:  just start-replica
 *   - Descriptors generated:     npm run papi:generate (inside examples/papi/)
 *
 * Usage:
 *   node replica-demo.js [chain_ws] [primary_url] [replica_url] \
 *                        [primary_seed] [replica_seed] [client_seed]
 */

import assert from "node:assert";
import {
  establishReplicaAgreement,
  establishStorageAgreement,
  negotiateTerms,
  registerProvider,
  submitClientCheckpoint,
  updateProviderSettings,
  uploadChunk,
  fetchCheckpointSignature,
  downloadChunk,
} from "./api.js";
import {
  connect,
  ensureProviderRegistered,
  makeSigner,
  READ_OPTS,
  waitForBlockProduction,
  waitForChainReady,
  waitForNextBlock,
} from "./common.js";

// ---------------------------------------------------------------------------
// Argument parsing
// ---------------------------------------------------------------------------

const argv = process.argv;
const CHAIN_WS      = argv[2] || "ws://127.0.0.1:2222";
const PRIMARY_URL   = argv[3] || "http://127.0.0.1:3333";
const REPLICA_URL   = argv[4] || "http://127.0.0.1:3334";
const PRIMARY_SEED  = argv[5] || "//Alice";
const REPLICA_SEED  = argv[6] || "//Charlie";
const CLIENT_SEED   = argv[7] || "//Bob";

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

async function openPrimaryAgreement(api, primaryUrl, client, primary) {
  const signed = await negotiateTerms(primaryUrl, {
    owner: client.address,
    max_bytes: 1_073_741_824n,
    duration: 500,
    price_per_byte: 100n,
    replica_params: null,
    bucket_id: null,
  });
  const bucketId = await establishStorageAgreement(api, client, primary, signed);
  console.log("  Bucket", bucketId, "opened with primary agreement");
  return bucketId;
}

async function openReplicaAgreement(api, replicaUrl, client, replica, bucketId) {
  // replica_sync_price must be set on the replica's settings so /negotiate
  // accepts requests with replica_params — validated in negotiate.rs.
  const signed = await negotiateTerms(replicaUrl, {
    owner: client.address,
    max_bytes: 1_073_741_824n,
    duration: 500,
    price_per_byte: 100n,
    replica_params: {
      sync_balance: "100000000000000000",
      min_sync_interval: 5,
      sync_price: "1000000000000"
    },
    bucket_id: bucketId.toString(),
  });
  await establishReplicaAgreement(api, client, replica, bucketId, signed);
  console.log("  Replica agreement opened for", replica.address, "on bucket", bucketId);
}

/** Poll GET /node on replicaUrl until the hash is found or we time out. */
async function waitForReplicaSync(replicaUrl, hash, { timeoutMs = 120_000, pollMs = 6_000 } = {}) {
  const deadline = Date.now() + timeoutMs;
  let attempts = 0;
  while (Date.now() < deadline) {
    attempts++;
    try {
      const res = await fetch(`${replicaUrl}/node?hash=${hash}`);
      if (res.ok) {
        const body = await res.json();
        console.log(`  Replica has the chunk after ${attempts} poll(s)`);
        return body;
      }
    } catch {
      // network hiccup — keep polling
    }
    console.log(`  [poll ${attempts}] waiting for replica to sync...`);
    await new Promise((r) => setTimeout(r, pollMs));
  }
  throw new Error(`Replica did not sync chunk ${hash} within ${timeoutMs}ms`);
}

// ---------------------------------------------------------------------------
// Main
// ---------------------------------------------------------------------------

async function main() {
  const primary = makeSigner(PRIMARY_SEED);
  const replica = makeSigner(REPLICA_SEED);
  const client  = makeSigner(CLIENT_SEED);

  console.log("Chain:    ", CHAIN_WS);
  console.log("Primary:  ", PRIMARY_URL, " =>", primary.address);
  console.log("Replica:  ", REPLICA_URL, " =>", replica.address);
  console.log("Client:   ", client.address);

  const { papi, api } = await connect(CHAIN_WS);

  try {
    await waitForChainReady(api);
    await waitForBlockProduction(api);
    await waitForNextBlock(papi);

    // ------------------------------------------------------------------
    console.log("\n=== Step 1: Register providers ===");
    await ensureProviderRegistered(api, primary, PRIMARY_URL, {
      minDuration: 10,
      maxDuration: 100_000,
      pricePerByte: 1n,
      acceptingPrimary: true,
      replicaSyncPrice: undefined,
      acceptingExtensions: true,
      maxCapacity: 0n,
    });
    await ensureProviderRegistered(api, replica, REPLICA_URL, {
      minDuration: 10,
      maxDuration: 100_000,
      pricePerByte: 10n,
      acceptingPrimary: false,
      replicaSyncPrice: 10n,
      acceptingExtensions: true,
      maxCapacity: 0n,
    });

    // ------------------------------------------------------------------
    console.log("\n=== Step 2: Open primary agreement ===");
    const bucketId = await openPrimaryAgreement(api, PRIMARY_URL, client, primary);

    // ------------------------------------------------------------------
    console.log("\n=== Step 3: Open replica agreement ===");
    await openReplicaAgreement(api, REPLICA_URL, client, replica, bucketId);

    // ------------------------------------------------------------------
    console.log("\n=== Step 4: Upload data to primary ===");
    const payload = `replica-demo data [bucket=${bucketId}]`;
    const { hash, data, commit } = await uploadChunk(PRIMARY_URL, bucketId, payload);
    console.log("  Uploaded", data.length, "bytes, hash:", hash);
    console.log("  MMR root:", commit.mmr_root);

    // ------------------------------------------------------------------
    console.log("\n=== Step 5: Submit checkpoint ===");
    // The chain's BucketSnapshot.mmr_root advances here — this is what the
    // replica coordinator compares against during its poll cycle.
    const ck = await fetchCheckpointSignature(PRIMARY_URL, bucketId);
    await submitClientCheckpoint(api, client, primary, bucketId, ck);
    console.log("  Checkpoint submitted (chain MMR root:", ck.mmr_root, ")");

    // ------------------------------------------------------------------
    console.log("\n=== Step 6: Wait for replica sync ===");
    // The replica coordinator polls every 12s by default. We poll /node on the
    // replica until it has the chunk, timing out after 60s.
    console.log("  Replica poll interval: 12s (default). Polling for up to 60s...");
    await waitForReplicaSync(REPLICA_URL, hash);

    // ------------------------------------------------------------------
    console.log("\n=== Step 7: Verify replica data ===");
    const downloaded = await downloadChunk(REPLICA_URL, hash);
    assert.deepStrictEqual(
      downloaded,
      Buffer.from(data),
      "Replica data does not match original"
    );
    console.log("  Data verified on replica (%d bytes)", downloaded.length);

    console.log("\nPASSED: replica synced and data matches primary ✓");
  } catch (err) {
    console.error("\nERROR:", err.message || err);
    if (err.stack) console.error(err.stack);
    process.exitCode = 1;
  } finally {
    papi.destroy();
  }
}

main().then(() => console.log("\n=== Replica demo complete! ==="));
