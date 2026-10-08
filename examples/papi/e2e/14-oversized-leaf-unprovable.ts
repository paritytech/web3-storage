// SPDX-License-Identifier: Apache-2.0

/**
 * E2E Workflow 14 - A leaf larger than MaxChunkSize cannot be proven
 *
 * Accounts: //Alice (provider), //Bob (bucket owner), //Charlie (outsider)
 *
 * `PUT /node` stores a leaf of any size up to the 256 MB body limit, and
 * `POST /commit` signs the MMR root over it. The SDK's `uploadChunk` sends the
 * whole payload as one leaf. A `Proof` response carries the chunk in a
 * `BoundedVec<u8, MaxChunkSize>` (256 KiB), so a leaf of 256 KiB + 1 byte has
 * no valid response. Any account can challenge it on a public bucket. At the
 * deadline `resolve_expired_challenge` slashes the provider's whole stake
 * (`crates/pallets/storage-provider/src/impls/challenges.rs`,
 * `slash_provider_for_failed_challenge`). The test stops before the deadline.
 *
 * 14.1 Control: a 256 KiB leaf is challenged and defended.
 * 14.2 A 256 KiB + 1 byte leaf is stored and signed by the provider.
 * 14.3 The provider cannot answer a challenge on it: the chain rejects the
 *      response that carries the real bytes.
 *
 * Usage: node --import tsx e2e/14-oversized-leaf-unprovable.ts [chain_ws] [provider_url]
 */

import assert from "node:assert";
import {
  challengeOffchain,
  createBucketWithPrimary,
  ensureProviderRegistered,
  fetchChallengeProof,
  makeSigner,
  providerFetch,
  respondToChallenge,
  uploadChunk,
} from "@web3-storage/sdk";
import { ensureSoleAcceptingProvider } from "../support.js";
import { negotiateSigned, runSuite, setupChain } from "./helpers.js";

const CHAIN_WS = process.argv[2] || "ws://127.0.0.1:2222";
const PROVIDER_URL = process.argv[3] || "http://127.0.0.1:3333";

/** `MaxChunkSize` in both runtimes (`runtimes/*\/src/storage.rs`). */
const MAX_CHUNK_SIZE = 256 * 1024;

