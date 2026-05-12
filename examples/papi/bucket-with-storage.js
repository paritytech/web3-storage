/**
 * Atomic bucket-and-agreement quickstart for pallet-storage-provider.
 *
 * full-flow.js sets up a bucket and agreement step-by-step (create_bucket ->
 * request_primary_agreement -> accept_agreement). This example uses the
 * shortcut extrinsic create_bucket_with_storage, which performs all three in
 * one transaction by auto-matching a provider that meets the requested
 * price / duration / capacity.
 *
 * The script then uploads a single chunk, submits a checkpoint, and finally
 * calls freeze_bucket — which becomes possible once a snapshot exists.
 *
 * Prerequisites:
 *   - Parachain at ws://127.0.0.1:2222
 *   - Provider node running and registered as //Alice with accepting_primary=true
 *     (run full-flow.js once to set that up, or this script will do it)
 *
 * Usage: node bucket-with-storage.js [chain_ws] [provider_url] [provider_seed] [client_seed]
 */

import { Binary, Enum } from "@polkadot-api/substrate-bindings";
import { blake2AsU8a, cryptoWaitReady } from "@polkadot/util-crypto";
import {
  connect,
  makeSigner,
  toHex,
  hexToBytes,
  providerFetch,
  ensureProviderRegistered,
  requireOneEvent,
} from "./common.js";

const CHAIN_WS = process.argv[2] || "ws://127.0.0.1:2222";
const PROVIDER_URL = process.argv[3] || "http://127.0.0.1:3333";
const PROVIDER_SEED = process.argv[4] || "//Alice";
const CLIENT_SEED = process.argv[5] || "//Bob";

async function createBucketWithStorage(api, client, params) {
  const result = await api.tx.StorageProvider.create_bucket_with_storage(
    params
  ).signAndSubmit(client.signer);
  const created = requireOneEvent(
    result.events,
    api.event.StorageProvider.BucketCreated,
    "BucketCreated"
  );
  const accepted = requireOneEvent(
    result.events,
    api.event.StorageProvider.AgreementAccepted,
    "AgreementAccepted"
  );
  console.log("  bucket_id        =", created.bucket_id);
  console.log("  matched provider =", accepted.provider);
  console.log("  expires_at       =", accepted.expires_at);
  return { bucketId: created.bucket_id, provider: accepted.provider };
}

async function uploadChunk(providerUrl, bucketId, payload) {
  const data = new TextEncoder().encode(payload);
  const hash = toHex(blake2AsU8a(data));
  await providerFetch(providerUrl, "/node", {
    method: "PUT",
    body: {
      bucket_id: Number(bucketId),
      hash,
      data: Buffer.from(data).toString("base64"),
      children: null,
    },
  });
  const commit = await providerFetch(providerUrl, "/commit", {
    method: "POST",
    body: { bucket_id: Number(bucketId), data_roots: [hash] },
  });
  console.log("  uploaded %d bytes, mmr_root=%s", data.length, commit.mmr_root);
  return commit;
}

async function submitCheckpoint(api, provider, client, bucketId) {
  const ck = await providerFetch(PROVIDER_URL, "/checkpoint-signature", {
    params: { bucket_id: Number(bucketId) },
  });
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
  console.log("  Checkpoint submitted (leaf_count=%s)", ck.leaf_count);
}

async function freezeBucket(api, client, bucketId) {
  const result = await api.tx.StorageProvider.freeze_bucket({
    bucket_id: bucketId,
  }).signAndSubmit(client.signer);
  const event = requireOneEvent(
    result.events,
    api.event.StorageProvider.BucketFrozen,
    "BucketFrozen"
  );
  console.log("  BucketFrozen at start_seq=%s", event.frozen_start_seq);
}

async function main() {
  await cryptoWaitReady();

  const provider = makeSigner(PROVIDER_SEED);
  const client = makeSigner(CLIENT_SEED);

  console.log("Chain:", CHAIN_WS, " Provider HTTP:", PROVIDER_URL);
  console.log("Provider (%s) => %s", PROVIDER_SEED, provider.address);
  console.log("Client   (%s) => %s", CLIENT_SEED, client.address);

  const { papi, api } = await connect(CHAIN_WS);

  try {
    console.log("\n=== Step 1: Ensure provider is registered & accepting ===");
    await ensureProviderRegistered(api, provider, PROVIDER_URL);

    console.log("\n=== Step 2: create_bucket_with_storage (atomic) ===");
    const { bucketId } = await createBucketWithStorage(api, client, {
      max_bytes: 1_048_576n,
      duration: 50,
      max_price_per_byte: 10n,
    });

    console.log("\n=== Step 3: Upload one chunk ===");
    await uploadChunk(
      PROVIDER_URL,
      bucketId,
      `Quickstart payload @ ${new Date().toISOString()}`
    );

    console.log("\n=== Step 4: Submit checkpoint ===");
    await submitCheckpoint(api, provider, client, bucketId);

    console.log("\n=== Step 5: Freeze bucket (now possible) ===");
    await freezeBucket(api, client, bucketId);

    console.log("PASSED: atomic-setup + upload + freeze flow complete");
  } catch (err) {
    console.error("\nERROR:", err.message || err);
    if (err.stack) console.error(err.stack);
    process.exitCode = 1;
  } finally {
    papi.destroy();
  }
}

main().then(() => console.log("\n=== Done ==="));
