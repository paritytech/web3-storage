// SPDX-License-Identifier: Apache-2.0

/**
 * PAPI-based demo of the whole replica workflow of web3-storage.
 *
 *  1. Register a primary and a replica provider on chain
 *  2. Open a public bucket on the primary, upload a three-chunk file and checkpoint it
 *  3. Add a replica agreement: the replica node syncs the bucket and confirms
 *     on chain (ReplicaSynced), then serves the data itself
 *  4. Upload and checkpoint more data: the replica syncs the difference and
 *     confirms again
 *
 * Prerequisites (each in its own terminal, from the repository root):
 *   just start-e2e-chain
 *   just start-provider                 # primary: //Alice, port 3333
 *   just start-replica-provider         # replica: //Charlie, port 3334, replica sync on
 *   just papi-setup
 *
 * Usage: node --import tsx replica-flow.ts [chain_ws] [primary_url] [replica_url]
 *        [primary_seed] [replica_seed] [client_seed]
 */

import assert from "node:assert";
import {
  addReplicaProvider,
  bytesEq,
  computeCid,
  connect,
  createBucketWithPrimary,
  DEFAULT_CHUNK_SIZE,
  downloadChunk,
  ensureProviderRegistered,
  fetchCheckpointSignature,
  firstMatch,
  getAgreementNonce,
  getProviderNodeInfo,
  makeSigner,
  merkleRoot,
  negotiateTerms,
  providerFetch,
  putChunk,
  putInternalNode,
  READ_OPTS,
  registerProvider,
  submitClientCheckpoint,
  toHex,
  updateProviderSettings,
  waitForBlockProduction,
  waitForChainReady,
  waitForNextBlock,
  type ChainSigner,
  type ParachainApi,
} from "@web3-storage/sdk";

const CHAIN_WS = process.argv[2] || "ws://127.0.0.1:2222";
const PRIMARY_URL = process.argv[3] || "http://127.0.0.1:3333";
const REPLICA_URL = process.argv[4] || "http://127.0.0.1:3334";
const PRIMARY_SEED = process.argv[5] || "//Alice";
const REPLICA_SEED = process.argv[6] || "//Charlie";
const CLIENT_SEED = process.argv[7] || "//Bob";

/** What the replica charges for each confirmed sync. */
const SYNC_PRICE = 1_000_000_000n;
/**
 * The replica's price per byte. It differs from the primary's, so the wait
 * for the node to read the new settings can tell the two apart.
 */
const REPLICA_PRICE_PER_BYTE = 3n;
/** The sync funding the client locks for the replica: five syncs. */
const SYNC_BALANCE = 5n * SYNC_PRICE;
/** Anchor blocks (6 s each) that must pass between two confirmed syncs. */
const MIN_SYNC_INTERVAL = 1;
/**
 * Anchor blocks the agreements last. A replica skips an agreement that
 * expires within its sync timeout (300 s, 50 blocks), so this leaves room.
 */
const DURATION = 300;
const MAX_BYTES = 16n * 1024n * 1024n;
/** Time the replica node gets to notice the agreement, sync and confirm. */
const SYNC_TIMEOUT_MS = 240_000;

/**
 * Register the replica provider and set it to quote replica agreements. A
 * replica does not need to accept primary agreements.
 */
async function ensureReplicaProvider(api: ParachainApi, replica: ChainSigner, url: string) {
  const existing = await api.query.StorageProvider.Providers.getValue(replica.address, READ_OPTS);
  if (!existing) {
    console.log("  Registering replica provider", replica.address);
    await registerProvider(api, replica, url);
  }
  await updateProviderSettings(api, replica, {
    min_duration: 10,
    max_duration: 100_000,
    price_per_byte: REPLICA_PRICE_PER_BYTE,
    accepting_primary: false,
    replica_sync_price: SYNC_PRICE,
    accepting_extensions: true,
    max_capacity: 0n,
  });

  // The node reads its settings from finalized blocks and rejects /negotiate
  // until it has seen this registration.
  for (let attempt = 0; attempt < 20; attempt++) {
    const { readiness, provider_registration_info } = await getProviderNodeInfo(url);
    if (
      readiness.signing_configured &&
      provider_registration_info != null &&
      BigInt(provider_registration_info.settings.price_per_byte) === REPLICA_PRICE_PER_BYTE
    ) {
      return;
    }
    await new Promise((resolve) => setTimeout(resolve, 3000));
  }
  throw new Error(`Replica node at ${url} did not sync its registration within 60s`);
}

/**
 * Upload a file of `sizes.length` chunks: full-size chunks with data of their
 * own, then the chunk tree over them, then `POST /commit`. Returns the
 * chunks' hashes and the /commit response.
 *
 * The provider signs only the canonical tree: chunks of DEFAULT_CHUNK_SIZE
 * bytes except a shorter last one, and an odd last node moves up a level.
 */
