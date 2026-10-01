// SPDX-License-Identifier: Apache-2.0

/**
 * E2E Workflow 14 - The provider can answer every challenge the pallet accepts
 *
 * Accounts: //Alice (provider), //Bob (bucket admin)
 *
 * The provider commits the valid zero-padded tree over 3 chunks, which has 4
 * slots. Slot 3 is padding: no chunk exists for it.
 * 
 * `challenge_offchain` does not bound `chunk_index` against the leaf count,
 * so it accepts a challenge on slot 3.
 * 
 * The provider cannot build a proof for that slot, so the challenge
 * stays open until the timeout sweep slashes the provider with
 * `SlashReason::Timeout`. The runtime tests cover the timeout slash; this
 * workflow stops at the provider's response.
 *
 * When 14.2 fails, its challenge stays open. It blocks `complete_deregister`
 * for //Alice, and on a chain that runs past `ChallengeTimeout` the sweep
 * slashes //Alice's stake.
 *
 * 14.1 The provider answers a challenge on chunk 2 of a 3-chunk tree.
 * 14.2 A challenge on chunk 3 (the padding slot) is either rejected by
 *      `challenge_offchain` or answered by the provider.
 *
 * Usage: node --import tsx e2e/14-padding-slot-challenge.ts [chain_ws] [provider_url]
 */

import assert from "node:assert";
import {
  bytesEq,
  challengeOffchain,
  computeCid,
  ensureProviderRegistered,
  fetchChallengeProof,
  makeSigner,
  paddedMerkleRoot,
  providerFetch,
  putChunk,
  READ_OPTS,
  respondToChallenge,
  toHex,
} from "@web3-storage/sdk";
import { ensureSoleAcceptingProvider } from "../support.js";
import { negotiateAndEstablish, putInternal, runSuite, setupChain } from "./helpers.js";

const CHAIN_WS = process.argv[2] || "ws://127.0.0.1:2222";
const PROVIDER_URL = process.argv[3] || "http://127.0.0.1:3333";

/** The last real chunk of a 3-chunk tree. */
const LAST_CHUNK = 2n;

/** The padding slot of a 3-chunk tree. */
const PADDING_CHUNK = 3n;

async function main() {
  const provider = makeSigner("//Alice");
  const owner = makeSigner("//Bob");

  const { papi, api } = await setupChain(CHAIN_WS);
  await ensureProviderRegistered(api, provider, PROVIDER_URL);
  const restoreProviders = await ensureSoleAcceptingProvider(api, provider);

  try {
    const { bucketId } = await negotiateAndEstablish(
      api,
      PROVIDER_URL,
      owner,
      provider,
      { maxBytes: 1_048_576n, duration: 200 },
      true, // finalize: the provider reads bucket membership from finalized state
    );

    // H(H(c0, c1), H(c2, 0)): the padded tree `/commit` accepts.
    const chunks = [0, 1, 2].map((i) => new Uint8Array(32).fill(i + 1));
    const leaves = chunks.map((c) => computeCid(c));
    for (const chunk of chunks) {
      await putChunk(PROVIDER_URL, bucketId, chunk, owner);
    }
    const left = await putInternal(PROVIDER_URL, bucketId, owner, leaves[0], leaves[1]);
    const right = await putInternal(PROVIDER_URL, bucketId, owner, leaves[2], new Uint8Array(32));
    const root = await putInternal(PROVIDER_URL, bucketId, owner, left, right);
    assert.ok(bytesEq(root, paddedMerkleRoot(leaves)), "root must be the padded root");

    const commit: any = await providerFetch(PROVIDER_URL, "/commit", {
      method: "POST",
      body: { bucket_id: Number(bucketId), data_roots: [toHex(root)] },
      sign: { signer: owner.signer, bucketId },
    });

    /** Open a challenge on `chunkIndex` of the committed root. */
    function challenge(chunkIndex: bigint): Promise<{ deadline: number; index: number }> {
      return challengeOffchain(api, owner, provider, bucketId, {
        mmrRoot: commit.mmr_root,
        startSeq: commit.start_seq,
        leafCount: commit.leaf_count,
        leafIndex: commit.leaf_indices[0],
        providerSignature: commit.provider_signature,
        chunkIndex,
      });
    }

    /** Answer `challengeId` with `proof` and assert the pallet accepts it. */
    async function respondAndExpectDefended(
      challengeId: { deadline: number; index: number },
      proof: unknown,
    ): Promise<void> {
      const result = await respondToChallenge(api, provider, challengeId, proof);
      const defended = api.event.StorageProvider.ChallengeDefended.filter(result.events as never);
      assert.strictEqual(defended.length, 1, "expected ChallengeDefended");
    }

    const tests: Array<{ name: string; fn: () => Promise<void> }> = [
      {
        name: "14.1 The provider answers a challenge on chunk 2 of a 3-chunk tree",
        fn: async () => {
          const challengeId = await challenge(LAST_CHUNK);
          const proof = await fetchChallengeProof(api, PROVIDER_URL, challengeId);
          await respondAndExpectDefended(challengeId, proof);
        },
      },
      {
        name: "14.2 A challenge on the padding slot is rejected or answered",
        fn: async () => {
          let challengeId: { deadline: number; index: number };
          try {
            challengeId = await challenge(PADDING_CHUNK);
          } catch (err) {
            // The error variant for an out-of-range `chunk_index` does not exist yet.
            assert.match((err as Error).message, /dispatch failed: Module::StorageProvider::/);
            console.log(`          challenge_offchain rejected: ${(err as Error).message}`);
            return;
          }

          const open = await api.query.StorageProvider.Challenges.getValue(
            challengeId.deadline,
            challengeId.index,
            READ_OPTS,
          );
          assert.strictEqual(open?.target.chunk_index, PADDING_CHUNK, "challenge must target slot 3");
          console.log(`          challenge_offchain accepted chunk_index=${PADDING_CHUNK}`);

          let proof: unknown;
          try {
            proof = await fetchChallengeProof(api, PROVIDER_URL, challengeId);
          } catch (err) {
            const message = (err as Error).message;
            console.log(`          provider response: ${message}`);
            assert.match(
              message,
              /^\/chunk_proof: 404 .*"not_found".*"chunk_3"/,
              "unexpected provider failure",
            );
            assert.fail(
              "the provider has no proof for padding slot 3, so the challenge stays open " +
                "until its deadline and the timeout sweep slashes the provider",
            );
          }
          await respondAndExpectDefended(challengeId, proof);
        },
      },
    ];

    await runSuite("14 - The provider can answer every challenge the pallet accepts", tests, {
      api,
      papi,
    });
  } finally {
    try {
      await restoreProviders();
    } catch (err) {
      console.log("  restoring provider settings failed:", (err as Error).message);
      process.exitCode = 1;
    }
    papi.destroy();
  }
}

main().catch((err) => {
  console.error(err);
  process.exitCode = 1;
}).finally(() => {
  process.exit(process.exitCode || 0);
});
