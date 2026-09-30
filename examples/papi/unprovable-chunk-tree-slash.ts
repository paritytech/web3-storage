// SPDX-License-Identifier: Apache-2.0

/**
 * Unprovable chunk tree slashes the provider
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
 * 13.2 A challenge on `c2` of `R` does not slash the provider: either
 *      `/commit` rejects `R`, or the provider defends the challenge.
 * 13.3 `/commit` accepts the padded tree over the same chunks, and the
 *      provider defends a challenge on `c2` of that root.
 *
 * Usage:
 * 1. `just start-e2e-chain`
 * 2. `just start-provider`
 * 3. node --import tsx unprovable-chunk-tree-slash.ts ws://127.0.0.1:2222 http://127.0.0.1:3333
 */

import assert from "node:assert";
import {
  addStake,
  bytesEq,
  challengeOffchain,
  computeCid,
  ensureProviderRegistered,
  fetchChallengeProof,
  hashChildren,
  makeSigner,
  paddedMerkleRoot,
  providerFetch,
  putChunk,
  READ_OPTS,
  respondToChallenge,
  toHex,
  type ChainSigner,
} from "@web3-storage/sdk";
import { ensureSoleAcceptingProvider } from "./support.js";
import { negotiateAndEstablish, runSuite, setupChain } from "./e2e/helpers.js";

const CHAIN_WS = process.argv[2] || "ws://127.0.0.1:2222";
const PROVIDER_URL = process.argv[3] || "http://127.0.0.1:3333";

/** No binary proof for chunk 2 verifies against the unbalanced root `R`. */
const CHALLENGED_CHUNK = 2n;

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
  const zero = new Uint8Array(32);

  async function commitRoot(root: Uint8Array): Promise<any> {
    return providerFetch(PROVIDER_URL, "/commit", {
      method: "POST",
      body: { bucket_id: Number(bucketId), data_roots: [toHex(root)] },
      sign: { signer: owner.signer, bucketId },
    });
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
        /\/commit: 4\d\d/,
        "provider must not sign a root it cannot prove",
      );
    },
  });


  let slashedAmount = 0n;
  tests.push({
    name: "13.2 A challenge on chunk 2 of the unbalanced tree does not slash the provider",
    fn: async () => {
      // H(H(c0, c1), c2): the current state, 
      const inner = await putInternal(bucketId, owner, leaves[0], leaves[1]);
      const root = await putInternal(bucketId, owner, inner, leaves[2]);

      let commit: any;
      try {
        commit = await commitRoot(root);
      } catch (err) {
        // The provider did not sign R, so no challenge on R is possible.
        assert.match((err as Error).message, /\/commit: 4\d\d/);
        return;
      }

      const info = await api.query.StorageProvider.Providers.getValue(provider.address, READ_OPTS);
      const stakeBefore = info!.stake;

      const challengeId = await challengeOffchain(api, owner, provider, bucketId, {
        mmrRoot: commit.mmr_root,
        startSeq: commit.start_seq,
        leafCount: commit.leaf_count,
        leafIndex: commit.leaf_indices[0],
        providerSignature: commit.provider_signature,
        chunkIndex: CHALLENGED_CHUNK,
      });
      const proof = await fetchChallengeProof(api, PROVIDER_URL, challengeId);
      const result = await respondToChallenge(api, provider, challengeId, proof);

      const slashed = api.event.StorageProvider.ChallengeSlashed.filter(result.events as never);
      for (const ev of slashed) {
        slashedAmount += ev.payload.slashed_amount;
        console.log(
          `          ChallengeSlashed reason=${ev.payload.reason.type} slashed_amount=${ev.payload.slashed_amount}`,
        );
      }
      assert.strictEqual(slashed.length, 0, "provider must not be slashed");

      const after = await api.query.StorageProvider.Providers.getValue(provider.address, READ_OPTS);
      assert.strictEqual(after!.stake, stakeBefore, "provider stake must not change");
    },
  });

  // Restore Alice's stake so later runs against the same chain still work.
  if (slashedAmount > 0n) {
    try {
      await addStake(api, provider, slashedAmount);
    } catch (err) {
      console.log("  restoring provider stake failed:", (err as Error).message);
      process.exitCode = 1;
    }
  }

  tests.push({
    name: "13.3 /commit accepts the padded tree and the provider defends chunk 2",
    fn: async () => {
      // H(H(c0, c1), H(c2, 0)): the shape `verify_merkle_proof` accepts.
      const left = await putInternal(bucketId, owner, leaves[0], leaves[1]);
      const right = await putInternal(bucketId, owner, leaves[2], zero);
      const root = await putInternal(bucketId, owner, left, right);
      assert.ok(bytesEq(root, paddedMerkleRoot(leaves)), "root must be the padded root");

      const commit = await commitRoot(root);
      const info = await api.query.StorageProvider.Providers.getValue(provider.address, READ_OPTS);
      const stakeBefore = info!.stake;

      const challengeId = await challengeOffchain(api, owner, provider, bucketId, {
        mmrRoot: commit.mmr_root,
        startSeq: commit.start_seq,
        leafCount: commit.leaf_count,
        leafIndex: commit.leaf_indices[0],
        providerSignature: commit.provider_signature,
        chunkIndex: CHALLENGED_CHUNK,
      });
      const proof = await fetchChallengeProof(api, PROVIDER_URL, challengeId);
      const result = await respondToChallenge(api, provider, challengeId, proof);
      const defended = api.event.StorageProvider.ChallengeDefended.filter(result.events as never);
      assert.strictEqual(defended.length, 1, "expected ChallengeDefended");

      const after = await api.query.StorageProvider.Providers.getValue(provider.address, READ_OPTS);
      assert.strictEqual(after!.stake, stakeBefore, "provider stake must not change");
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