async function uploadFile(
  bucketId: bigint,
  client: ChainSigner,
  sizes: number[],
  seed: number,
): Promise<{ chunks: Uint8Array[]; commit: any }> {
  const chunks = sizes.map((size, i) => new Uint8Array(size).fill(seed + i));
  let level: Uint8Array[] = [];
  for (const chunk of chunks) {
    await putChunk(PRIMARY_URL, bucketId, chunk, client);
    level.push(computeCid(chunk));
  }
  const leaves = level;
  while (level.length > 1) {
    const parents: Uint8Array[] = [];
    for (let i = 0; i < level.length; i += 2) {
      if (i + 1 < level.length) {
        const { cid } = await putInternalNode(PRIMARY_URL, bucketId, level[i], level[i + 1], client);
        parents.push(cid);
      } else {
        parents.push(level[i]);
      }
    }
    level = parents;
  }
  assert.ok(bytesEq(level[0], merkleRoot(leaves)), "the uploaded tree must be the canonical tree");

  const commit = await providerFetch(PRIMARY_URL, "/commit", {
    method: "POST",
    body: { bucket_id: Number(bucketId), data_roots: [toHex(level[0])] },
    sign: { signer: client.signer, bucketId },
  });
  return { chunks, commit };
}

/** The root the chain holds at `position`: 0 for the snapshot, 1-6 for a slot. */
async function rootAt(api: ParachainApi, bucketId: bigint, position: number): Promise<string> {
  const bucket = (await api.query.StorageProvider.Buckets.getValue(bucketId, READ_OPTS))!;
  const commitment = position === 0 ? bucket.snapshot?.commitment : bucket.historical_roots[position - 1]?.[1];
  assert.ok(commitment, `the chain holds no commitment at position ${position}`);
  return commitment.mmr_root as string;
}

/**
 * Wait for the first ReplicaSynced event of `bucketId` that confirms `root`,
 * and return its payload. It subscribes when called, so call it before the
 * action that starts the sync.
 */
async function replicaSynced(api: ParachainApi, bucketId: bigint, root: string) {
  const matches = ({ payload }: { payload: { bucket_id: bigint; mmr_root: string } }) =>
    payload.bucket_id === bucketId && payload.mmr_root.toLowerCase() === root.toLowerCase();
  const { events } = await firstMatch(
    api.event.StorageProvider.ReplicaSynced.watch(),
    ({ events }) => events.some(matches),
    { timeoutMs: SYNC_TIMEOUT_MS, description: `the replica to confirm root ${root}` },
  );
  return events.find(matches)!.payload;
}

/** The replica role of `replica`'s agreement for `bucketId`. */
async function replicaRole(api: ParachainApi, bucketId: bigint, replica: ChainSigner) {
  const agreement = (await api.query.StorageProvider.StorageAgreements.getValue(
    bucketId,
    replica.address,
    READ_OPTS,
  ))!;
  if (agreement.role.type !== "Replica") throw new Error("expected a replica agreement");
  return agreement.role.value;
}

async function checkpoint(
  api: ParachainApi,
  client: ChainSigner,
  primary: ChainSigner,
  bucketId: bigint,
): Promise<string> {
  const ck = await fetchCheckpointSignature(PRIMARY_URL, bucketId, client);
  await submitClientCheckpoint(api, client, primary, bucketId, ck);
  console.log("  Checkpointed root %s (%s leaves)", ck.mmr_root, ck.leaf_count);
  return ck.mmr_root;
}

/** Check what the replica confirmed and what it now serves. */
async function verifyReplica(
  api: ParachainApi,
  bucketId: bigint,
  replica: ChainSigner,
  root: string,
  event: { position_matched: number; sync_payment: bigint },
  files: Uint8Array[][],
) {
  console.log("  ReplicaSynced: position=%d payment=%s", event.position_matched, event.sync_payment);
  assert.strictEqual(event.sync_payment, SYNC_PRICE, "the replica is paid its sync price");
  // The replica signs the root at one position only, so the pallet matches that
  // position.
  const confirmed = await rootAt(api, bucketId, event.position_matched);
  assert.strictEqual(confirmed.toLowerCase(), root.toLowerCase(), "the confirmed position holds the root");

  const { last_sync: lastSync } = await replicaRole(api, bucketId, replica);
  assert.ok(lastSync, "the agreement records the sync");
  assert.strictEqual((lastSync.commitment.mmr_root as string).toLowerCase(), root.toLowerCase());

  const status = await providerFetch(REPLICA_URL, "/replica/sync_status", {
    params: { bucket_id: bucketId },
  });
  assert.strictEqual(status.local_mmr_root.toLowerCase(), root.toLowerCase());
  console.log("  Replica node root matches the chain: %s", status.local_mmr_root);

  // Every chunk reads back from the replica and verifies against its hash.
  for (const chunks of files) {
    for (const chunk of chunks) {
      const downloaded = await downloadChunk(REPLICA_URL, bucketId, toHex(computeCid(chunk)));
      assert.ok(bytesEq(downloaded, chunk), "the replica serves the uploaded chunk");
    }
  }
  console.log("  Read %d chunks back from the replica node", files.flat().length);
}

