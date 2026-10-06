// SPDX-License-Identifier: Apache-2.0

/**
 * E2E Workflow 13 - A provider cannot answer a challenge with a node's bytes
 *
 * Accounts: //Alice (provider), //Bob (bucket admin)
 *
 * The provider commits the padded tree over 4 chunks:
 *
 *           root
 *          /    \
 *         L      R
 *        / \    / \
 *       c0 c1  c2 c3
 *
 * `verify_merkle_proof` accepts a proof of any length, and chunks and nodes
 * hash the same way. So the 64 bytes of `L` (`hash(c0) || hash(c1)`) with the
 * 1-step proof `[R]` reach `root`, and the chain takes them as chunk 0. A
 * provider that keeps only `L` and `R` can defend every challenge.
 *
 * 13.1 The provider answers a challenge on chunk 0 with the real chunk.
 * 13.2 A response to a challenge on chunk 0 with the bytes of `L` and the
 *      proof `[R]` is not accepted.
 *
 * Usage: node --import tsx e2e/13-node-bytes-as-chunk.ts [chain_ws] [provider_url]
 */

import assert from "node:assert";
import {
  addStake,
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

async function main() {
  const provider = makeSigner("//Alice");
  const owner = makeSigner("//Bob");

  const { papi, api } = await setupChain(CHAIN_WS);
  await ensureProviderRegistered(api, provider, PROVIDER_URL);
  const restoreProviders = await ensureSoleAcceptingProvider(api, provider);
  let slashedAmount = 0n;

  try {
    const { bucketId } = await negotiateAndEstablish(
      api,
      PROVIDER_URL,
      owner,
      provider,
      { maxBytes: 1_048_576n, duration: 200 },
      true, // finalize: the provider reads bucket membership from finalized state
    );

    const chunks = [0, 1, 2, 3].map((i) => new Uint8Array(32).fill(i + 1));
    const leaves = chunks.map((c) => computeCid(c));
    for (const chunk of chunks) {
      await putChunk(PROVIDER_URL, bucketId, chunk, owner);
    }
    const left = await putInternal(PROVIDER_URL, bucketId, owner, leaves[0], leaves[1]);
    const right = await putInternal(PROVIDER_URL, bucketId, owner, leaves[2], leaves[3]);
    const root = await putInternal(PROVIDER_URL, bucketId, owner, left, right);
    assert.ok(bytesEq(root, paddedMerkleRoot(leaves)), "root must be the padded root");

    const commit: any = await providerFetch(PROVIDER_URL, "/commit", {
      method: "POST",
      body: { bucket_id: Number(bucketId), data_roots: [toHex(root)] },
      sign: { signer: owner.signer, bucketId },
    });

    function challengeChunk0(): Promise<{ deadline: number; index: number }> {
      return challengeOffchain(api, owner, provider, bucketId, {
        mmrRoot: commit.mmr_root,
        startSeq: commit.start_seq,
        leafCount: commit.leaf_count,
        leafIndex: commit.leaf_indices[0],
        providerSignature: commit.provider_signature,
        chunkIndex: 0n,
      });
    }

    const tests: Array<{ name: string; fn: () => Promise<void> }> = [
      {
        name: "13.1 The provider answers a challenge on chunk 0 with the real chunk",
        fn: async () => {
          const challengeId = await challengeChunk0();
          const proof = await fetchChallengeProof(api, PROVIDER_URL, challengeId);
          const result = await respondToChallenge(api, provider, challengeId, proof);
          const defended = api.event.StorageProvider.ChallengeDefended.filter(
            result.events as never,
          );
          assert.strictEqual(defended.length, 1, "expected ChallengeDefended");
        },
      },
      {
        name: "13.2 A response with the bytes of node L as chunk 0 is not accepted",
        fn: async () => {
          const challengeId = await challengeChunk0();
          // Keep the real MMR proof; replace the chunk with the bytes of `L`
          // and the 2-step chunk proof with the 1-step proof `[R]`.
          const real = await fetchChallengeProof(api, PROVIDER_URL, challengeId);
          const nodeBytes = new Uint8Array(64);
          nodeBytes.set(leaves[0], 0);
          nodeBytes.set(leaves[1], 32);
          const forged = {
            ...real,
            chunk_data: nodeBytes,
            chunk_proof: { siblings: [toHex(right)], path: [false] },
          };

          let defended = 0;
          try {
            const result = await respondToChallenge(api, provider, challengeId, forged);
            defended = api.event.StorageProvider.ChallengeDefended.filter(
              result.events as never,
            ).length;
            for (const ev of api.event.StorageProvider.ChallengeSlashed.filter(
              result.events as never,
            )) {
              slashedAmount += ev.payload.slashed_amount;
              console.log(`          ChallengeSlashed reason=${ev.payload.reason.type}`);
            }
          } catch (err) {
            console.log(`          respond_to_challenge rejected: ${(err as Error).message}`);
            // A dispatch error leaves the challenge open; close it with the real
            // proof so the timeout sweep does not slash the provider later.
            await respondToChallenge(api, provider, challengeId, real);
            // The error variant for a proof of the wrong length does not exist yet.
            assert.match((err as Error).message, /dispatch failed: Module::StorageProvider::/);
          }

          console.log(`          ChallengeDefended=${defended}`);
          assert.strictEqual(
            defended,
            0,
            "the chain accepted the 64 bytes of node L as chunk 0 with a 1-step proof",
          );
        },
      },
    ];

    await runSuite("13 - A provider cannot answer a challenge with a node's bytes", tests, {
      api,
      papi,
    });
  } finally {
    // Restore Alice's stake if the forged response was slashed.
    if (slashedAmount > 0n) {
      try {
        await addStake(api, provider, slashedAmount);
      } catch (err) {
        console.log("  restoring provider stake failed:", (err as Error).message);
        process.exitCode = 1;
      }
    }
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
