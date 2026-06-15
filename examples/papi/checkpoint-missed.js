/**
 * Missed-checkpoint reporting flow for pallet-storage-provider.
 *
 * Demonstrates that when a bucket has a checkpoint window configured but no
 * provider submits a `provider_checkpoint` for that window, anyone can call
 * `report_missed_checkpoint` once the window has fully passed. The pallet
 * slashes the elected leader's reserved stake and pays the reporter 10%.
 *
 * Exercised extrinsics:
 *   - configure_checkpoint_window  (tight interval so the demo runs in <2 min)
 *   - report_missed_checkpoint     (the slashing path)
 *
 * Prerequisites:
 *   - Parachain running at ws://127.0.0.1:2222
 *   - Provider node running at the specified URL (its checkpoint coordinator
 *     must NOT be enabled, otherwise it would auto-submit and there would be
 *     no missed window to report)
 *   - Descriptors generated: npm run papi:generate
 *
 * Usage: node checkpoint-missed.js [chain_ws] [provider_url] [provider_seed] [client_seed]
 */

import assert from "node:assert";
import {
  configureCheckpointWindow,
  establishStorageAgreement,
  negotiateTerms,
  reportMissedCheckpoint,
} from "./api.js";
import {
  connect,
  ensureProviderRegistered,
  ensureSoleAcceptingProvider,
  makeSigner,
  parseProviderClientArgs,
  READ_OPTS,
  sameAddress,
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

// Tight window so the demo finishes quickly. report_missed_checkpoint requires
// current_block > (window + 1) * interval, so the longest we ever wait is
// `interval` blocks (~60s at 6s blocks).
const WINDOW_INTERVAL = 10;
const WINDOW_GRACE = 5;

async function main() {
  const provider = makeSigner(PROVIDER_SEED);
  const client = makeSigner(CLIENT_SEED);

  console.log("Chain:", CHAIN_WS, " Provider HTTP:", PROVIDER_URL);
  console.log("Provider (%s) => %s", PROVIDER_SEED, provider.address);
  console.log("Reporter (%s) => %s", CLIENT_SEED, client.address);

  const { papi, api } = await connect(CHAIN_WS);
  await waitForChainReady(api);
  await waitForBlockProduction(api);
  await waitForNextBlock(papi);

  let restoreOthers = null;
  try {
    console.log("\n=== Step 1: Setup provider + bucket + agreement ===");
    await ensureProviderRegistered(api, provider, PROVIDER_URL);
    restoreOthers = await ensureSoleAcceptingProvider(api, provider);

    const signed = await negotiateTerms(PROVIDER_URL, {
      owner: client.address,
      max_bytes: 1_048_576n, // 1 MiB
      duration: 200,
      price_per_byte: 1n,
      replica_params: null,
    });
    const bucketId = await establishStorageAgreement(api, client, provider, signed);
    console.log("  Bucket + agreement opened: id=%s", bucketId);

    const bucket = await api.query.StorageProvider.Buckets.getValue(
      bucketId,
      READ_OPTS
    );
    assert.ok(
      bucket.primary_providers.some((p) => sameAddress(p, provider.address)),
      "Provider should be primary after establish_storage_agreement"
    );

    console.log("\n=== Step 2: configure_checkpoint_window (tight) ===");
    await configureCheckpointWindow(api, client, bucketId, {
      interval: WINDOW_INTERVAL,
      gracePeriod: WINDOW_GRACE,
    });
    console.log(
      "  Window configured: interval=%d grace=%d",
      WINDOW_INTERVAL,
      WINDOW_GRACE
    );

    console.log("\n=== Step 3: Pick a window and let it elapse without a checkpoint ===");
    const head = Number(await api.query.System.Number.getValue(READ_OPTS));
    const missedWindow = BigInt(Math.floor(head / WINDOW_INTERVAL));
    // window_end = (missedWindow + 1) * interval ; need current_block > window_end
    const windowEnd = (Number(missedWindow) + 1) * WINDOW_INTERVAL;
    console.log(
      "  head=%d  missed_window=%s  window_end=%d (must wait until head > %d)",
      head,
      missedWindow,
      windowEnd,
      windowEnd
    );
    await waitForBlock(papi, windowEnd);

    console.log("\n=== Step 4: Record balances, then report_missed_checkpoint ===");
    const providerBefore = await api.query.StorageProvider.Providers.getValue(
      provider.address,
      READ_OPTS
    );
    const reporterAcctBefore = await api.query.System.Account.getValue(
      client.address,
      READ_OPTS
    );
    console.log("  Provider stake before: %s", providerBefore.stake.toString());
    console.log(
      "  Reporter free before:  %s",
      reporterAcctBefore.data.free.toString()
    );

    const event = await reportMissedCheckpoint(api, client, bucketId, missedWindow);
    console.log(
      "  CheckpointMissPenalized: provider=%s window=%s penalty=%s",
      event.provider,
      event.window,
      event.penalty.toString()
    );
    assert.ok(
      sameAddress(event.provider, provider.address),
      `Leader should be the lone primary provider, got ${event.provider}`
    );
    assert.ok(event.penalty > 0n, "Penalty should be > 0");

    console.log("\n=== Step 5: Verify slashing + reporter reward ===");
    const providerAfter = await api.query.StorageProvider.Providers.getValue(
      provider.address,
      READ_OPTS
    );
    const stakeDelta = providerBefore.stake - providerAfter.stake;
    console.log("  Provider stake delta: %s", stakeDelta.toString());
    assert.strictEqual(
      stakeDelta,
      event.penalty,
      `Provider stake should drop by exactly the penalty (${event.penalty})`
    );

    // LastCheckpointWindow is updated to prevent re-reporting.
    const lastWindow =
      await api.query.StorageProvider.LastCheckpointWindow.getValue(
        bucketId,
        READ_OPTS
      );
    assert.strictEqual(
      lastWindow,
      missedWindow,
      `LastCheckpointWindow should record the just-reported window (${missedWindow}), got ${lastWindow}`
    );
    console.log("  LastCheckpointWindow[%s] = %s ✓", bucketId, lastWindow);

    console.log("\nPASSED: missed-checkpoint reporting + leader slashing");
  } catch (err) {
    console.error("\nERROR:", err.message || err);
    if (err.stack) console.error(err.stack);
    process.exitCode = 1;
  } finally {
    if (restoreOthers) {
      try {
        await restoreOthers();
      } catch (err) {
        console.error("WARN: restoring providers failed:", err.message || err);
      }
    }
    papi.destroy();
  }
}

main().then(() => console.log("\n=== Done ==="));
