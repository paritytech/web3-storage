/**
 * Provider-initiated checkpoint + reward flow for pallet-storage-provider.
 *
 * Demonstrates the four checkpoint-reward extrinsics in one script:
 *   - configure_checkpoint_window  (bucket admin tunes interval / grace)
 *   - fund_checkpoint_pool         (anyone tops up the reward pool)
 *   - provider_checkpoint          (the leader submits and earns from the pool)
 *   - claim_checkpoint_rewards     (provider sweeps accumulated rewards)
 *
 * `checkpoint` (client-initiated) is exercised by full-flow.js /
 * bucket-with-storage.js — this script focuses on the autonomous variant
 * where providers coordinate checkpoints without a client signing each one.
 *
 * Prerequisites:
 *   - Parachain running at ws://127.0.0.1:2222
 *   - Provider node running at the specified URL
 *   - Descriptors generated: npm run papi:generate
 *
 * Usage: node checkpoint-rewards.js [chain_ws] [provider_url] [provider_seed] [client_seed]
 */

import assert from "node:assert";
import {
  claimCheckpointRewards,
  configureCheckpointWindow,
  createBucket,
  fetchCheckpointDuty,
  fundCheckpointPool,
  requestPrimaryAgreement,
  signCheckpointProposal,
  submitProviderCheckpoint,
  uploadChunk,
} from "./api.js";
import {
  connect,
  ensureProviderRegistered,
  ensureSoleAcceptingProvider,
  makeSigner,
  parseProviderClientArgs,
  sameAddress,
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

// Checkpoint window must be large enough that the window number we compute
// client-side is still the current window by the time the extrinsic executes.
// 50 blocks ≈ 5 minutes at 6s blocks — plenty of headroom.
const WINDOW_INTERVAL = 50;
const WINDOW_GRACE = 20;

// Anyone can top up the pool; we use 5 tokens so the demo can cover at least
// one provider_checkpoint reward (CheckpointReward = 1 token by default).
const POOL_AMOUNT = 5_000_000_000_000n;

async function runProviderCheckpoint(api, papi, provider, bucketId) {
  // The window must be the *current* one at execution time. Read the chain's
  // head, compute window = head / interval, then sign + submit. The runtime
  // recomputes the window from its own block number at inclusion time, so if
  // we're near the boundary the window can roll over before our tx lands and
  // the call fails with InvalidCheckpointWindow. Require enough headroom
  // (a few blocks for inclusion under load) before submitting.
  const HEADROOM_BLOCKS = 15;
  let currentBlock = Number(await api.query.System.Number.getValue());
  let windowNum = Math.floor(currentBlock / WINDOW_INTERVAL);
  let nextWindowStart = (windowNum + 1) * WINDOW_INTERVAL;
  if (nextWindowStart - currentBlock < HEADROOM_BLOCKS) {
    console.log(
      "  Only %d blocks left in window %d; waiting for window %d to start...",
      nextWindowStart - currentBlock,
      windowNum,
      windowNum + 1
    );
    await waitForBlock(papi, nextWindowStart - 1);
    currentBlock = Number(await api.query.System.Number.getValue());
    windowNum = Math.floor(currentBlock / WINDOW_INTERVAL);
  }
  const window = BigInt(windowNum);
  console.log(
    "  current_block=%d  interval=%d  window=%s",
    currentBlock,
    WINDOW_INTERVAL,
    window
  );

  // Pull our local commitment from the provider node, then ask the provider
  // to sign a CheckpointProposal (different payload from /checkpoint-signature,
  // which signs CommitmentPayload — that one is for client-initiated checkpoint).
  const duty = await fetchCheckpointDuty(PROVIDER_URL, bucketId);
  assert.ok(duty.ready, `Provider not ready to checkpoint: ${JSON.stringify(duty)}`);

  const signed = await signCheckpointProposal(PROVIDER_URL, bucketId, duty, window);
  assert.ok(signed.agreed, `Provider refused to sign: ${JSON.stringify(signed)}`);

  const event = await submitProviderCheckpoint(
    api,
    provider,
    bucketId,
    duty,
    signed.signature,
    window
  );
  console.log(
    "  Checkpoint accepted: window=%s leader=%s reward=%s",
    event.window,
    event.leader,
    event.reward.toString()
  );
  assert.ok(
    event.reward > 0n,
    `Expected reward > 0 (pool was funded), got ${event.reward}`
  );
  return event.reward;
}

async function claimAndVerify(api, provider, bucketId, expectedReward) {
  // CheckpointRewards is a double map (account, bucket_id) → balance.
  // Provider-first key order enables `iter_prefix(&provider)` so deregistration
  // can drain a provider's pending rewards without scanning every bucket.
  const pending = await api.query.StorageProvider.CheckpointRewards.getValue(
    provider.address,
    bucketId
  );
  console.log("  Pending rewards before claim: %s", pending.toString());
  assert.strictEqual(
    pending,
    expectedReward,
    `Pending ${pending} != event reward ${expectedReward}`
  );

  const event = await claimCheckpointRewards(api, provider, bucketId);
  console.log(
    "  Claimed %s units for provider %s",
    event.amount.toString(),
    event.provider
  );
  assert.strictEqual(event.amount, expectedReward);

  const after = await api.query.StorageProvider.CheckpointRewards.getValue(
    provider.address,
    bucketId
  );
  assert.strictEqual(
    after,
    0n,
    `CheckpointRewards should be cleared after claim, got ${after}`
  );
}

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
    console.log("\n=== Step 1: Ensure provider registered & sole acceptor ===");
    await ensureProviderRegistered(api, provider, PROVIDER_URL);
    restoreOthers = await ensureSoleAcceptingProvider(api, provider);

    console.log("\n=== Step 2: Create bucket (client = admin) ===");
    const bucketId = await createBucket(api, client);
    console.log("  Bucket created: id=%s", bucketId);

    console.log("\n=== Step 3: Open agreement so the provider becomes primary ===");
    const maxBytes = 1_048_576n;
    const duration = 200;
    await requestPrimaryAgreement(api, client, provider, bucketId, {
      max_bytes: maxBytes,
      duration,
      max_payment: maxBytes * BigInt(duration) * 2n,
    });
    await waitForAgreementAcceptance(api, provider.address, bucketId);
    console.log("  Agreement accepted (auto by provider node)");

    const bucket = await api.query.StorageProvider.Buckets.getValue(bucketId);
    assert.ok(
      bucket.primary_providers.some((p) => sameAddress(p, provider.address)),
      "Provider should be in primary_providers after agreement"
    );

    console.log("\n=== Step 4: Upload data so the MMR has something to commit ===");
    const uploadNonce = Number(await api.query.System.Number.getValue());
    const { data, commit } = await uploadChunk(
      PROVIDER_URL,
      bucketId,
      `checkpoint-rewards @ ${new Date().toISOString()}`,
      uploadNonce
    );
    console.log("  Uploaded %d bytes, mmr_root=%s", data.length, commit.mmr_root);

    console.log("\n=== Step 5: configure_checkpoint_window ===");
    const cfgEvent = await configureCheckpointWindow(api, client, bucketId, {
      interval: WINDOW_INTERVAL,
      gracePeriod: WINDOW_GRACE,
    });
    console.log(
      "  Config saved: interval=%s grace=%s enabled=%s",
      cfgEvent.interval,
      cfgEvent.grace_period,
      cfgEvent.enabled
    );

    console.log("\n=== Step 6: fund_checkpoint_pool ===");
    const fundEvent = await fundCheckpointPool(api, client, bucketId, POOL_AMOUNT);
    console.log(
      "  Pool funded by %s with %s units",
      fundEvent.funder,
      fundEvent.amount.toString()
    );
    const balance = await api.query.StorageProvider.CheckpointPool.getValue(
      bucketId
    );
    console.log("  CheckpointPool[%s] = %s", bucketId, balance.toString());
    assert.ok(
      balance >= POOL_AMOUNT,
      `Pool balance ${balance} < funded amount ${POOL_AMOUNT}`
    );

    console.log("\n=== Step 7: provider_checkpoint (autonomous) ===");
    const reward = await runProviderCheckpoint(api, papi, provider, bucketId);

    console.log("\n=== Step 8: claim_checkpoint_rewards ===");
    await claimAndVerify(api, provider, bucketId, reward);

    console.log("\nPASSED: provider-initiated checkpoint reward cycle complete");
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
