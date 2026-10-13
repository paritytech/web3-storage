// SPDX-License-Identifier: Apache-2.0

/**
 * E2E Workflow 13 - Provider signs only provable chunk trees
 *
 * Accounts: //Alice (provider), //Bob (bucket admin)
 *
 * The provider's proof builder (`build_merkle_proof`) assumes the zero-padded
 * balanced binary tree. The provider becomes liable for every root it signs in
 * `POST /commit`, so it must sign only roots it can prove.
 *
 * 13.1 `/commit` rejects the unbalanced tree `R = H(H(c0, c1), c2)`. No proof
 *      for `c2` verifies against `R`, so a signed `R` lets the bucket admin
 *      slash the provider's whole stake with `SlashReason::InvalidProof`.
 *
 * Usage: node --import tsx e2e/13-unprovable-chunk-tree.ts [chain_ws] [provider_url]
 */

import assert from "node:assert";
import {
  bytesEq,
  computeCid,
  ensureProviderRegistered,
  hashChildren,
  makeSigner,
  paddedMerkleRoot,
  providerFetch,
  putChunk,
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

  const chunks = [0, 1, 2].map((i) => new Uint8Array(32).fill(i + 1));
  const leaves = chunks.map((c) => computeCid(c));

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

  const tests: Array<{ name: string; fn: () => Promise<void> }> = [];

  tests.push({
    name: "13.1 /commit rejects an unbalanced chunk tree",
    fn: async () => {
      for (let i = 0; i < chunks.length; i++) {
        await putChunk(PROVIDER_URL, bucketId, chunks[i], owner);
      }
      // R = H(H(c0, c1), c2): every node passes the provider's upload checks.
      const inner = await putInternal(bucketId, owner, leaves[0], leaves[1]);
      const root = await putInternal(bucketId, owner, inner, leaves[2]);
      assert.ok(
        !bytesEq(root, paddedMerkleRoot(leaves)),
        "R must differ from the zero-padded balanced root",
      );

      await assert.rejects(
        commitRoot(root),
        assertNonCanonicalTree,
        "provider must not sign a root it cannot prove",
      );
    },
  });

  await runSuite("13 - Provider signs only provable chunk trees", tests, { api, papi });

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
