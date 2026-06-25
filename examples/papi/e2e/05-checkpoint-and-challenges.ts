// SPDX-License-Identifier: Apache-2.0

/**
 * E2E Workflow 05 — Checkpoint and Challenges
 *
 * Accounts: //Alice (provider), //Bob (client)
 *
 * Tests: client/provider checkpoints, off-chain/on-chain challenges + defense.
 *
 * Usage: node e2e/05-checkpoint-and-challenges.js [chain_ws] [provider_url]
 */

import assert from "node:assert";
import {
  challengeCheckpoint,
  challengeOffchain,
  claimCheckpointRewards,
  configureCheckpointWindow,
  ensureProviderRegistered,
  fetchChallengeProof,
  fetchCheckpointDuty,
  fetchCheckpointSignature,
  fundCheckpointPool,
  makeSigner,
  READ_OPTS,
  respondToChallenge,
  signCheckpointProposal,
  submitClientCheckpoint,
  submitProviderCheckpoint,
  uploadChunk,
  waitForBlock,
} from "@web3-storage/sdk";
import { ensureSoleAcceptingProvider } from "../support.js";
import { negotiateAndEstablish, runSuite, submitTxExpectFailure, setupChain } from "./helpers.js";

const CHAIN_WS = process.argv[2] || "ws://127.0.0.1:2222";
const PROVIDER_URL = process.argv[3] || "http://127.0.0.1:3333";

const WINDOW_INTERVAL = 20;
const WINDOW_GRACE = 10;
const POOL_AMOUNT = 5_000_000_000_000n;

