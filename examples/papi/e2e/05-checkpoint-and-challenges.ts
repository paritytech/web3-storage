// SPDX-License-Identifier: Apache-2.0

/**
 * E2E Workflow 05 — Checkpoint and Challenges
 *
 * Accounts: //Alice (provider), //Bob (client)
 *
 * Tests: client checkpoints, off-chain/on-chain challenges + defense.
 *
 * Usage: node e2e/05-checkpoint-and-challenges.js [chain_ws] [provider_url]
 */

import assert from "node:assert";
import {
  challengeCheckpoint,
  challengeOffchain,
  ensureProviderRegistered,
  fetchChallengeProof,
  fetchCheckpointSignature,
  makeSigner,
  respondToChallenge,
  setBucketVisibility,
  submitClientCheckpoint,
  uploadChunk,
} from "@web3-storage/sdk";
import { ensureSoleAcceptingProvider } from "../support.js";
import { negotiateAndEstablish, runSuite, submitTxExpectFailure, setupChain } from "./helpers.js";

const CHAIN_WS = process.argv[2] || "ws://127.0.0.1:2222";
const PROVIDER_URL = process.argv[3] || "http://127.0.0.1:3333";

async function main() {
  const provider = makeSigner("//Alice");
  const client = makeSigner("//Bob");
  // Neither a member nor an agreement owner of the bucket below.
  const stranger = makeSigner("//Ferdie");

  const { papi, api } = await setupChain(CHAIN_WS);
  await ensureProviderRegistered(api, provider, PROVIDER_URL);
  const restore = await ensureSoleAcceptingProvider(api, provider);

  // Create a bucket + agreement + upload data for checkpoint tests.
  const maxBytes = 1_048_576n;
  const duration = 200;
  const { bucketId } = await negotiateAndEstablish(
    api,
    PROVIDER_URL,
    client,
    provider,
    { maxBytes, duration },
    true, // finalize: an immediate provider upload reads finalized membership
  );

  const payload = `checkpoint-test @ ${Date.now()}`;
  const upload = await uploadChunk(PROVIDER_URL, bucketId, payload, client);
  const uploadInfo = {
    leafIndex: upload.commit.leaf_indices[0],
    mmrRoot: upload.commit.mmr_root,
    startSeq: upload.commit.start_seq,
    leafCount: upload.commit.leaf_count,
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

  // ── Failure ───────────────────────────────────────────────────────────────

  tests.push({
    name: "5.4 Challenge a provider not in the snapshot",
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
        target: { leaf_index: 0n, chunk_index: 0n },
      });
      await submitTxExpectFailure(tx, client.signer, "ProviderNotInSnapshot", "5.4");
    },
  });

  tests.push({
    name: "5.5 Private bucket blocks stranger challenges on the primary",
    fn: async () => {
      // The bucket was created with the wrapper default (Private): a signed
      // account that is neither a member nor a primary-agreement owner may
      // not challenge the primary.
      const tx = api.tx.StorageProvider.challenge_checkpoint({
        bucket_id: bucketId,
        provider: provider.address,
        target: { leaf_index: BigInt(uploadInfo.leafIndex), chunk_index: 0n },
      });
      await submitTxExpectFailure(tx, stranger.signer, "NotAuthorizedForPrivateBucket", "5.5");
    },
  });

  tests.push({
    name: "5.6 Publicized bucket: stranger challenge defended at zero provider cost",
    fn: async () => {
      await setBucketVisibility(api, client, bucketId, "Public");
      const challengeId = await challengeCheckpoint(
        api,
        stranger,
        provider,
        bucketId,
        uploadInfo.leafIndex
      );
      const proof = await fetchChallengeProof(api, PROVIDER_URL, challengeId);
      const result = await respondToChallenge(api, provider, challengeId, proof);
      const events = api.event.StorageProvider.ChallengeDefended.filter(result.events as never);
      assert.strictEqual(events.length, 1, "Expected ChallengeDefended event");
      // Public tier: the stranger's deposit reimburses the provider in full;
      // the provider bears nothing and its stake is never touched.
      assert.strictEqual(
        events[0].payload.provider_cost,
        0n,
        "public defense must cost the provider 0"
      );
      assert.ok(events[0].payload.challenger_cost > 0n, "the stranger pays the full deposit");
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
