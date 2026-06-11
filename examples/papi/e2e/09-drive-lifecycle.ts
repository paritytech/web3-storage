/**
 * E2E Workflow 09 — Drive Lifecycle
 *
 * Accounts: //Alice (provider), //Bob (owner), //Ferdie (member)
 *
 * Tests: create, share, unshare, delete drives.
 *
 * Usage: node e2e/09-drive-lifecycle.js [chain_ws] [provider_url]
 */

import assert from "node:assert";
import {
  ensureProviderRegistered,
  makeSigner,
  READ_OPTS,
  sameAddress,
  waitForAgreementAcceptance,
} from "@web3-storage/sdk";
import { FileSystemClient } from "@web3-storage/sdk/fs";
import { ensureSoleAcceptingProvider, printBucketMembers } from "../support.js";
import { runSuite, submitTxExpectFailure, setupChain, getFree } from "./helpers.js";

const CHAIN_WS = process.argv[2] || "ws://127.0.0.1:2222";
const PROVIDER_URL = process.argv[3] || "http://127.0.0.1:3333";

async function main() {
  const provider = makeSigner("//Alice");
  const owner = makeSigner("//Bob");
  const member = makeSigner("//Ferdie");

  const { papi, api } = await setupChain(CHAIN_WS);
  await ensureProviderRegistered(api, provider, PROVIDER_URL);
  const restore = await ensureSoleAcceptingProvider(api, provider);

  // The suite drives the file-system surface through the layer-1 client
  // (chain ops silent-by-default; fs HTTP ops signed when possible).
  const fs = new FileSystemClient({ api, signer: owner, providerUrl: PROVIDER_URL });

  let driveId: bigint, bucketId: bigint;

  const tests: Array<{ name: string; fn: () => Promise<void> }> = [];

  // ── Success ───────────────────────────────────────────────────────────────

  tests.push({
    name: "9.1 Create drive",
    fn: async () => {
      const maxCapacity = 1_048_576n;
      const storagePeriod = 100;
      const result = await fs.createDrive({
        name: `e2e-drive-${Date.now()}`,
        maxCapacity,
        storagePeriod,
        payment: maxCapacity * BigInt(storagePeriod) * 10n,
        minProviders: 1,
      });
      driveId = result.driveId;
      bucketId = result.bucketId;
      assert.ok(driveId !== undefined, "drive_id should be returned");
      assert.ok(bucketId !== undefined, "bucket_id should be returned");
      assert.ok(
        sameAddress(result.matchedProvider!, provider.address),
        "Should match expected provider"
      );
      await waitForAgreementAcceptance(api, provider.address, bucketId);

      // Verify storage.
      const drive = await api.query.DriveRegistry.Drives.getValue(driveId, READ_OPTS);
      assert.ok(drive, "Drive should exist in storage");
      const userDrives = await api.query.DriveRegistry.UserDrives.getValue(owner.address, READ_OPTS);
      assert.ok(userDrives.some((id) => id === driveId), "Owner's UserDrives should contain drive");
      const driveForBucket = await api.query.DriveRegistry.BucketToDrive.getValue(bucketId, READ_OPTS);
      assert.strictEqual(driveForBucket, driveId, "BucketToDrive should map back");
    },
  });

  tests.push({
    name: "9.2 Share drive (Writer)",
    fn: async () => {
      await fs.shareDrive(driveId, member.address, "Writer");
      const members = await printBucketMembers(api, bucketId, "after share Writer");
      assert.ok(
        members.some((m: { account: string }) => sameAddress(m.account, member.address)),
        "Member should appear in underlying bucket"
      );
    },
  });

  tests.push({
    name: "9.3 Share drive (Reader) — change role",
    fn: async () => {
      await fs.shareDrive(driveId, member.address, "Reader");
      const members = await fs.getBucketMembers(bucketId);
      const m = members.find((m) => sameAddress(m.account, member.address));
      assert.strictEqual(m!.role, "Reader", "Member should now be Reader");
    },
  });

  tests.push({
    name: "9.4 Unshare drive",
    fn: async () => {
      await fs.unshareDrive(driveId, member.address);
      const members = await fs.getBucketMembers(bucketId);
      assert.ok(
        !members.some((m: { account: string }) => sameAddress(m.account, member.address)),
        "Member should be gone from bucket"
      );
    },
  });

  tests.push({
    name: "9.5 Delete drive",
    fn: async () => {
      const ownerBefore = await getFree(api, owner);
      await fs.deleteDrive(driveId);
      const ownerAfter = await getFree(api, owner);
      // Owner should get a refund (balance increased, minus tx fees).
      console.log("    owner free delta = %s", (ownerAfter - ownerBefore).toString());
      const driveAfter = await api.query.DriveRegistry.Drives.getValue(driveId, READ_OPTS);
      assert.strictEqual(driveAfter, undefined, "Drive should be gone after delete");
    },
  });

  // ── Failure ───────────────────────────────────────────────────────────────

  tests.push({
    name: "9.6 Non-owner shares drive",
    fn: async () => {
      // Create a new drive for this test.
      const maxCapacity = 1_048_576n;
      const storagePeriod = 100;
      const result = await fs.createDrive({
        name: `e2e-drive-9b-${Date.now()}`,
        maxCapacity,
        storagePeriod,
        payment: maxCapacity * BigInt(storagePeriod) * 10n,
        minProviders: 1,
      });
      await waitForAgreementAcceptance(api, provider.address, result.bucketId);
      const tx = api.tx.DriveRegistry.share_drive({
        drive_id: result.driveId,
        member: owner.address,
        role: (await import("polkadot-api")).Enum("Writer"),
      });
      await submitTxExpectFailure(tx, member.signer, "NotAuthorizedToShare", "9.6");
    },
  });

  tests.push({
    name: "9.7 Non-owner deletes drive",
    fn: async () => {
      const maxCapacity = 1_048_576n;
      const storagePeriod = 100;
      const result = await fs.createDrive({
        name: `e2e-drive-9c-${Date.now()}`,
        maxCapacity,
        storagePeriod,
        payment: maxCapacity * BigInt(storagePeriod) * 10n,
        minProviders: 1,
      });
      await waitForAgreementAcceptance(api, provider.address, result.bucketId);
      const tx = api.tx.DriveRegistry.delete_drive({ drive_id: result.driveId });
      await submitTxExpectFailure(tx, member.signer, "NotDriveOwner", "9.7");
    },
  });

  tests.push({
    name: "9.8 FileSystemClient fs HTTP round-trip (mkdir/upload/ls/download/delete)",
    fn: async () => {
      const result = await fs.createDrive({
        name: `e2e-drive-9fs-${Date.now()}`,
        maxCapacity: 1_048_576n,
        storagePeriod: 100,
        payment: 1_048_576n * 100n * 10n,
        minProviders: 1,
      });
      await waitForAgreementAcceptance(api, provider.address, result.bucketId);

      await fs.createDirectory(result.bucketId, "/docs");
      const body = new TextEncoder().encode(`fs round-trip ${Date.now()}`);
      await fs.uploadFile(result.bucketId, "/docs/note.txt", body, { contentType: "text/plain" });

      const entries = await fs.listDirectory(result.bucketId, "/docs");
      assert.ok(entries.some((e) => e.name === "note.txt" && e.entryType === "file"), "ls should show the file");

      const downloaded = await fs.downloadFile(result.bucketId, "/docs/note.txt");
      assert.deepStrictEqual(downloaded, body, "downloaded bytes should match upload");

      await fs.deleteFile(result.bucketId, "/docs/note.txt");
      const after = await fs.listDirectory(result.bucketId, "/docs");
      assert.ok(!after.some((e) => e.name === "note.txt"), "file should be gone after delete");
    },
  });

  await runSuite("09 — Drive Lifecycle", tests, { api, papi });

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
