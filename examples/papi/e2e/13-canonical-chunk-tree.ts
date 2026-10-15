// SPDX-License-Identifier: Apache-2.0

/**
 * E2E Workflow 13 - Provider signs only canonical chunk trees
 *
 * Accounts: //Alice (provider), //Bob (bucket admin)
 *
 * The provider's proof builder (`build_merkle_proof`) assumes the canonical
 * tree: chunks of `DEFAULT_CHUNK_SIZE` bytes except the last, and a binary tree
 * in which an odd last node moves up a level unchanged. The pallet derives the
 * chunk count from the data size. The provider becomes liable for every root it
 * signs in `POST /commit`, so it must sign only roots it can prove.
 *
 * 13.1 `/commit` signs the canonical tree `R = H(H(c0, c1), c2)` over chunks of
 *      `DEFAULT_CHUNK_SIZE`, `DEFAULT_CHUNK_SIZE` and 10 bytes, and the provider
 *      defends a challenge on chunk 2.
 * 13.2 `/commit` rejects `H(c0, H(c1, c2))`: the pallet cannot verify a proof
 *      for `c2` against it, so a signed root of this shape lets the bucket admin
 *      slash the provider's whole stake with `SlashReason::InvalidProof`.
 * 13.3 `/commit` rejects the canonical shape over chunks that are not
 *      `DEFAULT_CHUNK_SIZE` bytes, because the derived chunk count is wrong.
 *
 * Usage: node --import tsx e2e/13-canonical-chunk-tree.ts [chain_ws] [provider_url]
 */

import assert from "node:assert";
import {
  bytesEq,
  challengeOffchain,
  computeCid,
  DEFAULT_CHUNK_SIZE,
  ensureProviderRegistered,
  fetchChallengeProof,
  hashChildren,
  makeSigner,
  merkleRoot,
  providerFetch,
  putChunk,
  respondToChallenge,
  toHex,
  type ChainSigner,
} from "@web3-storage/sdk";
import { ensureSoleAcceptingProvider } from "../support.js";
import { negotiateAndEstablish, runSuite, setupChain } from "./helpers.js";

const CHAIN_WS = process.argv[2] || "ws://127.0.0.1:2222";
const PROVIDER_URL = process.argv[3] || "http://127.0.0.1:3333";

/**
 * PUT a self-consistent internal node: `data = left || right` and
 * `hash = blake2(data)`, so the provider's hash check passes.
 */
async function putInternal(
  bucketId: bigint,
  signer: ChainSigner,
  left: Uint8Array,
  right: Uint8Array,
): Promise<Uint8Array> {
  const data = new Uint8Array(64);
  data.set(left, 0);
  data.set(right, 32);
  const { cid } = await putChunk(PROVIDER_URL, bucketId, data, signer, [left, right]);
  assert.ok(bytesEq(cid, hashChildren(left, right)), "internal node hash must equal H(left, right)");
  return cid;
}

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

  /** Upload leaf chunks of the given sizes, filled with 1, 2, 3, ..., and return their hashes. */
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

  /** Assert `err` is the provider's `400 non_canonical_tree` rejection of `/commit`. */
  function assertNonCanonicalTree(err: unknown): true {
    const match = /^\/commit: (\d{3}) (.*)$/s.exec((err as Error).message);
    assert.ok(match, `unexpected error: ${(err as Error).message}`);
    assert.strictEqual(match[1], "400");
    assert.strictEqual(JSON.parse(match[2]).error, "non_canonical_tree");
    return true;
  }

  // Two full chunks and a 10-byte last chunk, shared by 13.1 and 13.2.
  const leaves = await putLeaves([DEFAULT_CHUNK_SIZE, DEFAULT_CHUNK_SIZE, 10]);

  const tests: Array<{ name: string; fn: () => Promise<void> }> = [];

  tests.push({
    name: "13.1 /commit signs the canonical chunk tree and the provider defends chunk 2",
    fn: async () => {
      // R = H(H(c0, c1), c2): the last chunk moves up a level without a sibling.
      const inner = await putInternal(bucketId, owner, leaves[0], leaves[1]);
      const root = await putInternal(bucketId, owner, inner, leaves[2]);
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
    name: "13.2 /commit rejects a chunk tree that leans right",
    fn: async () => {
      // R = H(c0, H(c1, c2)): every node passes the provider's upload checks.
      const inner = await putInternal(bucketId, owner, leaves[1], leaves[2]);
      const root = await putInternal(bucketId, owner, leaves[0], inner);

      await assert.rejects(
        commitRoot(root),
        assertNonCanonicalTree,
        "provider must not sign a root it cannot prove",
      );
    },
  });

  tests.push({
    name: "13.3 /commit rejects chunks that are not DEFAULT_CHUNK_SIZE bytes",
    fn: async () => {
      // The canonical shape, but the chunk count derived from 96 bytes is 1, not 3.
      const small = await putLeaves([32, 32, 32]);
      const inner = await putInternal(bucketId, owner, small[0], small[1]);
      const root = await putInternal(bucketId, owner, inner, small[2]);
      assert.ok(bytesEq(root, merkleRoot(small)), "R must equal the canonical root");

      await assert.rejects(
        commitRoot(root),
        assertNonCanonicalTree,
        "provider must not sign a root whose chunk sizes the pallet cannot verify",
      );
    },
  });

  await runSuite("13 - Provider signs only canonical chunk trees", tests, { api, papi });

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
