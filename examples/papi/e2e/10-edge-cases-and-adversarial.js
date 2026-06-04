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
  createBucket,
  createBucketWithStorage,
  endAgreement,
  fetchCheckpointSignature,
  freezeBucket,
  rejectAgreement,
  requestPrimaryAgreement,
  setMember,
  submitClientCheckpoint,
  uploadChunk,
  withdrawAgreementRequest,
} from "../api.js";
import {
  ensureProviderRegistered,
  ensureSoleAcceptingProvider,
  makeSigner,
  toHex,
  waitForAgreementAcceptance,
  waitForBlock,
} from "../common.js";
import { runSuite, submitTxExpectFailure, setupChain, getFree } from "./helpers.js";

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
  const maxPayment = maxBytes * BigInt(duration) * 10n;

  const tests = [];

  // ── Balance Accounting ────────────────────────────────────────────────────

  tests.push({
    name: "10.1 Request + reject returns funds",
    fn: async () => {
      const balBefore = await getFree(api, bob);
      const bucketId = await createBucket(api, bob);
      await requestPrimaryAgreement(api, bob, provider, bucketId, {
        max_bytes: maxBytes,
        duration,
        max_payment: maxPayment,
      });
      const balAfterRequest = await getFree(api, bob);
      assert.ok(balAfterRequest < balBefore, "Balance should decrease after request (payment locked)");
      await rejectAgreement(api, provider, bucketId);
      const balAfterReject = await getFree(api, bob);
      // After rejection, the locked payment is returned. Balance should recover
      // (minus tx fees for the 3 extrinsics: create_bucket + request + reject was provider's).
      assert.ok(
        balAfterReject > balAfterRequest,
        `Balance should increase after reject: ${balAfterReject} > ${balAfterRequest}`
      );
    },
  });

  tests.push({
    name: "10.2 Request + withdraw returns funds",
    fn: async () => {
      // Use a large duration so the reserved payment far exceeds tx fees.
      // The pallet reserves price_per_byte * max_bytes * duration, which at
      // price_per_byte=1 must be large enough to dwarf the withdraw tx fee.
      const longDuration = 10_000;
      const longMaxPayment = maxBytes * BigInt(longDuration) * 10n;
      const bucketId = await createBucket(api, bob);
      await requestPrimaryAgreement(api, bob, provider, bucketId, {
        max_bytes: maxBytes,
        duration: longDuration,
        max_payment: longMaxPayment,
      });
      const balAfterRequest = await getFree(api, bob);
      await withdrawAgreementRequest(api, bob, bucketId, provider);
      const balAfterWithdraw = await getFree(api, bob);
      assert.ok(
        balAfterWithdraw > balAfterRequest,
        `Balance should increase after withdraw: ${balAfterWithdraw} > ${balAfterRequest}`
      );
    },
  });

  // ── Capacity Tracking ─────────────────────────────────────────────────────

  tests.push({
    name: "10.3 committed_bytes increments on accept",
    fn: async () => {
      const infoBefore = await api.query.StorageProvider.Providers.getValue(provider.address);
      const beforeBytes = infoBefore.committed_bytes;
      const bucketId = await createBucket(api, bob);
      await requestPrimaryAgreement(api, bob, provider, bucketId, {
        max_bytes: maxBytes,
        duration: 100,
        max_payment: maxBytes * 100n * 10n,
      });
      await waitForAgreementAcceptance(api, provider.address, bucketId);
      const infoAfter = await api.query.StorageProvider.Providers.getValue(provider.address);
      assert.ok(
        infoAfter.committed_bytes > beforeBytes,
        `committed_bytes should increase: ${infoAfter.committed_bytes} > ${beforeBytes}`
      );
    },
  });

  tests.push({
    name: "10.4 committed_bytes decrements on end",
    fn: async () => {
      const bucketId = await createBucket(api, bob);
      await requestPrimaryAgreement(api, bob, provider, bucketId, {
        max_bytes: maxBytes,
        duration: 10,
        max_payment: maxBytes * 10n * 10n,
      });
      await waitForAgreementAcceptance(api, provider.address, bucketId);
      const infoBefore = await api.query.StorageProvider.Providers.getValue(provider.address);
      const agreement = await api.query.StorageProvider.StorageAgreements.getValue(
        bucketId,
        provider.address
      );
      await waitForBlock(papi, Number(agreement.expires_at));
      await endAgreement(api, bob, provider, bucketId, "Pay");
      const infoAfter = await api.query.StorageProvider.Providers.getValue(provider.address);
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
      const { bucketId } = await createBucketWithStorage(api, bob, {
        max_bytes: maxBytes,
        duration: 100,
        max_price_per_byte: 10n,
      });
      await waitForAgreementAcceptance(api, provider.address, bucketId);
      // freeze_bucket requires a snapshot (checkpoint) to exist.
      await uploadChunk(PROVIDER_URL, bucketId, "data for snapshot");
      const ck = await fetchCheckpointSignature(PROVIDER_URL, bucketId);
      await submitClientCheckpoint(api, bob, provider, bucketId, ck);
      await freezeBucket(api, bob, bucketId);
      const bucket = await api.query.StorageProvider.Buckets.getValue(bucketId);
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
      const { bucketId } = await createBucketWithStorage(api, bob, {
        max_bytes: maxBytes,
        duration: 100,
        max_price_per_byte: 10n,
      });
      await waitForAgreementAcceptance(api, provider.address, bucketId);
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
      const bucket1 = await createBucket(api, dave);
      const bucket2 = await createBucket(api, dave);
      await setMember(api, dave, bucket1, eve, "Writer");
      await setMember(api, dave, bucket2, eve, "Reader");
      const eveBuckets = await api.query.StorageProvider.MemberBuckets.getValue(eve.address);
      assert.ok(eveBuckets.some((id) => id === bucket1), "Eve should be member of bucket1");
      assert.ok(eveBuckets.some((id) => id === bucket2), "Eve should be member of bucket2");
    },
  });

  // ── Data Integrity ────────────────────────────────────────────────────────

  tests.push({
    name: "10.8 Upload verify blake2-256",
    fn: async () => {
      const { bucketId } = await createBucketWithStorage(api, bob, {
        max_bytes: maxBytes,
        duration: 100,
        max_price_per_byte: 10n,
      });
      await waitForAgreementAcceptance(api, provider.address, bucketId);
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
      const { bucketId } = await createBucketWithStorage(api, bob, {
        max_bytes: maxBytes,
        duration: 100,
        max_price_per_byte: 10n,
      });
      await waitForAgreementAcceptance(api, provider.address, bucketId);
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
