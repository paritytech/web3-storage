// SPDX-License-Identifier: Apache-2.0

/**
 * Checks that the provider defends an off-chain challenge against a signed
 * commitment that a later commit superseded.
 * The test passes on ChallengeDefended and fails on ChallengeSlashed.
 *
 * The bug this test reproduces, step by step, with two chunks A and B:
 *
 * 1. Bob uploads chunk A and calls /commit.
 *    The provider's MMR contains one leaf: [A].
 *    Root R1 = hash(A).
 *    The provider signs (root R1, start_seq 0, leaf_count 1) and returns the signature to Bob.
 *    This signature is a promise: "I hold the data under root R1."
 *
 * 2. Bob uploads chunk B and calls /commit.
 *    The MMR now contains [A, B].
 *    Root R2 = hash(hash(A), hash(B)).
 *    The provider signs R2.
 *    The provider stores only R2 and the leaf list. R1 does not exist in its storage anymore.
 *
 * 3. Anyone who holds the first signature calls challenge_offchain with R1 and leaf 0:
 *    "Prove that leaf 0 is in R1."
 *    The chain checks the signature and accepts the challenge.
 *    The challenge stores R1 as the root to check against.
 *
 * 4. The provider builds a proof for leaf 0.
 *    get_mmr_proof receives only (bucket, leaf_index).
 *    It does not receive R1, so it builds the proof from the current leaves [A, B].
 *    The proof says: "leaf A plus sibling hash(B) gives R2."
 *
 * 5. The chain verifies the proof against R1, the root stored in the challenge.
 *    The proof hashes to R2, and R2 != R1, so verification fails.
 *
 * 6. The chain treats a proof that fails verification as a false response and slashes the whole stake at once (InvalidProof).
 *    The provider still holds chunk A. It cannot build a proof for the root it signed.
 *
 * Usage:
 * 1. just start-e2e-chain
 * 2. just start-provider
 * 3. node --import tsx e2e/challenge-stale-commitment.ts ws://127.0.0.1:2222 http://127.0.0.1:3333
 */

import assert from "node:assert";
import {
  challengeOffchain,
  ensureProviderRegistered,
  fetchChallengeProof,
  makeSigner,
  respondToChallenge,
  uploadChunk,
} from "@web3-storage/sdk";
import { ensureSoleAcceptingProvider } from "../support.js";
import { negotiateAndEstablish, runSuite, setupChain } from "./helpers.js";

const CHAIN_WS = process.argv[2] || "ws://127.0.0.1:2222";
const PROVIDER_URL = process.argv[3] || "http://127.0.0.1:3333";

async function main() {
  console.warn(
    "WARNING: a failing run slashes //Alice's entire provider stake on " +
      `${CHAIN_WS}. Run only against a throwaway chain.`,
  );

  const provider = makeSigner("//Alice");
  const client = makeSigner("//Bob");

  const { papi, api } = await setupChain(CHAIN_WS);
  await ensureProviderRegistered(api, provider, PROVIDER_URL);
  const restore = await ensureSoleAcceptingProvider(api, provider);

  const tests: Array<{ name: string; fn: () => Promise<void> }> = [];

  tests.push({
    name: "Off-chain challenge on a superseded commitment is defended",
    fn: async () => {
      const { bucketId } = await negotiateAndEstablish(
        api,
        PROVIDER_URL,
        client,
        provider,
        { maxBytes: 1_048_576n, duration: 200 },
        true, // finalize: an immediate provider upload reads finalized membership
      );

      const uploadA = await uploadChunk(
        PROVIDER_URL,
        bucketId,
        `stale-commitment-a @ ${Date.now()}`,
        client,
      );
      const firstCommitment = {
        leafIndex: uploadA.commit.leaf_indices[0],
        mmrRoot: uploadA.commit.mmr_root,
        startSeq: uploadA.commit.start_seq,
        leafCount: uploadA.commit.leaf_count,
        providerSignature: uploadA.commit.provider_signature,
      };

      // A second commit changes the provider's current MMR root and peaks,
      // but does not invalidate the first commitment.
      await uploadChunk(PROVIDER_URL, bucketId, `stale-commitment-b @ ${Date.now()}`, client);

      const challengeId = await challengeOffchain(api, client, provider, bucketId, firstCommitment);
      assert.ok(challengeId.deadline, "Challenge should have a deadline");

      const proof = await fetchChallengeProof(api, PROVIDER_URL, challengeId);
      const result = await respondToChallenge(api, provider, challengeId, proof);

      const slashed = api.event.StorageProvider.ChallengeSlashed.filter(result.events as never);
      assert.strictEqual(
        slashed.length,
        0,
        `Provider slashed: reason=${JSON.stringify(slashed[0]?.payload.reason)} ` +
          `slashed_amount=${slashed[0]?.payload.slashed_amount}`,
      );
      const defended = api.event.StorageProvider.ChallengeDefended.filter(result.events as never);
      assert.strictEqual(defended.length, 1, "Expected ChallengeDefended event");
    },
  });

  await runSuite("Challenge on a stale commitment", tests, { api, papi });

  try {
    await restore();
  } catch {}
  papi.destroy();
}

main()
  .catch((err) => {
    console.error(err);
    process.exitCode = 1;
  })
  .finally(() => {
    process.exit(process.exitCode || 0);
  });
