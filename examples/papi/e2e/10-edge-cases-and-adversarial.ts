// SPDX-License-Identifier: Apache-2.0

/**
 * E2E Workflow 10 — Edge Cases & Adversarial
 *
 * Tests: balance accounting, capacity tracking, frozen buckets,
 * concurrent operations, data integrity, and access-control rejections
 * (non-admin writes, freeze without a checkpoint, unsigned/non-member
 * provider uploads).
 *
 * Usage: node e2e/10-edge-cases-and-adversarial.js [chain_ws] [provider_url]
 */

import assert from "node:assert";
import { Enum } from "polkadot-api";
import { blake2b256 } from "@polkadot-labs/hdkd-helpers";
import {
  endAgreement,
  currentRelayBlock,
  ensureProviderRegistered,
  fetchCheckpointSignature,
  freezeBucket,
  makeSigner,
  READ_OPTS,
  setMember,
  signProviderRequest,
  submitClientCheckpoint,
  toHex,
  uploadChunk,
  waitForRelayBlock,
} from "@web3-storage/sdk";
import { ensureSoleAcceptingProvider } from "../support.js";
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

  const tests: Array<{ name: string; fn: () => Promise<void> }> = [];

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
      const infoBefore = (await api.query.StorageProvider.Providers.getValue(
        provider.address,
        READ_OPTS
      ))!;
      const beforeBytes = infoBefore.committed_bytes;
      await negotiateAndEstablish(api, PROVIDER_URL, bob, provider, { maxBytes, duration: 100 });
      const infoAfter = (await api.query.StorageProvider.Providers.getValue(
        provider.address,
        READ_OPTS
      ))!;
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
      const infoBefore = (await api.query.StorageProvider.Providers.getValue(
        provider.address,
        READ_OPTS
      ))!;
      const agreement = (await api.query.StorageProvider.StorageAgreements.getValue(
        bucketId,
        provider.address,
        READ_OPTS
      ))!;
      await waitForRelayBlock(papi, api, Number(agreement.expires_at));
      await endAgreement(api, bob, provider, bucketId, "Pay");
      const infoAfter = (await api.query.StorageProvider.Providers.getValue(
        provider.address,
        READ_OPTS
      ))!;
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
      const { bucketId } = await negotiateAndEstablish(
        api,
        PROVIDER_URL,
        bob,
        provider,
        { maxBytes, duration: 100 },
        true, // finalize: immediate upload reads finalized membership
      );
      // freeze_bucket requires a snapshot (checkpoint) to exist.
      const nonce = await currentRelayBlock(api);
      await uploadChunk(PROVIDER_URL, bucketId, "data for snapshot", nonce, bob);
      const ck = await fetchCheckpointSignature(PROVIDER_URL, bucketId, nonce);
      await submitClientCheckpoint(api, bob, provider, bucketId, ck);
      await freezeBucket(api, bob, bucketId);
      const bucket = (await api.query.StorageProvider.Buckets.getValue(bucketId, READ_OPTS))!;
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
      const { bucketId } = await negotiateAndEstablish(
        api,
        PROVIDER_URL,
        bob,
        provider,
        { maxBytes, duration: 100 },
        true, // finalize: immediate upload reads finalized membership
      );
      // Upload some data.
      const nonce1 = await currentRelayBlock(api);
      await uploadChunk(PROVIDER_URL, bucketId, "pre-freeze data", nonce1, bob);
      // Checkpoint before freeze.
      const ck1 = await fetchCheckpointSignature(PROVIDER_URL, bucketId, nonce1);
      await submitClientCheckpoint(api, bob, provider, bucketId, ck1);
      // Freeze.
      await freezeBucket(api, bob, bucketId);
      // Upload more data.
      const nonce2 = await currentRelayBlock(api);
      await uploadChunk(PROVIDER_URL, bucketId, "post-freeze data", nonce2, bob);
      // Checkpoint after freeze — should still work (captures frozen_start_seq).
      const ck2 = await fetchCheckpointSignature(PROVIDER_URL, bucketId, nonce2);
      const result = await submitClientCheckpoint(api, bob, provider, bucketId, ck2);
      const events = api.event.StorageProvider.BucketCheckpointed.filter(result.events as never);
      assert.strictEqual(events.length, 1, "Checkpoint after freeze should emit event");
    },
  });

  // ── Concurrent Operations ─────────────────────────────────────────────────

  tests.push({
    name: "10.7 Same account member of multiple buckets",
    fn: async () => {
      const { bucketId: bucket1 } = await negotiateAndEstablish(api, PROVIDER_URL, dave, provider, {
        maxBytes,
        duration: 100,
      });
      const { bucketId: bucket2 } = await negotiateAndEstablish(api, PROVIDER_URL, dave, provider, {
        maxBytes,
        duration: 100,
      });
      await setMember(api, dave, bucket1, eve, "Writer");
      await setMember(api, dave, bucket2, eve, "Reader");
      const eveBuckets = await api.query.StorageProvider.MemberBuckets.getValue(
        eve.address,
        READ_OPTS
      );
      assert.ok(
        eveBuckets.some((id: bigint) => id === bucket1),
        "Eve should be member of bucket1"
      );
      assert.ok(
        eveBuckets.some((id: bigint) => id === bucket2),
        "Eve should be member of bucket2"
      );
    },
  });

  // ── Data Integrity ────────────────────────────────────────────────────────

  tests.push({
    name: "10.8 Upload verify blake2-256",
    fn: async () => {
      const { bucketId } = await negotiateAndEstablish(
        api,
        PROVIDER_URL,
        bob,
        provider,
        { maxBytes, duration: 100 },
        true, // finalize: immediate upload reads finalized membership
      );
      const data = "integrity check data for blake2-256";
      const bytes = new TextEncoder().encode(data);
      const expectedHash = toHex(blake2b256(bytes));
      const nonce = await currentRelayBlock(api);
      const { hash } = await uploadChunk(PROVIDER_URL, bucketId, data, nonce, bob);
      assert.strictEqual(hash, expectedHash, "Provider hash should match local blake2-256");
    },
  });

  tests.push({
    name: "10.9 Identical content → same hash, different MMR leaves",
    fn: async () => {
      const { bucketId } = await negotiateAndEstablish(
        api,
        PROVIDER_URL,
        bob,
        provider,
        { maxBytes, duration: 100 },
        true, // finalize: immediate upload reads finalized membership
      );
      const data = "identical content for dedup test";
      const nonce = await currentRelayBlock(api);
      const r1 = await uploadChunk(PROVIDER_URL, bucketId, data, nonce, bob);
      const r2 = await uploadChunk(PROVIDER_URL, bucketId, data, nonce, bob);
      assert.strictEqual(r1.hash, r2.hash, "Hashes should match for identical content");
      assert.notStrictEqual(
        r1.commit.leaf_indices[0],
        r2.commit.leaf_indices[0],
        "Leaf indices should differ"
      );
    },
  });

  // ── Access Control (adversarial — must be rejected) ───────────────────────

  tests.push({
    name: "10.10 Non-admin cannot add bucket members",
    fn: async () => {
      // Bob owns (and is sole admin of) the bucket; Eve is not even a member.
      const { bucketId } = await negotiateAndEstablish(api, PROVIDER_URL, bob, provider, {
        maxBytes,
        duration: 100,
      });
      const tx = api.tx.StorageProvider.set_member({
        bucket_id: bucketId,
        member: dave.address,
        role: Enum("Reader"),
      });
      // ensure_admin rejects a non-admin caller before any role logic runs.
      await submitTxExpectFailure(tx, eve.signer, "NotBucketAdmin", "10.10");
    },
  });

  tests.push({
    name: "10.11 Non-admin cannot freeze a bucket",
    fn: async () => {
      const { bucketId } = await negotiateAndEstablish(api, PROVIDER_URL, bob, provider, {
        maxBytes,
        duration: 100,
      });
      const tx = api.tx.StorageProvider.freeze_bucket({ bucket_id: bucketId });
      await submitTxExpectFailure(tx, eve.signer, "NotBucketAdmin", "10.11");
    },
  });

  tests.push({
    name: "10.12 Freezing without a checkpoint fails",
    fn: async () => {
      // Fresh bucket: no checkpoint submitted, so no snapshot exists yet.
      const { bucketId } = await negotiateAndEstablish(api, PROVIDER_URL, bob, provider, {
        maxBytes,
        duration: 100,
      });
      // Bob is the admin (passes ensure_admin); freeze then trips NoSnapshot.
      const tx = api.tx.StorageProvider.freeze_bucket({ bucket_id: bucketId });
      await submitTxExpectFailure(tx, bob.signer, "NoSnapshot", "10.12");
    },
  });

  tests.push({
    name: "10.13 Provider rejects unsigned and non-member uploads",
    fn: async () => {
      const { bucketId } = await negotiateAndEstablish(
        api,
        PROVIDER_URL,
        bob,
        provider,
        { maxBytes, duration: 100 },
        true, // finalize: the provider resolves membership from finalized state
      );
      const bytes = new TextEncoder().encode("must not land");
      const body = JSON.stringify({
        bucket_id: Number(bucketId),
        hash: toHex(blake2b256(bytes)),
        data: Buffer.from(bytes).toString("base64"),
        children: null,
      });

      // No Authorization header at all → 401.
      const unsigned = await fetch(`${PROVIDER_URL}/node`, {
        method: "PUT",
        headers: { "Content-Type": "application/json" },
        body,
      });
      assert.strictEqual(unsigned.status, 401, "unsigned upload must be rejected with 401");

      // Valid signature from Eve, who holds no role on the bucket → 403.
      const eveSigned = await fetch(`${PROVIDER_URL}/node`, {
        method: "PUT",
        headers: {
          "Content-Type": "application/json",
          ...(await signProviderRequest(eve.signer, "PUT", bucketId)),
        },
        body,
      });
      assert.strictEqual(eveSigned.status, 403, "non-member upload must be rejected with 403");
    },
  });

  await runSuite("10 — Edge Cases & Adversarial", tests, { api, papi });

  try {
    await restore();
  } catch {}
  papi.destroy();
}

main()
  .catch((err) => {
    console.error(err);
    process.exitCode = 1;
  })
  .finally(() => {
    process.exit(process.exitCode || 0);
  });
