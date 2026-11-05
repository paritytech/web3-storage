// SPDX-License-Identifier: Apache-2.0

/**
 * E2E Workflow 15 — Two Primary Providers
 *
 * Accounts: //Alice (provider node 1), //Charlie (provider node 2),
 * //Bob (client)
 *
 * Needs two provider nodes: //Alice at `provider_url` and //Charlie at
 * `second_provider_url` (`just start-second-provider`). Primaries do not sync
 * with each other, so the client uploads to each one and collects both
 * signatures for the checkpoint. Tests cover:
 * - 15.1-15.4: a bucket with two primaries, and a stale primary signature;
 * - 15.5-15.8 and 15.13-15.14: a provider switch. The client moves the data
 *   from the old primary to a new one, the new primary signs the snapshot with
 *   `extend_checkpoint`, the client checkpoints without the old primary, and
 *   ends the old agreement after it expires;
 * - 15.9-15.12: an admin ending a primary agreement early on a
 *   `min_providers = 2` bucket. These tests run while the switch scenario
 *   waits for the old agreement to expire.
 *
 * Usage: node e2e/15-multi-primary-providers.js [chain_ws] [provider_url] [second_provider_url]
 */

import assert from "node:assert";
import {
  addPrimaryProvider,
  asHex,
  bytesEq,
  challengeCheckpoint,
  checkpointTx,
  createBucket,
  currentRelayBlock,
  decodeMultiSignature,
  downloadChunk,
  endAgreement,
  ensureProviderRegistered,
  fetchChallengeProof,
  getProviderNodeInfo,
  makeSigner,
  READ_OPTS,
  respondToChallenge,
  sameAddress,
  setMinProviders,
  submitClientCheckpoint,
  submitTx,
  updateProviderMultiaddr,
  uploadChunk,
  waitForRelayBlock,
  type CommitResponse,
  type ParachainApi,
} from "@web3-storage/sdk";
import {
  negotiateAndEstablish,
  negotiateSigned,
  runSuite,
  setupChain,
  submitTxExpectFailure,
} from "./helpers.js";

const CHAIN_WS = process.argv[2] || "ws://127.0.0.1:2222";
const PROVIDER_URL = process.argv[3] || "http://127.0.0.1:3333";
const SECOND_PROVIDER_URL = process.argv[4] || "http://127.0.0.1:3334";

type Upload = Awaited<ReturnType<typeof uploadChunk>>;

/** The snapshot's signer bits, one entry per index of `primary_providers`. */
async function snapshotSigners(api: ParachainApi, bucketId: bigint): Promise<number[]> {
  const bucket = (await api.query.StorageProvider.Buckets.getValue(bucketId, READ_OPTS))!;
  assert.ok(bucket.snapshot, "bucket should have a snapshot");
  return Array.from(bucket.snapshot.primary_signers);
}