async function main() {
  const provider = makeSigner("//Alice");
  const owner = makeSigner("//Bob");
  const outsider = makeSigner("//Charlie");

  const { papi, api } = await setupChain(CHAIN_WS);
  await ensureProviderRegistered(api, provider, PROVIDER_URL);
  const restoreProviders = await ensureSoleAcceptingProvider(api, provider);

  async function challengeExists(id: {
    deadline: number;
    index: number;
  }): Promise<boolean> {
    const c = await api.query.StorageProvider.Challenges.getValue(
      id.deadline,
      id.index,
      {
        at: "best",
      },
    );
    return c !== undefined;
  }

  async function waitUntil(
    cond: () => Promise<boolean>,
    what: string,
    timeoutMs = 180_000,
  ) {
    const end = Date.now() + timeoutMs;
    while (Date.now() < end) {
      if (await cond()) return;
      await new Promise((r) => setTimeout(r, 2_000));
    }
    throw new Error(`timed out waiting for ${what}`);
  }

  try {
    // Bob opens a public bucket with Alice as the primary.
    const signed = await negotiateSigned(api, PROVIDER_URL, owner, provider, {
      maxBytes: 4_194_304n,
      duration: 500,
    });
    const { bucketId } = await createBucketWithPrimary(
      api,
      owner,
      provider,
      signed,
      {
        visibility: "Public",
        mode: "finalized",
      },
    );
    console.log(`  bucket ${bucketId} (Public), primary //Alice`);

    async function uploadAndChallenge(size: number) {
      const payload = new Uint8Array(size).map((_, i) => (i * 7 + size) % 251);
      // Bob uses the SDK call as documented: one payload, one leaf, one commit.
      const upload = await uploadChunk(PROVIDER_URL, bucketId, payload, owner);
      console.log(
        `          uploadChunk(${size} B): leaf=${upload.hash.slice(0, 18)}… mmr_root=${String(upload.commit.mmr_root).slice(0, 18)}… leaf_count=${upload.commit.leaf_count}`,
      );
      // Charlie is neither a member nor the agreement owner.
      const challengeId = await challengeOffchain(
        api,
        outsider,
        provider,
        bucketId,
        {
          mmrRoot: upload.commit.mmr_root,
          startSeq: upload.commit.start_seq,
          leafCount: upload.commit.leaf_count,
          leafIndex: upload.commit.leaf_indices[0],
          providerSignature: upload.commit.provider_signature,
          chunkIndex: 0n,
        },
      );
      console.log(
        `          challenge_offchain by //Charlie accepted: deadline=${challengeId.deadline} index=${challengeId.index}`,
      );
      return { challengeId, payload };
    }

    let oversized: Awaited<ReturnType<typeof uploadAndChallenge>> | undefined;

    const tests: Array<{ name: string; fn: () => Promise<void> }> = [
      {
        name: "14.1 Control: a 256 KiB leaf is challenged and defended",
        fn: async () => {
          const { challengeId } = await uploadAndChallenge(MAX_CHUNK_SIZE);
          // The provider node runs with the challenge responder. Respond here
          // too in case the node has it disabled; whichever lands first wins.
          if (await challengeExists(challengeId)) {
            try {
              const proof = await fetchChallengeProof(
                api,
                PROVIDER_URL,
                challengeId,
              );
              await respondToChallenge(api, provider, challengeId, proof);
            } catch (err) {
              console.log(
                `          manual response skipped: ${(err as Error).message.slice(0, 120)}`,
              );
            }
          }
          await waitUntil(
            async () => !(await challengeExists(challengeId)),
            "the defence",
          );
          const stats = (
            await api.query.StorageProvider.Providers.getValue(
              provider.address,
              {
                at: "best",
              },
            )
          )?.stats;
          console.log(
            `          challenge closed, provider stats=${JSON.stringify(stats, (_, v) => (typeof v === "bigint" ? v.toString() : v))}`,
          );
        },
      },
      {
        name: "14.2 A 256 KiB + 1 byte leaf is stored and signed by the provider",
        fn: async () => {
          oversized = await uploadAndChallenge(MAX_CHUNK_SIZE + 1);
          // The provider serves the same bytes back as chunk 0 of the leaf.
          const proof: any = await providerFetch(PROVIDER_URL, "/chunk_proof", {
            params: {
              data_root: (
                await fetchChallengeProof(
                  api,
                  PROVIDER_URL,
                  oversized.challengeId,
                )
              ).mmr_proof.leaf.data_root,
              chunk_index: 0,
            },
          });
          const served = Buffer.from(proof.chunk_data, "base64");
          console.log(
            `          GET /chunk_proof: chunk_data=${served.length} B, siblings=${proof.proof.siblings.length}`,
          );
          assert.strictEqual(served.length, MAX_CHUNK_SIZE + 1);
        },
      },
      {
        name: "14.3 The chain rejects the response that carries the real bytes",
        fn: async () => {
          assert.ok(oversized, "14.2 must run first");
          const real = await fetchChallengeProof(
            api,
            PROVIDER_URL,
            oversized.challengeId,
          );
          let accepted = false;
          try {
            await respondToChallenge(
              api,
              provider,
              oversized.challengeId,
              real,
            );
            accepted = true;
          } catch (err) {
            console.log(
              `          respond_to_challenge rejected: ${(err as Error).message.slice(0, 200)}`,
            );
          }
          assert.strictEqual(
            accepted,
            false,
            "the chain accepted a chunk above MaxChunkSize",
          );
          assert.ok(
            await challengeExists(oversized.challengeId),
            "the challenge must stay open",
          );
        },
      },
    ];

    await runSuite(
      "14 - A leaf larger than MaxChunkSize cannot be proven",
      tests,
      {
        api,
        papi,
      },
    );
  } finally {
    try {
      await restoreProviders();
    } catch (err) {
      console.log(
        "  restoring provider settings failed:",
        (err as Error).message,
      );
      process.exitCode = 1;
    }
    papi.destroy();
  }
}

main()
  .catch((err) => {
    console.error(err);
    process.exitCode = 1;
  })
  .finally(() => {
    process.exit(process.exitCode || 0);
  });