async function main() {
  const provider = makeSigner("//Alice");
  const client = makeSigner("//Bob");

  const { papi, api } = await setupChain(CHAIN_WS);
  await ensureProviderRegistered(api, provider, PROVIDER_URL);
  const restore = await ensureSoleAcceptingProvider(api, provider);

  // Create a bucket + agreement + upload data for checkpoint tests.
  const maxBytes = 1_048_576n;
  const duration = 200;
  const { bucketId } = await negotiateAndEstablish(api, PROVIDER_URL, client, provider, {
    maxBytes,
    duration,
  });

  const payload = `checkpoint-test @ ${Date.now()}`;
  const upload = await uploadChunk(PROVIDER_URL, bucketId, payload);
  const uploadInfo = {
    leafIndex: upload.commit.leaf_indices[0],
    mmrRoot: upload.commit.mmr_root,
    startSeq: upload.commit.start_seq,
    providerSignature: upload.commit.provider_signature,
  };

  const tests: Array<{ name: string; fn: () => Promise<void> }> = [];

  // ── Success ───────────────────────────────────────────────────────────────

  tests.push({
    name: "5.1 Client checkpoint",
    fn: async () => {
      const ck = await fetchCheckpointSignature(PROVIDER_URL, bucketId);
      assert.ok(ck.mmr_root, "Checkpoint should have mmr_root");
      const result = await submitClientCheckpoint(api, client, provider, bucketId, ck);
      const events = api.event.StorageProvider.BucketCheckpointed.filter(result.events as never);
      assert.strictEqual(events.length, 1, "Expected BucketCheckpointed event");
    },
  });

  tests.push({
    name: "5.2 Off-chain challenge + defense",
    fn: async () => {
      const challengeId = await challengeOffchain(
        api,
        client,
        provider,
        bucketId,
        uploadInfo
      );
      assert.ok(challengeId.deadline, "Challenge should have a deadline");
      const proof = await fetchChallengeProof(api, PROVIDER_URL, challengeId);
      const result = await respondToChallenge(api, provider, challengeId, proof);
      const events = api.event.StorageProvider.ChallengeDefended.filter(result.events as never);
      assert.strictEqual(events.length, 1, "Expected ChallengeDefended event");
    },
  });

  tests.push({
    name: "5.3 On-chain challenge + defense",
    fn: async () => {
      const challengeId = await challengeCheckpoint(
        api,
        client,
        provider,
        bucketId,
        uploadInfo.leafIndex
      );
      assert.ok(challengeId.deadline, "Challenge should have a deadline");
      const proof = await fetchChallengeProof(api, PROVIDER_URL, challengeId);
      const result = await respondToChallenge(api, provider, challengeId, proof);
      const events = api.event.StorageProvider.ChallengeDefended.filter(result.events as never);
      assert.strictEqual(events.length, 1, "Expected ChallengeDefended event");
    },
  });

  tests.push({
    name: "5.4 Provider-initiated checkpoint + reward",
    fn: async () => {
      await configureCheckpointWindow(api, client, bucketId, {
        interval: WINDOW_INTERVAL,
        gracePeriod: WINDOW_GRACE,
      });
      await fundCheckpointPool(api, client, bucketId, POOL_AMOUNT);

      // Compute current window with headroom.
      const HEADROOM = 8;
      let currentBlock = Number(await api.query.System.Number.getValue(READ_OPTS));
      let windowNum = Math.floor(currentBlock / WINDOW_INTERVAL);
      let nextWindowStart = (windowNum + 1) * WINDOW_INTERVAL;
      if (nextWindowStart - currentBlock < HEADROOM) {
        await waitForBlock(papi, nextWindowStart - 1);
        currentBlock = Number(await api.query.System.Number.getValue(READ_OPTS));
        windowNum = Math.floor(currentBlock / WINDOW_INTERVAL);
      }
      const window = BigInt(windowNum);

      const duty = await fetchCheckpointDuty(PROVIDER_URL, bucketId);
      assert.ok(duty.ready, "Provider should be ready to checkpoint");
      const signed = await signCheckpointProposal(PROVIDER_URL, bucketId, duty, window);
      assert.ok(signed.agreed, "Provider should agree to sign");

      const event = await submitProviderCheckpoint(
        api,
        provider,
        bucketId,
        duty,
        signed.signature,
        Number(window)
      );
      assert.ok(event.reward > 0n, "Reward should be positive");
    },
  });

  tests.push({
    name: "5.5 Claim checkpoint rewards",
    fn: async () => {
      const pending = await api.query.StorageProvider.CheckpointRewards.getValue(
        provider.address,
        bucketId,
        READ_OPTS
      );
      assert.ok(pending > 0n, "Should have pending rewards");
      const event = await claimCheckpointRewards(api, provider, bucketId);
      assert.ok(event.amount > 0n, "Claimed amount should be positive");
      const after = await api.query.StorageProvider.CheckpointRewards.getValue(
        provider.address,
        bucketId,
        READ_OPTS
      );
      assert.strictEqual(after, 0n, "Rewards should be cleared after claim");
    },
  });

  // ── Failure ───────────────────────────────────────────────────────────────

  tests.push({
    name: "5.6 Challenge a provider not in the snapshot",
    fn: async () => {
      // challenge_checkpoint validates the *provider* against the snapshot at
      // creation, but NOT the leaf_index: a beyond-canonical leaf is rejected
      // later, when the provider answers with Superseded (LeafBeyondCanonical
      // — see the respond_to_challenge_superseded_fails_leaf_beyond_canonical
      // unit test). So the creation-time guard to exercise here is the provider
      // check — Bob is not a primary provider of this bucket.
      const tx = api.tx.StorageProvider.challenge_checkpoint({
        bucket_id: bucketId,
        provider: client.address, // not in the snapshot's primary_providers
        leaf_index: 0n,
        chunk_index: 0n,
      });
      await submitTxExpectFailure(tx, client.signer, "ProviderNotInSnapshot", "5.6");
    },
  });

  tests.push({
    name: "5.7 No rewards to claim",
    fn: async () => {
      // We already claimed rewards in 5.5, so claiming again should fail.
      const tx = api.tx.StorageProvider.claim_checkpoint_rewards({ bucket_id: bucketId });
      await submitTxExpectFailure(tx, provider.signer, "NoRewardsToClaim", "5.7");
    },
  });

  await runSuite("05 — Checkpoint and Challenges", tests, { api, papi });

  try {
    await restore();
  } catch {}
  papi.destroy();
}

main().catch((err) => {
  console.error(err);
  process.exitCode = 1;
}).finally(() => {
  process.exit(process.exitCode || 0);
});
