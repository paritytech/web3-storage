// SPDX-License-Identifier: Apache-2.0

/**
 * E2E Workflow 14 - Provider signs only canonical chunk trees
 *
 * Accounts: //Alice (provider), //Bob (bucket admin)
 *
 * `POST /commit` signs only the canonical tree: chunks of `DEFAULT_CHUNK_SIZE`
 * bytes except a shorter last chunk, and an odd last node moves up a level
 * unchanged. The provider cannot prove a chunk of a root with another shape.
 *
 * 14.1 `/commit` signs the canonical tree `R = H(H(c0, c1), c2)` over chunks of
 *      `DEFAULT_CHUNK_SIZE`, `DEFAULT_CHUNK_SIZE` and 10 bytes, and the provider
 *      defends a challenge on chunk 2.
 * 14.2 `/commit` rejects `H(c0, H(c1, c2))`: the pallet cannot verify a proof
 *      for `c2` against it, so a challenge on `c2` expires and slashes the
 *      provider.
 *
 * Usage: node --import tsx e2e/14-canonical-chunk-tree.ts [chain_ws] [provider_url]
 */

import assert from "node:assert";
import {
  bytesEq,
  challengeOffchain,
  computeCid,
  DEFAULT_CHUNK_SIZE,
  ensureProviderRegistered,
  fetchChallengeProof,
  makeSigner,
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
  const restore = await ensureSoleAcceptingProvider(api, provider);

  const { bucketId } = await negotiateAndEstablish(
    api,
    PROVIDER_URL,
    owner,
    provider,
    { maxBytes: 1_048_576n, duration: 200 },
    true, // finalize: the provider reads bucket membership from finalized state
  );

  async function putLeaves(sizes: number[]): Promise<Uint8Array[]> {
    const leaves: Uint8Array[] = [];
    for (const [i, size] of sizes.entries()) {
      const chunk = new Uint8Array(size).fill(i + 1);
      await putChunk(PROVIDER_URL, bucketId, chunk, owner);
      leaves.push(computeCid(chunk));
    }
    return leaves;
  }

  async function commitRoot(root: Uint8Array): Promise<any> {
    return providerFetch(PROVIDER_URL, "/commit", {
      method: "POST",
      body: { bucket_id: Number(bucketId), data_roots: [toHex(root)] },
      sign: { signer: owner.signer, bucketId },
    });
  }

  function assertNonCanonicalTree(err: unknown): true {
    const match = /^\/commit: (\d{3}) (.*)$/s.exec((err as Error).message);
    assert.ok(match, `unexpected error: ${(err as Error).message}`);
    assert.strictEqual(match[1], "400");
    assert.strictEqual(JSON.parse(match[2]).error, "non_canonical_tree");
    return true;
  }

  // Two full chunks and a 10-byte last chunk, shared by 14.1 and 14.2.
  const leaves = await putLeaves([DEFAULT_CHUNK_SIZE, DEFAULT_CHUNK_SIZE, 10]);

  const tests: Array<{ name: string; fn: () => Promise<void> }> = [];

  tests.push({
    name: "14.1 /commit signs the canonical chunk tree and the provider defends chunk 2",
    fn: async () => {
      const inner = await putInternal(PROVIDER_URL, bucketId, owner, leaves[0], leaves[1]);
      const root = await putInternal(PROVIDER_URL, bucketId, owner, inner, leaves[2]);
      assert.ok(bytesEq(root, merkleRoot(leaves)), "R must equal the canonical root");

      const commit = await commitRoot(root);
      const challengeId = await challengeOffchain(api, owner, provider, bucketId, {
        leafIndex: commit.leaf_indices[0],
        mmrRoot: commit.mmr_root,
        startSeq: commit.start_seq,
        leafCount: commit.leaf_count,
        providerSignature: commit.provider_signature,
        chunkIndex: 2n,
      });
      const proof = await fetchChallengeProof(api, PROVIDER_URL, challengeId);
      const result = await respondToChallenge(api, provider, challengeId, proof);
      const events = api.event.StorageProvider.ChallengeDefended.filter(result.events as never);
      assert.strictEqual(events.length, 1, "Expected ChallengeDefended event");
    },
  });

  tests.push({
    name: "14.2 /commit rejects a chunk tree that leans right",
    fn: async () => {
      // R = H(c0, H(c1, c2)): every node passes the provider's upload checks.
      const inner = await putInternal(PROVIDER_URL, bucketId, owner, leaves[1], leaves[2]);
      const root = await putInternal(PROVIDER_URL, bucketId, owner, leaves[0], inner);

      await assert.rejects(
        commitRoot(root),
        assertNonCanonicalTree,
        "provider must not sign a root it cannot prove",
      );
    },
  });

  await runSuite("14 - Provider signs only canonical chunk trees", tests, { api, papi });

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
