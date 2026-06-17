// SPDX-License-Identifier: Apache-2.0

/**
 * E2E Workflow 10 — Edge Cases & Adversarial
 *
 * Tests: balance accounting, capacity tracking, frozen buckets,
 * concurrent operations, data integrity.
 *
 * Usage: node e2e/10-edge-cases-and-adversarial.js [chain_ws] [provider_url]
 */

import assert from "node:assert";
import { blake2b256 } from "@polkadot-labs/hdkd-helpers";
import {
  endAgreement,
  fetchCheckpointSignature,
  freezeBucket,
  setMember,
  submitClientCheckpoint,
  uploadChunk,
} from "../api.js";
import {
  ensureProviderRegistered,
  ensureSoleAcceptingProvider,
  makeSigner,
  READ_OPTS,
  toHex,
  waitForBlock,
} from "../common.js";
import {
  getFree,
  negotiateAndEstablish,
  runSuite,
  submitTxExpectFailure,
  setupChain,
} from "./helpers.js";

const CHAIN_WS = process.argv[2] || "ws://127.0.0.1:2222";
const PROVIDER_URL = process.argv[3] || "http://127.0.0.1:3333";

async function main() {
  const provider = makeSigner("//Alice");
  const bob = makeSigner("//Bob");
  const dave = makeSigner("//Dave");
  const eve = makeSigner("//Eve");

  const { papi, api } = await setupChain(CHAIN_WS);
  await ensureProviderRegistered(api, provider, PROVIDER_URL);
  const restore = await ensureSoleAcceptingProvider(api, provider);

  const maxBytes = 1_048_576n;
  const duration = 50;

  const tests = [];

  // ── Balance Accounting ────────────────────────────────────────────────────

  tests.push({
    name: "10.1 Establish locks the payment",
    fn: async () => {
      // Redeeming signed terms reserves price_per_byte × max_bytes × duration
      // up front. At price 1 that's ~52M units — far above any tx fee.
      const balBefore = await getFree(api, bob);
      await negotiateAndEstablish(api, PROVIDER_URL, bob, provider, { maxBytes, duration });
      const balAfter = await getFree(api, bob);
      assert.ok(
        balAfter < balBefore,
        `Balance should decrease after establish (payment locked): ${balAfter} < ${balBefore}`
      );
    },
  });

  // ── Capacity Tracking ─────────────────────────────────────────────────────

  tests.push({
    name: "10.2 committed_bytes increments on establish",
    fn: async () => {
      const infoBefore = await api.query.StorageProvider.Providers.getValue(provider.address, READ_OPTS);
      const beforeBytes = infoBefore.committed_bytes;
      await negotiateAndEstablish(api, PROVIDER_URL, bob, provider, { maxBytes, duration: 100 });
      const infoAfter = await api.query.StorageProvider.Providers.getValue(provider.address, READ_OPTS);
      assert.ok(
        infoAfter.committed_bytes > beforeBytes,
        `committed_bytes should increase: ${infoAfter.committed_bytes} > ${beforeBytes}`
      );
    },
  });

  tests.push({
    name: "10.3 committed_bytes decrements on end",
    fn: async () => {
      const { bucketId } = await negotiateAndEstablish(api, PROVIDER_URL, bob, provider, {
        maxBytes,
        duration: 10,
      });
      const infoBefore = await api.query.StorageProvider.Providers.getValue(provider.address, READ_OPTS);
      const agreement = await api.query.StorageProvider.StorageAgreements.getValue(
        bucketId,
        provider.address,
        READ_OPTS
      );
      await waitForBlock(papi, Number(agreement.expires_at));
      await endAgreement(api, bob, provider, bucketId, "Pay");
      const infoAfter = await api.query.StorageProvider.Providers.getValue(provider.address, READ_OPTS);
      assert.ok(
        infoAfter.committed_bytes < infoBefore.committed_bytes,
        `committed_bytes should decrease: ${infoAfter.committed_bytes} < ${infoBefore.committed_bytes}`
      );
    },
  });

  // ── Frozen Bucket Semantics ───────────────────────────────────────────────

  tests.push({
    name: "10.5 Freeze is irreversible",
    fn: async () => {
      const { bucketId } = await negotiateAndEstablish(api, PROVIDER_URL, bob, provider, {
        maxBytes,
        duration: 100,
      });
      // freeze_bucket requires a snapshot (checkpoint) to exist.
      await uploadChunk(PROVIDER_URL, bucketId, "data for snapshot");
      const ck = await fetchCheckpointSignature(PROVIDER_URL, bucketId);
      await submitClientCheckpoint(api, bob, provider, bucketId, ck);
      await freezeBucket(api, bob, bucketId);
      const bucket = await api.query.StorageProvider.Buckets.getValue(bucketId, READ_OPTS);
      assert.ok(bucket.frozen_start_seq !== undefined, "Bucket should be frozen");
      // There's no "unfreeze" extrinsic — verify by checking the bucket stays frozen.
      // Attempting to freeze again should fail (already frozen).
      const tx = api.tx.StorageProvider.freeze_bucket({ bucket_id: bucketId });
      await submitTxExpectFailure(tx, bob.signer, "BucketFrozen", "10.5");
    },
  });

  tests.push({
    name: "10.6 Checkpoint after freeze",
    fn: async () => {
      const { bucketId } = await negotiateAndEstablish(api, PROVIDER_URL, bob, provider, {
        maxBytes,
        duration: 100,
      });
      // Upload some data.
      await uploadChunk(PROVIDER_URL, bucketId, "pre-freeze data");
      // Checkpoint before freeze.
      const ck1 = await fetchCheckpointSignature(PROVIDER_URL, bucketId);
      await submitClientCheckpoint(api, bob, provider, bucketId, ck1);
      // Freeze.
      await freezeBucket(api, bob, bucketId);
      // Upload more data.
      await uploadChunk(PROVIDER_URL, bucketId, "post-freeze data");
      // Checkpoint after freeze — should still work (captures frozen_start_seq).
      const ck2 = await fetchCheckpointSignature(PROVIDER_URL, bucketId);
      const result = await submitClientCheckpoint(api, bob, provider, bucketId, ck2);
      const events = api.event.StorageProvider.BucketCheckpointed.filter(result.events);
      assert.strictEqual(events.length, 1, "Checkpoint after freeze should emit event");
    },
  });

  // ── Concurrent Operations ─────────────────────────────────────────────────

  tests.push({
    name: "10.7 Same account member of multiple buckets",
    fn: async () => {
      const { bucketId: bucket1 } = await negotiateAndEstablish(
        api, PROVIDER_URL, dave, provider, { maxBytes, duration: 100 }
      );
      const { bucketId: bucket2 } = await negotiateAndEstablish(
        api, PROVIDER_URL, dave, provider, { maxBytes, duration: 100 }
      );
      await setMember(api, dave, bucket1, eve, "Writer");
      await setMember(api, dave, bucket2, eve, "Reader");
      const eveBuckets = await api.query.StorageProvider.MemberBuckets.getValue(eve.address, READ_OPTS);
      assert.ok(eveBuckets.some((id) => id === bucket1), "Eve should be member of bucket1");
      assert.ok(eveBuckets.some((id) => id === bucket2), "Eve should be member of bucket2");
    },
  });

  // ── Data Integrity ────────────────────────────────────────────────────────

  tests.push({
    name: "10.8 Upload verify blake2-256",
    fn: async () => {
      const { bucketId } = await negotiateAndEstablish(api, PROVIDER_URL, bob, provider, {
        maxBytes,
        duration: 100,
      });
      const data = "integrity check data for blake2-256";
      const bytes = new TextEncoder().encode(data);
      const expectedHash = toHex(blake2b256(bytes));
      const { hash } = await uploadChunk(PROVIDER_URL, bucketId, data);
      assert.strictEqual(hash, expectedHash, "Provider hash should match local blake2-256");
    },
  });

  tests.push({
    name: "10.9 Identical content → same hash, different MMR leaves",
    fn: async () => {
      const { bucketId } = await negotiateAndEstablish(api, PROVIDER_URL, bob, provider, {
        maxBytes,
        duration: 100,
      });
      const data = "identical content for dedup test";
      const r1 = await uploadChunk(PROVIDER_URL, bucketId, data);
      const r2 = await uploadChunk(PROVIDER_URL, bucketId, data);
      assert.strictEqual(r1.hash, r2.hash, "Hashes should match for identical content");
      assert.notStrictEqual(
        r1.commit.leaf_indices[0],
        r2.commit.leaf_indices[0],
        "Leaf indices should differ"
      );
    },
  });

  await runSuite("10 — Edge Cases & Adversarial", tests, { api, papi });

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