async function main() {
  const primary = makeSigner(PRIMARY_SEED);
  const replica = makeSigner(REPLICA_SEED);
  const client = makeSigner(CLIENT_SEED);

  console.log("Chain:", CHAIN_WS);
  console.log("Primary:", PRIMARY_URL, "=>", primary.address);
  console.log("Replica:", REPLICA_URL, "=>", replica.address);
  console.log("Client:", client.address);

  const { papi, api } = await connect(CHAIN_WS);
  await waitForChainReady(api);
  await waitForBlockProduction(api);
  await waitForNextBlock(papi);

  try {
    console.log("\n=== Step 1: Register the providers ===");
    await ensureProviderRegistered(api, primary, PRIMARY_URL);
    await ensureReplicaProvider(api, replica, REPLICA_URL);

    console.log("\n=== Step 2: Bucket on the primary, first upload, checkpoint ===");
    const nonce = await getAgreementNonce(api, client.address);
    const primaryTerms = await negotiateTerms(PRIMARY_URL, {
      owner: client.address,
      max_bytes: MAX_BYTES,
      duration: DURATION,
      price_per_byte: 1n,
      nonce,
      replica_params: null,
      bucket: null,
    });
    // Public: a primary of a private bucket refuses a replica's reads of the
    // MMR leaves, and the replica would have to sync from another replica.
    const { bucketId } = await createBucketWithPrimary(api, client, primary, primaryTerms, {
      mode: "finalized",
      visibility: "Public",
    });
    console.log("  Bucket %s opened on the primary", bucketId);

    const first = await uploadFile(bucketId, client, [DEFAULT_CHUNK_SIZE, DEFAULT_CHUNK_SIZE, 10], 1);
    console.log("  Uploaded 3 chunks, root %s", first.commit.mmr_root);
    const firstRoot = await checkpoint(api, client, primary, bucketId);

    console.log("\n=== Step 3: Add the replica; it syncs and confirms ===");
    const replicaTerms = await negotiateTerms(REPLICA_URL, {
      owner: client.address,
      max_bytes: MAX_BYTES,
      duration: DURATION,
      price_per_byte: REPLICA_PRICE_PER_BYTE,
      nonce: await getAgreementNonce(api, client.address),
      // The node reads `replica_params` as JSON numbers only and requires
      // `sync_price`; both values are far below 2^53.
      replica_params: {
        sync_balance: Number(SYNC_BALANCE),
        min_sync_interval: MIN_SYNC_INTERVAL,
        sync_price: Number(SYNC_PRICE),
      },
      bucket: bucketId,
    });
    // Subscribe before the agreement lands: the replica node starts as soon
    // as it sees ReplicaAgreementEstablished.
    const firstSync = replicaSynced(api, bucketId, firstRoot);
    await addReplicaProvider(api, client, replica, replicaTerms);
    console.log("  Replica agreement established; waiting for the replica node...");
    const firstEvent = await firstSync;
    await verifyReplica(api, bucketId, replica, firstRoot, firstEvent, [first.chunks]);

    console.log("\n=== Step 4: More data; the replica syncs the difference ===");
    // The next sync may only confirm after the interval.
    await waitForNextBlock(papi);
    const second = await uploadFile(bucketId, client, [DEFAULT_CHUNK_SIZE, 100], 50);
    console.log("  Uploaded 2 chunks, root %s", second.commit.mmr_root);
    const secondSync = replicaSynced(api, bucketId, second.commit.mmr_root);
    const secondRoot = await checkpoint(api, client, primary, bucketId);
    assert.strictEqual(secondRoot.toLowerCase(), second.commit.mmr_root.toLowerCase());
    const secondEvent = await secondSync;
    await verifyReplica(api, bucketId, replica, secondRoot, secondEvent, [first.chunks, second.chunks]);

    assert.strictEqual(
      (await replicaRole(api, bucketId, replica)).sync_balance,
      SYNC_BALANCE - 2n * SYNC_PRICE,
      "two syncs were paid from the sync balance",
    );
    console.log("\nPASSED: the replica synced, confirmed twice and serves the bucket.");
  } finally {
    papi.destroy();
  }
}

main().catch((err) => {
  console.error("FAILED:", err);
  process.exit(1);
});
