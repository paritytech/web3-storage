// SPDX-License-Identifier: Apache-2.0

/**
 * E2E Workflow 14 - A leaf larger than MaxChunkSize is rejected
 *
 * Accounts: //Alice (provider), //Bob (bucket owner), //Charlie (outsider)
 *
 * A `Proof` response carries the chunk in a `BoundedVec<u8, MaxChunkSize>`
 * (256 KiB), so a leaf of 256 KiB + 1 byte has no valid response. A provider
 * that signed such a leaf would lose its whole stake to any challenger at the
 * deadline (`slash_provider_for_failed_challenge`). The provider rejects the
 * leaf at `PUT /node`, so it never signs a commitment over it.
 *
 * 14.1 Control: a 256 KiB leaf is challenged and defended.
 * 14.2 `PUT /node` of a 256 KiB + 1 byte leaf is rejected, and the provider
 *      has no root to commit.
 * 14.3 The SDK's `uploadChunk` throws before it sends the oversized leaf.
 *
 * Usage: node --import tsx e2e/14-oversized-leaf-rejected.ts [chain_ws] [provider_url]
 */

import assert from "node:assert";
import {
  bytesToBase64,
  challengeOffchain,
  createBucketWithPrimary,
  ensureProviderRegistered,
  fetchChallengeProof,
  hashLeaf,
  makeSigner,
  MAX_CHUNK_SIZE,
  providerFetch,
  respondToChallenge,
  uploadChunk,
} from "@web3-storage/sdk";
import { ensureSoleAcceptingProvider } from "../support.js";
import { negotiateSigned, runSuite, setupChain } from "./helpers.js";

const CHAIN_WS = process.argv[2] || "ws://127.0.0.1:2222";
const PROVIDER_URL = process.argv[3] || "http://127.0.0.1:3333";

function toHex(bytes: Uint8Array): string {
  return `0x${Buffer.from(bytes).toString("hex")}`;
}

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
        name: "14.2 PUT /node of a 256 KiB + 1 byte leaf is rejected",
        fn: async () => {
          const payload = new Uint8Array(MAX_CHUNK_SIZE + 1).fill(7);
          const hash = toHex(hashLeaf(payload));
          const sign = { signer: owner.signer, bucketId };
          // `providerFetch` throws `<path>: <status> <body>` on a non-2xx reply.
          await assert.rejects(
            providerFetch(PROVIDER_URL, "/node", {
              method: "PUT",
              body: {
                bucket_id: Number(bucketId),
                hash,
                data: bytesToBase64(payload),
                children: null,
              },
              sign,
            }),
            (err: Error) => {
              console.log(`          PUT ${err.message.slice(0, 160)}`);
              return /^\/node: 400 .*chunk_too_large/.test(err.message);
            },
          );

          // The leaf was not stored, so there is no root to commit and sign.
          await assert.rejects(
            providerFetch(PROVIDER_URL, "/commit", {
              method: "POST",
              body: { bucket_id: Number(bucketId), data_roots: [hash] },
              sign,
            }),
            (err: Error) => /^\/commit: 404 .*root_not_found/.test(err.message),
          );
        },
      },
      {
        name: "14.3 uploadChunk throws before it sends an oversized leaf",
        fn: async () => {
          await assert.rejects(
            uploadChunk(
              PROVIDER_URL,
              bucketId,
              new Uint8Array(MAX_CHUNK_SIZE + 1),
              owner,
            ),
            RangeError,
          );
        },
      },
    ];

    await runSuite(
      "14 - A leaf larger than MaxChunkSize is rejected",
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
