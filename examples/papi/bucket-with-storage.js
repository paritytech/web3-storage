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

import { cryptoWaitReady } from "@polkadot/util-crypto";
import {
  createBucketWithStorage,
  fetchCheckpointSignature,
  freezeBucket,
  submitClientCheckpoint,
  uploadChunk,
} from "./api.js";
import {
  connect,
  ensureProviderRegistered,
  ensureSoleAcceptingProvider,
  makeSigner,
  parseProviderClientArgs,
  sameAddress,
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

async function main() {
  await cryptoWaitReady();

  const provider = makeSigner(PROVIDER_SEED);
  const client = makeSigner(CLIENT_SEED);

  console.log("Chain:", CHAIN_WS, " Provider HTTP:", PROVIDER_URL);
  console.log("Provider (%s) => %s", PROVIDER_SEED, provider.address);
  console.log("Client   (%s) => %s", CLIENT_SEED, client.address);

  const { papi, api } = await connect(CHAIN_WS);
  await waitForChainReady(api);
  await waitForBlockProduction(api);
  await waitForNextBlock(papi);

  let restoreOthers = null;
  try {
    console.log("\n=== Step 1: Ensure provider is registered & accepting ===");
    await ensureProviderRegistered(api, provider, PROVIDER_URL);
    // CI runs a second provider node (//Charlie) on a different port; both
    // end up registered with accepting_primary=true and create_bucket_with_storage
    // picks the cheapest, breaking ties by Providers::iter() hash order — which
    // made the demo flake when //Charlie won the tie. Silence every other
    // accepting provider for the duration of this demo so the match is
    // deterministic.
    restoreOthers = await ensureSoleAcceptingProvider(api, provider);

    console.log("\n=== Step 2: create_bucket_with_storage (atomic) ===");
    const { bucketId, matchedProvider, expiresAt } = await createBucketWithStorage(
      api,
      client,
      {
        max_bytes: 1_048_576n,
        duration: 50,
        max_price_per_byte: 10n,
      }
    );
    console.log("  bucket_id        =", bucketId);
    console.log("  matched provider =", matchedProvider);
    console.log("  expires_at       =", expiresAt);
    if (!sameAddress(matchedProvider, provider.address)) {
      throw new Error(
        `create_bucket_with_storage matched ${matchedProvider}, expected ${provider.address}. ` +
          `The provider node at ${PROVIDER_URL} can only sign for ${PROVIDER_SEED}.`
      );
    }

    console.log("\n=== Step 3: Upload one chunk ===");
    const { data, commit } = await uploadChunk(
      PROVIDER_URL,
      bucketId,
      `Quickstart payload @ ${new Date().toISOString()}`
    );
    console.log("  uploaded %d bytes, mmr_root=%s", data.length, commit.mmr_root);

    console.log("\n=== Step 4: Submit checkpoint ===");
    const ck = await fetchCheckpointSignature(PROVIDER_URL, bucketId);
    await submitClientCheckpoint(api, client, provider, bucketId, ck);
    console.log("  Checkpoint submitted (leaf_count=%s)", ck.leaf_count);

    console.log("\n=== Step 5: Freeze bucket (now possible) ===");
    const frozen = await freezeBucket(api, client, bucketId);
    console.log("  BucketFrozen at start_seq=%s", frozen.frozen_start_seq);

    console.log("PASSED: atomic-setup + upload + freeze flow complete");
  } catch (err) {
    console.error("\nERROR:", err.message || err);
    if (err.stack) console.error(err.stack);
    process.exitCode = 1;
  } finally {
    if (restoreOthers) {
      try {
        await restoreOthers();
      } catch (err) {
        console.error("WARN: failed to restore other providers:", err.message || err);
      }
    }
    papi.destroy();
  }
}

main().then(() => console.log("\n=== Done ==="));
