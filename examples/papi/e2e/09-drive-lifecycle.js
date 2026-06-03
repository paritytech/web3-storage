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
import { createDrive, deleteDrive, shareDrive, unshareDrive } from "../api.js";
import {
  ensureProviderRegistered,
  ensureSoleAcceptingProvider,
  fmtRole,
  makeSigner,
  printBucketMembers,
  sameAddress,
  waitForAgreementAcceptance,
} from "../common.js";
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

  let driveId, bucketId;

  const tests = [];

  // ── Success ───────────────────────────────────────────────────────────────

  tests.push({
    name: "9.1 Create drive",
    fn: async () => {
      const maxCapacity = 1_048_576n;
      const storagePeriod = 100;
      const result = await createDrive(api, owner, `e2e-drive-${Date.now()}`, {
        max_capacity: maxCapacity,
        storage_period: storagePeriod,
        payment: maxCapacity * BigInt(storagePeriod) * 10n,
        min_providers: 1,
      });
      driveId = result.driveId;
      bucketId = result.bucketId;
      assert.ok(driveId !== undefined, "drive_id should be returned");
      assert.ok(bucketId !== undefined, "bucket_id should be returned");
      assert.ok(
        sameAddress(result.matchedProvider, provider.address),
        "Should match expected provider"
      );
      await waitForAgreementAcceptance(api, provider.address, bucketId);

      // Verify storage.
      const drive = await api.query.DriveRegistry.Drives.getValue(driveId);
      assert.ok(drive, "Drive should exist in storage");
      const userDrives = await api.query.DriveRegistry.UserDrives.getValue(owner.address);
      assert.ok(userDrives.some((id) => id === driveId), "Owner's UserDrives should contain drive");
      const driveForBucket = await api.query.DriveRegistry.BucketToDrive.getValue(bucketId);
      assert.strictEqual(driveForBucket, driveId, "BucketToDrive should map back");
    },
  });

  tests.push({
    name: "9.2 Share drive (Writer)",
    fn: async () => {
      const event = await shareDrive(api, owner, driveId, member, "Writer");
      assert.ok(event, "Should get DriveShared event");
      const members = await printBucketMembers(api, bucketId, "after share Writer");
      assert.ok(
        members.some((m) => sameAddress(m.account, member.address)),
        "Member should appear in underlying bucket"
      );
    },
  });

  tests.push({
    name: "9.3 Share drive (Reader) — change role",
    fn: async () => {
      const event = await shareDrive(api, owner, driveId, member, "Reader");
      assert.ok(event, "Should get DriveShared event");
      const bucket = await api.query.StorageProvider.Buckets.getValue(bucketId);
      const m = bucket.members.find((m) => sameAddress(m.account, member.address));
      assert.strictEqual(m.role.type, "Reader", "Member should now be Reader");
    },
  });

  tests.push({
    name: "9.4 Unshare drive",
    fn: async () => {
      const event = await unshareDrive(api, owner, driveId, member);
      assert.ok(event, "Should get DriveUnshared event");
      const bucket = await api.query.StorageProvider.Buckets.getValue(bucketId);
      assert.ok(
        !bucket.members.some((m) => sameAddress(m.account, member.address)),
        "Member should be gone from bucket"
      );
    },
  });

  tests.push({
    name: "9.5 Delete drive",
    fn: async () => {
      const ownerBefore = await getFree(api, owner);
      const event = await deleteDrive(api, owner, driveId);
      assert.ok(event, "Should get DriveDeleted event");
      const ownerAfter = await getFree(api, owner);
      // Owner should get a refund (balance increased, minus tx fees).
      console.log("    owner free delta = %s", (ownerAfter - ownerBefore).toString());
      const driveAfter = await api.query.DriveRegistry.Drives.getValue(driveId);
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
      const result = await createDrive(api, owner, `e2e-drive-9b-${Date.now()}`, {
        max_capacity: maxCapacity,
        storage_period: storagePeriod,
        payment: maxCapacity * BigInt(storagePeriod) * 10n,
        min_providers: 1,
      });
      await waitForAgreementAcceptance(api, provider.address, result.bucketId);
      const tx = api.tx.DriveRegistry.share_drive({
        drive_id: result.driveId,
        member: owner.address,
        role: (await import("@polkadot-api/substrate-bindings")).Enum("Writer"),
      });
      await submitTxExpectFailure(tx, member.signer, "NotAuthorizedToShare", "9.6");
    },
  });

  tests.push({
    name: "9.7 Non-owner deletes drive",
    fn: async () => {
      const maxCapacity = 1_048_576n;
      const storagePeriod = 100;
      const result = await createDrive(api, owner, `e2e-drive-9c-${Date.now()}`, {
        max_capacity: maxCapacity,
        storage_period: storagePeriod,
        payment: maxCapacity * BigInt(storagePeriod) * 10n,
        min_providers: 1,
      });
      await waitForAgreementAcceptance(api, provider.address, result.bucketId);
      const tx = api.tx.DriveRegistry.delete_drive({ drive_id: result.driveId });
      await submitTxExpectFailure(tx, member.signer, "NotDriveOwner", "9.7");
    },
  });

  await runSuite("09 — Drive Lifecycle", tests, { api, papi });

  try {
    await restore();
  } catch {}
  papi.destroy();
}

main();
