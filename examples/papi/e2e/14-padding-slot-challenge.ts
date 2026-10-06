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
 * so it accepts a challenge on slot 3 and on any index past the 4 slots.
 * `verify_merkle_proof` uses only the low bits of the index, so index 4 is
 * slot 0 and index 7 is slot 3.
 *
 * The provider answers a padding slot with empty `chunk_data` and the Merkle
 * path from the zero leaf, which the pallet accepts.
 *
 * 14.1 The provider answers a challenge on chunk 2 of a 3-chunk tree.
 * 14.2 The provider answers a challenge on chunk 3 (the padding slot).
 * 14.3 The provider answers challenges on chunk 4 (slot 0) and chunk 7
 *      (slot 3), which are past the padded size.
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

/** Indices past the 4 slots: 4 is slot 0 and 7 is slot 3. */
const PAST_PADDED_CHUNKS = [4n, 7n];

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
        name: "14.2 The provider answers a challenge on the padding slot",
        fn: async () => {
          const challengeId = await challenge(PADDING_CHUNK);
          const proof = await fetchChallengeProof(api, PROVIDER_URL, challengeId);
          await respondAndExpectDefended(challengeId, proof);
        },
      },
      {
        name: "14.3 The provider answers challenges past the padded size",
        fn: async () => {
          for (const chunkIndex of PAST_PADDED_CHUNKS) {
            const challengeId = await challenge(chunkIndex);
            const proof = await fetchChallengeProof(api, PROVIDER_URL, challengeId);
            await respondAndExpectDefended(challengeId, proof);
          }
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
