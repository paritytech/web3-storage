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

import { Binary, Enum } from "@polkadot-api/substrate-bindings";
import { blake2AsU8a, cryptoWaitReady } from "@polkadot/util-crypto";
import assert from "node:assert";
import {
  connect,
  ensureProviderRegistered,
  ensureSoleAcceptingProvider,
  hexToBytes,
  makeSigner,
  parseProviderClientArgs,
  providerFetch,
  requireOneEvent,
  sameAddress,
  submitTx,
  toHex,
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
  console.log("  Agreement accepted (auto by provider node)");
}

async function uploadOneChunk(bucketId) {
  const data = new TextEncoder().encode(
    `checkpoint-rewards @ ${new Date().toISOString()}`
  );
  const hash = toHex(blake2AsU8a(data));
  await providerFetch(PROVIDER_URL, "/node", {
    method: "PUT",
    body: {
      bucket_id: Number(bucketId),
      hash,
      data: Buffer.from(data).toString("base64"),
      children: null,
    },
  });
  const commit = await providerFetch(PROVIDER_URL, "/commit", {
    method: "POST",
    body: { bucket_id: Number(bucketId), data_roots: [hash] },
  });
  console.log("  Uploaded %d bytes, mmr_root=%s", data.length, commit.mmr_root);
  return commit;
}

async function configureCheckpointWindow(api, client, bucketId) {
  const result = await submitTx(
    api.tx.StorageProvider.configure_checkpoint_window({
      bucket_id: bucketId,
      interval: WINDOW_INTERVAL,
      grace_period: WINDOW_GRACE,
      enabled: true,
    }),
    client.signer,
    "configure_checkpoint_window"
  );
  const event = requireOneEvent(
    result.events,
    api.event.StorageProvider.CheckpointConfigUpdated,
    "CheckpointConfigUpdated"
  );
  console.log(
    "  Config saved: interval=%s grace=%s enabled=%s",
    event.interval,
    event.grace_period,
    event.enabled
  );
}

async function fundCheckpointPool(api, funder, bucketId, amount) {
  const result = await submitTx(
    api.tx.StorageProvider.fund_checkpoint_pool({
      bucket_id: bucketId,
      amount,
    }),
    funder.signer,
    "fund_checkpoint_pool"
  );
  const event = requireOneEvent(
    result.events,
    api.event.StorageProvider.CheckpointPoolFunded,
    "CheckpointPoolFunded"
  );
  console.log(
    "  Pool funded by %s with %s units",
    event.funder,
    event.amount.toString()
  );

  // Verify pool balance increased.
  const balance = await api.query.StorageProvider.CheckpointPool.getValue(
    bucketId
  );
  console.log("  CheckpointPool[%s] = %s", bucketId, balance.toString());
  assert.ok(
    balance >= amount,
    `Pool balance ${balance} < funded amount ${amount}`
  );
}

async function submitProviderCheckpoint(api, papi, provider, bucketId) {
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
  const duty = await providerFetch(PROVIDER_URL, "/checkpoint/duty", {
    params: { bucket_id: Number(bucketId) },
  });
  assert.ok(duty.ready, `Provider not ready to checkpoint: ${JSON.stringify(duty)}`);

  const signed = await providerFetch(PROVIDER_URL, "/checkpoint/sign", {
    method: "POST",
    body: {
      bucket_id: Number(bucketId),
      mmr_root: duty.mmr_root,
      start_seq: duty.start_seq,
      leaf_count: duty.leaf_count,
      window: Number(window),
    },
  });
  assert.ok(signed.agreed, `Provider refused to sign: ${JSON.stringify(signed)}`);

  const result = await submitTx(
    api.tx.StorageProvider.provider_checkpoint({
      bucket_id: bucketId,
      mmr_root: Binary.fromBytes(hexToBytes(duty.mmr_root)),
      start_seq: BigInt(duty.start_seq),
      leaf_count: BigInt(duty.leaf_count),
      window,
      signatures: [
        [
          provider.address,
          Enum("Sr25519", Binary.fromBytes(hexToBytes(signed.signature))),
        ],
      ],
    }),
    provider.signer,
    "provider_checkpoint"
  );

  const event = requireOneEvent(
    result.events,
    api.event.StorageProvider.ProviderCheckpointSubmitted,
    "ProviderCheckpointSubmitted"
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

async function claimRewards(api, provider, bucketId, expectedReward) {
  // CheckpointRewards is a double map (bucket_id, account) → balance.
  const pending = await api.query.StorageProvider.CheckpointRewards.getValue(
    bucketId,
    provider.address
  );
  console.log("  Pending rewards before claim: %s", pending.toString());
  assert.strictEqual(
    pending,
    expectedReward,
    `Pending ${pending} != event reward ${expectedReward}`
  );

  const result = await submitTx(
    api.tx.StorageProvider.claim_checkpoint_rewards({ bucket_id: bucketId }),
    provider.signer,
    "claim_checkpoint_rewards"
  );
  const event = requireOneEvent(
    result.events,
    api.event.StorageProvider.CheckpointRewardClaimed,
    "CheckpointRewardClaimed"
  );
  console.log(
    "  Claimed %s units for provider %s",
    event.amount.toString(),
    event.provider
  );
  assert.strictEqual(event.amount, expectedReward);

  const after = await api.query.StorageProvider.CheckpointRewards.getValue(
    bucketId,
    provider.address
  );
  assert.strictEqual(
    after,
    0n,
    `CheckpointRewards should be cleared after claim, got ${after}`
  );
}

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
    console.log("\n=== Step 1: Ensure provider registered & sole acceptor ===");
    await ensureProviderRegistered(api, provider, PROVIDER_URL);
    restoreOthers = await ensureSoleAcceptingProvider(api, provider);

    console.log("\n=== Step 2: Create bucket (client = admin) ===");
    const bucketId = await createBucket(api, client);

    console.log("\n=== Step 3: Open agreement so the provider becomes primary ===");
    await setupAgreement(api, client, provider, bucketId);
    const bucket = await api.query.StorageProvider.Buckets.getValue(bucketId);
    assert.ok(
      bucket.primary_providers.some((p) => sameAddress(p, provider.address)),
      "Provider should be in primary_providers after agreement"
    );

    console.log("\n=== Step 4: Upload data so the MMR has something to commit ===");
    await uploadOneChunk(bucketId);

    console.log("\n=== Step 5: configure_checkpoint_window ===");
    await configureCheckpointWindow(api, client, bucketId);

    console.log("\n=== Step 6: fund_checkpoint_pool ===");
    await fundCheckpointPool(api, client, bucketId, POOL_AMOUNT);

    console.log("\n=== Step 7: provider_checkpoint (autonomous) ===");
    const reward = await submitProviderCheckpoint(api, papi, provider, bucketId);

    console.log("\n=== Step 8: claim_checkpoint_rewards ===");
    await claimRewards(api, provider, bucketId, reward);

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