async function main() {
  const first = makeSigner("//Alice");
  const second = makeSigner("//Charlie");
  const client = makeSigner("//Bob");

  const { papi, api } = await setupChain(CHAIN_WS);
  try {
    await getProviderNodeInfo(SECOND_PROVIDER_URL);
  } catch (err) {
    throw new Error(
      `Second provider is not reachable at ${SECOND_PROVIDER_URL} ` +
        `(${(err as Error).message}). Start it with 'just start-second-provider'.`
    );
  }
  await ensureProviderRegistered(api, first, PROVIDER_URL);
  await ensureProviderRegistered(api, second, SECOND_PROVIDER_URL);
  const secondAddr = `/ip4/127.0.0.1/tcp/${new URL(SECOND_PROVIDER_URL).port}`;
  const secondInfo = await api.query.StorageProvider.Providers.getValue(second.address, READ_OPTS);
  if (new TextDecoder().decode(secondInfo!.multiaddr) !== secondAddr) {
    await updateProviderMultiaddr(api, second, secondAddr);
  }

  const maxBytes = 1_048_576n;
  const duration = 200;

  /** Creates a `min_providers = 2` bucket with //Alice and //Charlie as primaries. */
  async function bucketWithTwoPrimaries(): Promise<bigint> {
    const { bucketId } = await createBucket(api, client, { minProviders: 2 });
    for (const [provider, url] of [
      [first, PROVIDER_URL],
      [second, SECOND_PROVIDER_URL],
    ] as const) {
      const signed = await negotiateSigned(api, url, client, provider, {
        maxBytes,
        duration,
        bucketId,
      });
      await addPrimaryProvider(api, client, provider, signed, { mode: "finalized" });
    }
    return bucketId;
  }

  /** Uploads `payload` to both nodes. */
  async function uploadToBoth(bucketId: bigint, payload: string) {
    const [a, b] = await Promise.all([
      uploadChunk(PROVIDER_URL, bucketId, payload, client),
      uploadChunk(SECOND_PROVIDER_URL, bucketId, payload, client),
    ]);
    return { first: a, second: b };
  }

  /** //Charlie answers a `challenge_checkpoint` on `leafIndex` from its own node. */
  async function secondDefendsChallenge(bucketId: bigint, leafIndex: number) {
    const challengeId = await challengeCheckpoint(api, client, second, bucketId, leafIndex);
    const proof = await fetchChallengeProof(api, SECOND_PROVIDER_URL, challengeId, second);
    const result = await respondToChallenge(api, second, challengeId, proof);
    const events = api.event.StorageProvider.ChallengeDefended.filter(result.events as never);
    assert.strictEqual(events.length, 1, "Expected ChallengeDefended event");
  }

  async function getBucket(bucketId: bigint) {
    return (await api.query.StorageProvider.Buckets.getValue(bucketId, READ_OPTS))!;
  }

  async function getAgreement(bucketId: bigint, provider: { address: string }) {
    return (await api.query.StorageProvider.StorageAgreements.getValue(
      bucketId,
      provider.address,
      READ_OPTS
    ))!;
  }

  // Scenario state. Each test checks that the tests it depends on passed.
  let two: { bucketId: bigint; first: Upload; second: Upload } | undefined;
  let moved: bigint | undefined;
  let old: Upload | undefined;
  let next: Upload | undefined;
  let leaving: { bucketId: bigint; first: CommitResponse; second: CommitResponse } | undefined;
  let afterLeave: Upload | undefined;

  const tests: Array<{ name: string; fn: () => Promise<void> }> = [];

  // ── Two primaries ─────────────────────────────────────────────────────────

  tests.push({
    name: "15.1 Same upload to both providers gives the same commitment",
    fn: async () => {
      const bucketId = await bucketWithTwoPrimaries();
      const uploads = await uploadToBoth(bucketId, `two-primaries @ ${Date.now()}`);
      for (const field of ["mmr_root", "start_seq", "leaf_count"] as const) {
        assert.strictEqual(
          String(uploads.second.commit[field]),
          String(uploads.first.commit[field]),
          `${field} should match across providers`
        );
      }
      two = { bucketId, ...uploads };
    },
  });

  tests.push({
    name: "15.2 Checkpoint signed by both providers",
    fn: async () => {
      assert.ok(two, "15.1 must pass first");
      const result = await submitClientCheckpoint(
        api,
        client,
        two.bucketId,
        [
          { provider: first, commit: two.first.commit },
          { provider: second, commit: two.second.commit },
        ],
        { label: "checkpoint (2 signers)" }
      );
      const events = api.event.StorageProvider.BucketCheckpointed.filter(result.events as never);
      assert.strictEqual(events.length, 1, "Expected BucketCheckpointed event");
      assert.deepStrictEqual(
        await snapshotSigners(api, two.bucketId),
        [1, 1],
        "both primaries should be in the snapshot bitfield"
      );
    },
  });

  tests.push({
    name: "15.3 Second primary defends a challenge from its own node",
    fn: async () => {
      assert.ok(two, "15.1 must pass first");
      await secondDefendsChallenge(two.bucketId, two.second.commit.leaf_indices[0]);
    },
  });

  tests.push({
    name: "15.4 Checkpoint with a stale primary signature fails",
    fn: async () => {
      assert.ok(two, "15.1 must pass first");
      const stale = two.second.commit;
      // New data on //Alice only: //Charlie's signature now covers an older
      // commitment than //Alice's.
      const fresh = (await uploadChunk(PROVIDER_URL, two.bucketId, `stale @ ${Date.now()}`, client))
        .commit;
      assert.throws(
        () =>
          checkpointTx(api, two!.bucketId, [
            { provider: first, commit: fresh },
            { provider: second, commit: stale },
          ]),
        /signed a different commitment/,
        "the SDK should reject commits that differ"
      );
      // Build the tx directly so the chain checks the signature.
      const tx = api.tx.StorageProvider.checkpoint({
        bucket_id: two.bucketId,
        commitment: {
          mmr_root: asHex(fresh.mmr_root),
          start_seq: BigInt(fresh.start_seq),
          leaf_count: BigInt(fresh.leaf_count),
        },
        signatures: [
          [first.address, decodeMultiSignature(fresh.provider_signature)],
          [second.address, decodeMultiSignature(stale.provider_signature)],
        ],
      });
      await submitTxExpectFailure(tx, client.signer, "InvalidSignature", "15.4");
    },
  });

  // ── Provider switch, part 1 ───────────────────────────────────────────────

  tests.push({
    name: "15.5 Add a new primary while the old agreement is active",
    fn: async () => {
      const { bucketId } = await negotiateAndEstablish(
        api,
        PROVIDER_URL,
        client,
        first,
        { maxBytes, duration: 30 },
        true,
      );
      const upload = await uploadChunk(PROVIDER_URL, bucketId, `switch @ ${Date.now()}`, client);
      await submitClientCheckpoint(api, client, bucketId, [
        { provider: first, commit: upload.commit },
      ], { label: "checkpoint (old primary)" });
      assert.deepStrictEqual(await snapshotSigners(api, bucketId), [1]);
      const agreement = await getAgreement(bucketId, first);
      assert.ok(
        (await currentRelayBlock(api)) < Number(agreement.expires_at),
        "old agreement should not have expired yet"
      );
      const signed = await negotiateSigned(api, SECOND_PROVIDER_URL, client, second, {
        maxBytes,
        duration,
        bucketId,
      });
      await addPrimaryProvider(api, client, second, signed, { mode: "finalized" });
      const bucket = await getBucket(bucketId);
      assert.strictEqual(bucket.primary_providers.length, 2, "bucket should have two primaries");
      assert.deepStrictEqual(
        await snapshotSigners(api, bucketId),
        [1, 0],
        "adding a primary should append an unset bit"
      );
      moved = bucketId;
      old = upload;
    },
  });

  tests.push({
    name: "15.6 Move the data; the new primary signs the snapshot with extend_checkpoint",
    fn: async () => {
      assert.ok(moved !== undefined && old, "15.5 must pass first");
      const bytes = await downloadChunk(PROVIDER_URL, moved, old.hash);
      const upload = await uploadChunk(SECOND_PROVIDER_URL, moved, bytes, client);
      for (const field of ["mmr_root", "start_seq", "leaf_count"] as const) {
        assert.strictEqual(
          String(upload.commit[field]),
          String(old.commit[field]),
          `new primary should reach the same ${field}`
        );
      }
      assert.ok(
        bytesEq(await downloadChunk(SECOND_PROVIDER_URL, moved, old.hash), bytes),
        "new primary should serve the moved data"
      );
      await submitTx(
        api.tx.StorageProvider.extend_checkpoint({
          bucket_id: moved,
          additional_signatures: [
            [second.address, decodeMultiSignature(upload.commit.provider_signature)],
          ],
        }),
        client.signer,
        { label: "extend_checkpoint (new primary)" }
      );
      assert.deepStrictEqual(
        await snapshotSigners(api, moved),
        [1, 1],
        "both primaries should be in the snapshot bitfield"
      );
      next = upload;
    },
  });

  tests.push({
    name: "15.7 Checkpoint with the new primary only",
    fn: async () => {
      assert.ok(moved !== undefined && next, "15.6 must pass first");
      await submitClientCheckpoint(api, client, moved, [
        { provider: second, commit: next.commit },
      ], { label: "checkpoint (new primary)" });
      assert.deepStrictEqual(
        await snapshotSigners(api, moved),
        [0, 1],
        "only the new primary should be in the snapshot bitfield"
      );
    },
  });

  tests.push({
    name: "15.8 Old primary is not challengeable against the new snapshot",
    fn: async () => {
      assert.ok(moved !== undefined && old, "15.5 must pass first");
      assert.deepStrictEqual(await snapshotSigners(api, moved), [0, 1], "15.7 must pass first");
      const tx = api.tx.StorageProvider.challenge_checkpoint({
        bucket_id: moved,
        provider: first.address,
        target: { leaf_index: BigInt(old.commit.leaf_indices[0]), chunk_index: 0n },
      });
      await submitTxExpectFailure(tx, client.signer, "ProviderNotInSnapshot", "15.8");
    },
  });

  // ── Admin ends a primary early (runs during the 15.13 expiry wait) ────────

  tests.push({
    name: "15.9 Admin ends a primary agreement before expiry",
    fn: async () => {
      const bucketId = await bucketWithTwoPrimaries();
      const uploads = await uploadToBoth(bucketId, `admin-ends @ ${Date.now()}`);
      await submitClientCheckpoint(api, client, bucketId, [
        { provider: first, commit: uploads.first.commit },
        { provider: second, commit: uploads.second.commit },
      ], { label: "checkpoint (2 signers)" });
      const agreement = await getAgreement(bucketId, first);
      assert.ok(
        (await currentRelayBlock(api)) < Number(agreement.expires_at),
        "agreement should not have expired yet"
      );
      const result = await endAgreement(api, client, first, bucketId, "Pay");
      const events = api.event.StorageProvider.PrimaryProviderRemoved.filter(
        result.events as never
      );
      assert.strictEqual(events.length, 1, "Expected PrimaryProviderRemoved event");
      assert.strictEqual(
        events[0].payload.reason.type,
        "AdminTerminated",
        "removal should take the early termination path"
      );
      const bucket = await getBucket(bucketId);
      assert.strictEqual(bucket.primary_providers.length, 1, "bucket should have one primary");
      assert.ok(sameAddress(bucket.primary_providers[0], second.address), "second primary remains");
      assert.strictEqual(bucket.min_providers, 2, "min_providers should not change");
      assert.deepStrictEqual(
        await snapshotSigners(api, bucketId),
        [1],
        "the remaining primary's bit should move to index 0"
      );
      leaving = { bucketId, first: uploads.first.commit, second: uploads.second.commit };
    },
  });

  tests.push({
    name: "15.10 Checkpoint with a removed primary's signature fails",
    fn: async () => {
      assert.ok(leaving, "15.9 must pass first");
      const tx = checkpointTx(api, leaving.bucketId, [
        { provider: first, commit: leaving.first },
        { provider: second, commit: leaving.second },
      ]);
      await submitTxExpectFailure(tx, client.signer, "ProviderNotInSnapshot", "15.10");
    },
  });

  tests.push({
    name: "15.11 Checkpoint fails while fewer primaries remain than min_providers",
    fn: async () => {
      assert.ok(leaving, "15.9 must pass first");
      const upload = await uploadChunk(
        SECOND_PROVIDER_URL,
        leaving.bucketId,
        `after-leave @ ${Date.now()}`,
        client
      );
      const tx = checkpointTx(api, leaving.bucketId, [{ provider: second, commit: upload.commit }]);
      await submitTxExpectFailure(tx, client.signer, "InsufficientSignatures", "15.11");
      afterLeave = upload;
    },
  });

  tests.push({
    name: "15.12 Checkpoint succeeds after set_min_providers(1)",
    fn: async () => {
      assert.ok(leaving && afterLeave, "15.11 must pass first");
      await setMinProviders(api, client, leaving.bucketId, 1);
      const result = await submitClientCheckpoint(api, client, leaving.bucketId, [
        { provider: second, commit: afterLeave.commit },
      ], { label: "checkpoint (remaining primary)" });
      const events = api.event.StorageProvider.BucketCheckpointed.filter(result.events as never);
      assert.strictEqual(events.length, 1, "Expected BucketCheckpointed event");
      assert.deepStrictEqual(await snapshotSigners(api, leaving.bucketId), [1]);
    },
  });

  // ── Provider switch, part 2 ───────────────────────────────────────────────

  tests.push({
    name: "15.13 End the old agreement after expiry",
    fn: async () => {
      assert.ok(moved !== undefined, "15.5 must pass first");
      assert.deepStrictEqual(await snapshotSigners(api, moved), [0, 1], "15.7 must pass first");
      const agreement = await getAgreement(moved, first);
      console.log("    Waiting for expiry at block %d...", Number(agreement.expires_at));
      await waitForRelayBlock(papi, api, Number(agreement.expires_at));
      const result = await endAgreement(api, client, first, moved, "Pay");
      const events = api.event.StorageProvider.PrimaryProviderRemoved.filter(
        result.events as never
      );
      assert.strictEqual(events.length, 1, "Expected PrimaryProviderRemoved event");
      assert.strictEqual(events[0].payload.reason.type, "Expired", "removal should take the expiry path");
      const bucket = await getBucket(moved);
      assert.strictEqual(bucket.primary_providers.length, 1, "bucket should have one primary");
      assert.ok(
        sameAddress(bucket.primary_providers[0], second.address),
        "new primary should be at index 0"
      );
      assert.deepStrictEqual(
        await snapshotSigners(api, moved),
        [1],
        "snapshot bit should move to the new primary's index"
      );
    },
  });

  tests.push({
    name: "15.14 Remaining primary defends a challenge",
    fn: async () => {
      assert.ok(moved !== undefined && next, "15.6 must pass first");
      assert.deepStrictEqual(await snapshotSigners(api, moved), [1], "15.13 must pass first");
      await secondDefendsChallenge(moved, next.commit.leaf_indices[0]);
    },
  });

  await runSuite("15 — Two Primary Providers", tests, { api, papi });
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
