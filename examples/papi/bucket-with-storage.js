/**
 * Bucket-and-agreement quickstart for pallet-storage-provider.
 *
 * Walks the minimum atomic flow: negotiate provider-signed terms over
 * HTTP, redeem them via establish_storage_agreement (which opens the
 * bucket + primary agreement in one tx), upload a single chunk, submit
 * a checkpoint, then freeze the bucket — which only becomes possible
 * once a snapshot exists.
 *
 * Prerequisites:
 *   - Parachain at ws://127.0.0.1:2222
 *   - Provider node running and registered as //Alice with accepting_primary=true
 *     (this script will register/configure //Alice if needed)
 *
 * Usage: node bucket-with-storage.js [chain_ws] [provider_url] [provider_seed] [client_seed]
 */

import {
  establishStorageAgreement,
  fetchCheckpointSignature,
  freezeBucket,
  negotiateTerms,
  submitClientCheckpoint,
  uploadChunk,
} from "./api.js";
import {
  connect,
  ensureProviderRegistered,
  ensureSoleAcceptingProvider,
  makeSigner,
  parseProviderClientArgs,
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

    console.log("\n=== Step 2: Negotiate signed agreement terms ===");
    const signed = await negotiateTerms(PROVIDER_URL, {
      owner: client.address,
      max_bytes: 1_048_576, // 1 MiB
      duration: 50,
      price_per_byte: 0,
      replica_params: null,
    });
    console.log(
      "  Provider signed terms: nonce=%s, valid_until=%s",
      signed.terms.nonce,
      signed.terms.valid_until
    );

    console.log("\n=== Step 3: establish_storage_agreement (atomic) ===");
    const bucketId = await establishStorageAgreement(api, client, provider, signed);
    console.log("  bucket_id =", bucketId);

    console.log("\n=== Step 4: Upload one chunk ===");
    const { data, commit } = await uploadChunk(
      PROVIDER_URL,
      bucketId,
      `Quickstart payload @ ${new Date().toISOString()}`
    );
    console.log("  uploaded %d bytes, mmr_root=%s", data.length, commit.mmr_root);

    console.log("\n=== Step 5: Submit checkpoint ===");
    const ck = await fetchCheckpointSignature(PROVIDER_URL, bucketId);
    await submitClientCheckpoint(api, client, provider, bucketId, ck);
    console.log("  Checkpoint submitted (leaf_count=%s)", ck.leaf_count);

    console.log("\n=== Step 6: Freeze bucket (now possible) ===");
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
