// SPDX-License-Identifier: GPL-3.0-only

/**
 * Challenge response spec.
 *
 * Bob creates a bucket on the node's provider (Alice), uploads a chunk and
 * checkpoints it. Bob then challenges Alice off chain. The spec signs in as
 * Alice and answers the challenge from the Challenges page. The provider node
 * does not answer challenges by itself in this setup (the challenge responder
 * is off unless `--enable-challenge-responder` is passed), so the UI is the
 * only responder.
 */
import { makeLocalPageFixture, expect } from "@web3-storage/test-helpers/playwright";
import { Alice, Bob, createBucketViaApi, getApi } from "@web3-storage/test-helpers";
import {
  challengeOffchain,
  fetchCheckpointSignature,
  submitClientCheckpoint,
  uploadChunk,
} from "@web3-storage/sdk";

const PROVIDER_URL = process.env.PROVIDER_URL ?? "http://127.0.0.1:3333";

const test = makeLocalPageFixture({
  localStorage: {
    "web3-storage-selected-network": "local",
    "provider-dashboard-wallet-mode": "dev",
    "provider-dashboard-wallet-account": Alice.address,
  },
});

test.describe.configure({ mode: "serial" });
test.setTimeout(240_000);

let challengeId: { deadline: number; index: number };

test.beforeAll(async () => {
  test.setTimeout(180_000);
  const api = getApi();
  const { bucketId } = await createBucketViaApi(Bob);
  const upload = await uploadChunk(PROVIDER_URL, bucketId, `challenge-ui @ ${Date.now()}`, Bob);
  const checkpoint = await fetchCheckpointSignature(PROVIDER_URL, bucketId, Bob);
  await submitClientCheckpoint(api, Bob, Alice, bucketId, checkpoint);
  challengeId = await challengeOffchain(api, Bob, Alice, bucketId, {
    leafIndex: upload.commit.leaf_indices[0],
    mmrRoot: upload.commit.mmr_root,
    startSeq: upload.commit.start_seq,
    leafCount: upload.commit.leaf_count,
    providerSignature: upload.commit.provider_signature,
  });
});

test("provider answers a challenge from the Challenges page", async ({ localPage }) => {
  const api = getApi();
  // The watch observes finalized blocks, so subscribe before responding.
  const defended: Array<{ deadline: number; index: number }> = [];
  const subscription = api.event.StorageProvider.ChallengeDefended.watch().subscribe(
    ({ events }) => {
      for (const { payload } of events) defended.push(payload.challenge_id);
    },
  );

  try {
    await localPage.getByTestId("nav-challenges").click();
    const row = localPage.getByTestId(`challenge-row-${challengeId.deadline}-${challengeId.index}`);
    await expect(row.getByTestId("challenge-status")).toContainText("Pending", { timeout: 30_000 });

    await row.getByTestId("challenge-action-button").click();

    await expect(row.getByTestId("challenge-status")).toContainText("Responded", {
      timeout: 60_000,
    });
    await expect(row).not.toContainText("Slashed");

    expect(
      await api.query.StorageProvider.Challenges.getValue(
        challengeId.deadline,
        challengeId.index,
        { at: "best" },
      ),
    ).toBeUndefined();
    await expect
      .poll(
        () =>
          defended.some(
            (id) => id.deadline === challengeId.deadline && id.index === challengeId.index,
          ),
        { timeout: 60_000 },
      )
      .toBe(true);
  } finally {
    subscription.unsubscribe();
  }
});
