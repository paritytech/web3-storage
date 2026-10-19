// SPDX-License-Identifier: Apache-2.0

/**
 * E2E Workflow 13 - A provider cannot answer a challenge with a node's bytes
 *
 * Accounts: //Alice (provider), //Bob (bucket admin)
 *
 * The provider commits the tree over 4 chunks:
 *
 *           root
 *          /    \
 *         L      R
 *        / \    / \
 *       c0 c1  c2 c3
 *
 * Chunks hash with prefix `0x00` and internal nodes with `0x01`, so the 64
 * bytes of `L` (`hash(c0) || hash(c1)`) with the 1-step proof `[R]` do not
 * verify as chunk 0, and the chain rejects them with `InvalidProof`. The chain
 * also rejects the 1-step proof because 4 chunks need 2 siblings.
 *
 * 13.1 The provider answers a challenge on chunk 0 with the real chunk.
 * 13.2 A response to a challenge on chunk 0 with the bytes of `L` and the
 *      proof `[R]` is not accepted.
 *
 * Usage: node --import tsx e2e/13-node-bytes-as-chunk.ts [chain_ws] [provider_url]
 */

import assert from "node:assert";
import {
  bytesEq,
  challengeOffchain,
  computeCid,
  ensureProviderRegistered,
  fetchChallengeProof,
  makeSigner,
  DEFAULT_CHUNK_SIZE,
  merkleRoot,
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

  try {
    const { bucketId } = await negotiateAndEstablish(
      api,
      PROVIDER_URL,
      owner,
      provider,
      { maxBytes: 2_097_152n, duration: 200 },
      true, // finalize: the provider reads bucket membership from finalized state
    );

    // `/commit` accepts only DEFAULT_CHUNK_SIZE chunks, except a shorter last one.
    const chunks = [0, 1, 2, 3].map((i) =>
      new Uint8Array(i < 3 ? DEFAULT_CHUNK_SIZE : 32).fill(i + 1),
    );
    const leaves = chunks.map((c) => computeCid(c));
    for (const chunk of chunks) {
      await putChunk(PROVIDER_URL, bucketId, chunk, owner);
    }
    const left = await putInternal(PROVIDER_URL, bucketId, owner, leaves[0], leaves[1]);
    const right = await putInternal(PROVIDER_URL, bucketId, owner, leaves[2], leaves[3]);
    const root = await putInternal(PROVIDER_URL, bucketId, owner, left, right);
    assert.ok(bytesEq(root, merkleRoot(leaves)), "root must be the Merkle root of the chunks");

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

          await assert.rejects(
            respondToChallenge(api, provider, challengeId, forged),
            /dispatch failed: Module::StorageProvider::InvalidProof/,
          );
          // A dispatch error leaves the challenge open; close it with the real
          // proof so the timeout sweep does not slash the provider later.
          await respondToChallenge(api, provider, challengeId, real);
        },
      },
    ];

    await runSuite("13 - A provider cannot answer a challenge with a node's bytes", tests, {
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
