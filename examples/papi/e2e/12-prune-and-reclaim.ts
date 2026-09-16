// SPDX-License-Identifier: Apache-2.0

/**
 * E2E Workflow 12 — Prune & Reclaim (deletion reclaims paid capacity)
 *
 * Accounts: //Alice (provider), //Bob (client)
 *
 * Exercises the full deletion story: quota enforcement against the paid
 * max_bytes, the L0 prune (`POST /delete`), the admin's deletion
 * authorization (`/delete/confirm` — the on-chain `Deleted` challenge
 * defense), the checkpoint that makes the prune canonical, then physical
 * erasure with the quota headroom returning, so a fresh upload fits again.
 *
 * Ordering matters: the provider's GC syncs a bucket's quota from the chain
 * agreement on bucket-related events *after* the bucket exists locally, so
 * the first checkpoint (a BucketCheckpointed event) is what arms the quota.
 *
 * Usage: node e2e/12-prune-and-reclaim.js [chain_ws] [provider_url]
 */

import assert from "node:assert";
import {
  deleteData,
  downloadChunk,
  ensureProviderRegistered,
  fetchBucketUsage,
  fetchCheckpointSignature,
  makeSigner,
  pruneAndCheckpoint,
  submitClientCheckpoint,
  uploadChunk,
} from "@web3-storage/sdk";
import { ensureSoleAcceptingProvider } from "../support.js";
import { negotiateAndEstablish, runSuite, setupChain } from "./helpers.js";

const CHAIN_WS = process.argv[2] || "ws://127.0.0.1:2222";
const PROVIDER_URL = process.argv[3] || "http://127.0.0.1:3333";

/** Poll `fn` every `stepMs` until it returns truthy or `timeoutMs` elapses. */
async function pollUntil<T>(
  what: string,
  timeoutMs: number,
  stepMs: number,
  fn: () => Promise<T | null | false>,
): Promise<T> {
  const deadline = Date.now() + timeoutMs;
  for (;;) {
    const value = await fn();
    if (value) return value;
    if (Date.now() > deadline) throw new Error(`Timed out waiting for ${what}`);
    await new Promise((resolve) => setTimeout(resolve, stepMs));
  }
}

async function main() {
  const provider = makeSigner("//Alice");
  const client = makeSigner("//Bob");

  const { papi, api } = await setupChain(CHAIN_WS);
  await ensureProviderRegistered(api, provider, PROVIDER_URL);
  const restore = await ensureSoleAcceptingProvider(api, provider);

  // A quota tight enough that three 100-byte chunks nearly fill it.
  const maxBytes = 400n;
  const { bucketId } = await negotiateAndEstablish(
    api,
    PROVIDER_URL,
    client,
    provider,
    { maxBytes, duration: 600 },
    true, // finalize: uploads follow immediately
  );

  const chunkHashes: string[] = [];
  const chunk = (tag: number) => String.fromCharCode(65 + tag).repeat(100);

  const tests: Array<{ name: string; fn: () => Promise<void> }> = [];

  tests.push({
    name: "12.1 Upload three chunks and checkpoint",
    fn: async () => {
      for (let i = 0; i < 3; i++) {
        const { hash } = await uploadChunk(PROVIDER_URL, bucketId, chunk(i), client);
        chunkHashes.push(hash);
      }
      const ck = await fetchCheckpointSignature(PROVIDER_URL, bucketId);
      await submitClientCheckpoint(api, client, provider, bucketId, ck);
    },
  });

  tests.push({
    name: "12.2 Quota syncs from the agreement and usage matches uploads",
    fn: async () => {
      // The checkpoint event drives the provider's GC reconcile, which
      // clamps the bucket quota to the agreement's max_bytes.
      const usage = await pollUntil("quota sync", 90_000, 3_000, async () => {
        const u = await fetchBucketUsage(PROVIDER_URL, bucketId);
        return u.quotaSynced ? u : null;
      });
      assert.strictEqual(usage.maxBytes, maxBytes, "quota should equal agreement max_bytes");
      assert.strictEqual(usage.usedBytes, 300n, "three 100-byte chunks stored");
    },
  });

  tests.push({
    name: "12.3 Over-quota upload is refused (507 quota_exceeded)",
    fn: async () => {
      await assert.rejects(
        uploadChunk(PROVIDER_URL, bucketId, "z".repeat(150), client),
        /quota_exceeded/,
        "a 150-byte chunk must not fit into 400 - 300 remaining",
      );
    },
  });

  tests.push({
    name: "12.4 Prune the first two leaves and checkpoint the deletion",
    fn: async () => {
      const result = await pruneAndCheckpoint(
        api,
        PROVIDER_URL,
        client,
        provider,
        bucketId,
        2n, // new_start_seq: leaves 0 and 1 removed
      );
      assert.ok(result, "checkpoint of the pruned commitment should land");
    },
  });

  tests.push({
    name: "12.5 Rewind prune is refused (400 invalid_start_seq)",
    fn: async () => {
      await assert.rejects(
        deleteData(PROVIDER_URL, bucketId, 1n, client),
        /invalid_start_seq/,
        "start_seq can only advance",
      );
    },
  });

  {
    tests.push({
      name: "12.6 Physical erasure reclaims the quota headroom",
      fn: async () => {
        // Erasure requires the prune-checkpoint to be canonical and the
        // admin's deletion receipt — both done in 12.4 by pruneAndCheckpoint.
        const usage = await pollUntil("erasure + quota reclaim", 120_000, 3_000, async () => {
          const u = await fetchBucketUsage(PROVIDER_URL, bucketId);
          return u.usedBytes === 100n ? u : null;
        });
        assert.strictEqual(usage.usedBytes, 100n, "only the surviving leaf remains charged");
      },
    });

    tests.push({
      name: "12.7 Re-upload fits within the reclaimed headroom",
      fn: async () => {
        const { commit } = await uploadChunk(
          PROVIDER_URL,
          bucketId,
          "z".repeat(150),
          client,
        );
        assert.ok(commit.mmr_root, "the same-size upload that failed in 12.3 now fits");
      },
    });

    tests.push({
      name: "12.8 Pruned chunks are physically gone (404)",
      fn: async () => {
        await assert.rejects(
          downloadChunk(PROVIDER_URL, chunkHashes[0]),
          /404|not_found/,
          "erased chunk must not be downloadable",
        );
        // The surviving leaf's chunk is untouched.
        await downloadChunk(PROVIDER_URL, chunkHashes[2]);
      },
    });
  }

  await runSuite("12 — Prune & Reclaim", tests, { api, papi });

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
