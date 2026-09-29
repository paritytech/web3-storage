// SPDX-License-Identifier: Apache-2.0

/**
 * E2E Workflow 13 — Two Primary Providers
 *
 * Accounts: //Alice (provider node 1), //Charlie (provider node 2),
 * //Bob (client)
 *
 * Needs two provider nodes: //Alice at `provider_url` and //Charlie at
 * `second_provider_url` (`just start-second-provider`). Primaries do not sync
 * with each other, so the client uploads to each one and collects both
 * signatures for the checkpoint. Tests cover a bucket with two primaries and
 * a provider switch, where the client moves the data from the old primary to
 * a new one and checkpoints without the old one.
 *
 * Usage: node e2e/13-multi-primary-providers.js [chain_ws] [provider_url] [second_provider_url]
 */

import assert from "node:assert";
import {
  addPrimaryProvider,
  asHex,
  buildSignedTermsArgs,
  bytesEq,
  challengeCheckpoint,
  createBucket,
  decodeMultiSignature,
  downloadChunk,
  ensureProviderRegistered,
  fetchChallengeProof,
  makeSigner,
  READ_OPTS,
  respondToChallenge,
  sameAddress,
  submitTx,
  updateProviderMultiaddr,
  uploadChunk,
  type ChainSigner,
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

type Commit = {
  mmr_root: string;
  start_seq: number | string;
  leaf_count: number | string;
  provider_signature: string;
};

/** Build a `checkpoint` call carrying one signature per provider. */
function checkpointTx(
  api: ParachainApi,
  bucketId: bigint,
  commit: Commit,
  signed: Array<{ provider: ChainSigner; commit: Commit }>,
) {
  return api.tx.StorageProvider.checkpoint({
    bucket_id: bucketId,
    commitment: {
      mmr_root: asHex(commit.mmr_root),
      start_seq: BigInt(commit.start_seq),
      leaf_count: BigInt(commit.leaf_count),
    },
    signatures: signed.map(({ provider, commit }) => [
      provider.address,
      decodeMultiSignature(commit.provider_signature),
    ]),
  });
}

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
  await ensureProviderRegistered(api, first, PROVIDER_URL);
  await ensureProviderRegistered(api, second, SECOND_PROVIDER_URL);
  // Workflow 01 registers //Charlie with the first node's address, and
  // registration is skipped when it exists. Point it at the second node.
  await updateProviderMultiaddr(
    api,
    second,
    `/ip4/127.0.0.1/tcp/${new URL(SECOND_PROVIDER_URL).port}`
  );

  const maxBytes = 1_048_576n; // 1 MiB
  const duration = 200;

  // A bucket that needs both signatures on every checkpoint. Both primaries
  // are added with finalized transactions: a provider node reads bucket
  // membership from finalized state, and the uploads below follow at once.
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

  // The client uploads the same data to each primary.
  const payload = `two-primaries @ ${Date.now()}`;
  const firstUpload = await uploadChunk(PROVIDER_URL, bucketId, payload, client);
  const secondUpload = await uploadChunk(SECOND_PROVIDER_URL, bucketId, payload, client);

  // Provider switch: a bucket on the first provider only, with data and a
  // checkpoint. 13.7–13.9 move it to the second provider.
  const { bucketId: moved } = await negotiateAndEstablish(
    api,
    PROVIDER_URL,
    client,
    first,
    { maxBytes, duration },
    true,
  );
  const old = await uploadChunk(PROVIDER_URL, moved, `switch-provider @ ${Date.now()}`, client);
  await submitTx(
    checkpointTx(api, moved, old.commit, [{ provider: first, commit: old.commit }]),
    client.signer,
    { label: "checkpoint (old primary)" }
  );

  const tests: Array<{ name: string; fn: () => Promise<void> }> = [];

  // ── Two primaries on one bucket ─────────────────────────────────────────

  tests.push({
    name: "13.1 Both providers are primaries of one bucket",
    fn: async () => {
      const bucket = (await api.query.StorageProvider.Buckets.getValue(bucketId, READ_OPTS))!;
      assert.strictEqual(bucket.primary_providers.length, 2, "bucket should have two primaries");
      // Order matters: bit i of the snapshot bitfield is primary_providers[i].
      assert.ok(sameAddress(bucket.primary_providers[0], first.address), "first primary at index 0");
      assert.ok(
        sameAddress(bucket.primary_providers[1], second.address),
        "second primary at index 1"
      );
      for (const provider of [first, second]) {
        const agreement = await api.query.StorageProvider.StorageAgreements.getValue(
          bucketId,
          provider.address,
          READ_OPTS
        );
        assert.ok(agreement, `agreement should exist for ${provider.address}`);
      }
    },
  });

  tests.push({
    name: "13.2 Same upload to both providers gives the same commitment",
    fn: async () => {
      for (const field of ["mmr_root", "start_seq", "leaf_count"] as const) {
        assert.strictEqual(
          String(secondUpload.commit[field]),
          String(firstUpload.commit[field]),
          `${field} should match across providers`
        );
      }
    },
  });

  tests.push({
    name: "13.3 Checkpoint with one of two required signatures is rejected",
    fn: async () => {
      const tx = checkpointTx(api, bucketId, firstUpload.commit, [
        { provider: first, commit: firstUpload.commit },
      ]);
      await submitTxExpectFailure(tx, client.signer, "InsufficientSignatures", "13.3");
    },
  });

  tests.push({
    name: "13.4 Checkpoint signed by both providers",
    fn: async () => {
      const tx = checkpointTx(api, bucketId, firstUpload.commit, [
        { provider: first, commit: firstUpload.commit },
        { provider: second, commit: secondUpload.commit },
      ]);
      const result = await submitTx(tx, client.signer, { label: "checkpoint (2 signers)" });
      const events = api.event.StorageProvider.BucketCheckpointed.filter(result.events as never);
      assert.strictEqual(events.length, 1, "Expected BucketCheckpointed event");
      assert.deepStrictEqual(
        await snapshotSigners(api, bucketId),
        [0b11],
        "both primaries should be in the snapshot bitfield"
      );
    },
  });

  tests.push({
    name: "13.5 Second primary defends a challenge from its own node",
    fn: async () => {
      const challengeId = await challengeCheckpoint(
        api,
        client,
        second,
        bucketId,
        secondUpload.commit.leaf_indices[0]
      );
      const proof = await fetchChallengeProof(api, SECOND_PROVIDER_URL, challengeId);
      const result = await respondToChallenge(api, second, challengeId, proof);
      const events = api.event.StorageProvider.ChallengeDefended.filter(result.events as never);
      assert.strictEqual(events.length, 1, "Expected ChallengeDefended event");
    },
  });

  tests.push({
    name: "13.6 add_primary_provider rejects a provider that is already a primary",
    fn: async () => {
      const signed = await negotiateSigned(api, PROVIDER_URL, client, first, {
        maxBytes,
        duration,
        bucketId,
      });
      const tx = api.tx.StorageProvider.add_primary_provider({
        bucket_id: bucketId,
        ...buildSignedTermsArgs(first, signed),
      });
      await submitTxExpectFailure(tx, client.signer, "AgreementAlreadyExists", "13.6");
    },
  });

  // ── Provider switch ─────────────────────────────────────────────────────

  tests.push({
    name: "13.7 Add a new primary while the old agreement is active",
    fn: async () => {
      assert.deepStrictEqual(await snapshotSigners(api, moved), [0b01]);
      const signed = await negotiateSigned(api, SECOND_PROVIDER_URL, client, second, {
        maxBytes,
        duration,
        bucketId: moved,
      });
      await addPrimaryProvider(api, client, second, signed, { mode: "finalized" });
      const bucket = (await api.query.StorageProvider.Buckets.getValue(moved, READ_OPTS))!;
      assert.strictEqual(bucket.primary_providers.length, 2, "bucket should have two primaries");
    },
  });

  tests.push({
    name: "13.8 Move the data and checkpoint with the new primary only",
    fn: async () => {
      // The client downloads from the old primary and uploads to the new one.
      const bytes = await downloadChunk(PROVIDER_URL, old.hash);
      const next = await uploadChunk(SECOND_PROVIDER_URL, moved, bytes, client);
      assert.strictEqual(
        next.commit.mmr_root,
        old.commit.mmr_root,
        "new primary should reach the same root"
      );
      await submitTx(
        checkpointTx(api, moved, next.commit, [{ provider: second, commit: next.commit }]),
        client.signer,
        { label: "checkpoint (new primary)" }
      );
      assert.deepStrictEqual(
        await snapshotSigners(api, moved),
        [0b10],
        "only the new primary should be in the snapshot bitfield"
      );
      assert.ok(
        bytesEq(await downloadChunk(SECOND_PROVIDER_URL, old.hash), bytes),
        "new primary should serve the moved data"
      );
    },
  });

  tests.push({
    name: "13.9 Old primary is not challengeable against the new snapshot",
    fn: async () => {
      assert.deepStrictEqual(await snapshotSigners(api, moved), [0b10], "13.8 must pass first");
      const tx = api.tx.StorageProvider.challenge_checkpoint({
        bucket_id: moved,
        provider: first.address,
        target: { leaf_index: BigInt(old.commit.leaf_indices[0]), chunk_index: 0n },
      });
      await submitTxExpectFailure(tx, client.signer, "ProviderNotInSnapshot", "13.9");
    },
  });

  await runSuite("13 — Two Primary Providers", tests, { api, papi });
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
