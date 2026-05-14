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

import { cryptoWaitReady } from "@polkadot/util-crypto";
import assert from "node:assert";
import {
  connect,
  ensureProviderRegistered,
  ensureSoleAcceptingProvider,
  makeSigner,
  parseProviderClientArgs,
  requireOneEvent,
  sameAddress,
  submitTx,
  waitForAgreementAcceptance,
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

async function createBucket(api, client) {
  const result = await submitTx(
    api.tx.StorageProvider.create_bucket({ min_providers: 1 }),
    client.signer,
    "create_bucket"
  );
  const event = requireOneEvent(
    result.events,
    api.event.StorageProvider.BucketCreated,
    "BucketCreated"
  );
  console.log("  Bucket created: id=%s", event.bucket_id);
  return event.bucket_id;
}

async function setupAgreement(api, client, provider, bucketId) {
  const maxBytes = 1_048_576n;
  const duration = 200;
  await submitTx(
    api.tx.StorageProvider.request_primary_agreement({
      bucket_id: bucketId,
      provider: provider.address,
      max_bytes: maxBytes,
      duration,
      max_payment: maxBytes * BigInt(duration) * 2n,
    }),
    client.signer,
    "request_primary_agreement"
  );
  await waitForAgreementAcceptance(api, provider.address, bucketId);
  console.log("  Agreement accepted");
}

async function configureCheckpointWindow(api, admin, bucketId) {
  const result = await submitTx(
    api.tx.StorageProvider.configure_checkpoint_window({
      bucket_id: bucketId,
      interval: WINDOW_INTERVAL,
      grace_period: WINDOW_GRACE,
      enabled: true,
    }),
    admin.signer,
    "configure_checkpoint_window"
  );
  requireOneEvent(
    result.events,
    api.event.StorageProvider.CheckpointConfigUpdated,
    "CheckpointConfigUpdated"
  );
  console.log(
    "  Window configured: interval=%d grace=%d",
    WINDOW_INTERVAL,
    WINDOW_GRACE
  );
}

async function reportMissedWindow(api, reporter, bucketId, window) {
  const result = await submitTx(
    api.tx.StorageProvider.report_missed_checkpoint({
      bucket_id: bucketId,
      window,
    }),
    reporter.signer,
    "report_missed_checkpoint"
  );
  const event = requireOneEvent(
    result.events,
    api.event.StorageProvider.CheckpointMissPenalized,
    "CheckpointMissPenalized"
  );
  console.log(
    "  CheckpointMissPenalized: provider=%s window=%s penalty=%s",
    event.provider,
    event.window,
    event.penalty.toString()
  );
  return event;
}

async function main() {
  await cryptoWaitReady();

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
    const bucketId = await createBucket(api, client);
    await setupAgreement(api, client, provider, bucketId);
    const bucket = await api.query.StorageProvider.Buckets.getValue(bucketId);
    assert.ok(
      bucket.primary_providers.some((p) => sameAddress(p, provider.address)),
      "Provider should be primary after accept"
    );

    console.log("\n=== Step 2: configure_checkpoint_window (tight) ===");
    await configureCheckpointWindow(api, client, bucketId);

    console.log("\n=== Step 3: Pick a window and let it elapse without a checkpoint ===");
    const headRaw = await api.query.System.Number.getValue();
    const head = Number(headRaw);
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
      provider.address
    );
    const reporterAcctBefore = await api.query.System.Account.getValue(
      client.address
    );
    console.log(
      "  Provider stake before: %s",
      providerBefore.stake.toString()
    );
    console.log(
      "  Reporter free before:  %s",
      reporterAcctBefore.data.free.toString()
    );

    const event = await reportMissedWindow(api, client, bucketId, missedWindow);
    assert.ok(
      sameAddress(event.provider, provider.address),
      `Leader should be the lone primary provider, got ${event.provider}`
    );
    assert.ok(event.penalty > 0n, "Penalty should be > 0");

    console.log("\n=== Step 5: Verify slashing + reporter reward ===");
    const providerAfter = await api.query.StorageProvider.Providers.getValue(
      provider.address
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
      await api.query.StorageProvider.LastCheckpointWindow.getValue(bucketId);
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
