/**
 * Drive Registry lifecycle example (pallet-drive-registry).
 *
 * Demonstrates the Layer 1 file-system drive workflow:
 *   create_drive  (atomic: Layer 0 bucket + primary agreement request)
 *   -> provider accepts agreement
 *   -> query Drives and UserDrives
 *   -> share_drive (Writer)
 *   -> unshare_drive
 *   -> delete_drive  (computes prorated refund)
 *
 * Only on-chain extrinsics are exercised; no files are uploaded to the
 * provider. The provider node still needs to be running so it can be
 * registered on chain (its public key/multiaddr live there).
 *
 * Prerequisites:
 *   - Parachain at ws://127.0.0.1:2222
 *   - Provider node running (this script will register/configure //Alice if needed)
 *
 * Usage: node drive-lifecycle.js [chain_ws] [provider_url] [provider_seed] [owner_seed] [member_seed]
 */

import assert from "node:assert";
import { cryptoWaitReady } from "@polkadot/util-crypto";
import { createDrive, deleteDrive, shareDrive, unshareDrive } from "./api.js";
import {
  bytesToUtf8,
  connect,
  ensureProviderRegistered,
  ensureSoleAcceptingProvider,
  fmtRole,
  makeSigner,
  printBucketMembers,
  sameAddress,
  waitForAgreementAcceptance,
  waitForBlockProduction,
  waitForChainReady,
  waitForNextBlock,
} from "./common.js";

const CHAIN_WS = process.argv[2] || "ws://127.0.0.1:2222";
const PROVIDER_URL = process.argv[3] || "http://127.0.0.1:3333";
const PROVIDER_SEED = process.argv[4] || "//Alice";
const OWNER_SEED = process.argv[5] || "//Bob";
const MEMBER_SEED = process.argv[6] || "//Charlie";

async function printDriveInfo(api, owner, driveId, bucketId) {
  const drive = await api.query.DriveRegistry.Drives.getValue(driveId);
  console.log("  owner          =", drive.owner);
  console.log("  name           =", drive.name ? bytesToUtf8(drive.name) : "(none)");
  console.log("  max_capacity   =", drive.max_capacity);
  console.log("  storage_period =", drive.storage_period);
  console.log("  expires_at     =", drive.expires_at);
  console.log("  payment locked =", drive.payment);

  const userDrives = await api.query.DriveRegistry.UserDrives.getValue(
    owner.address
  );
  console.log("  UserDrives[owner] =", userDrives);
  assert.ok(
    userDrives.some((id) => id === driveId),
    "owner's UserDrives should contain the new drive"
  );

  const driveIdForBucket = await api.query.DriveRegistry.BucketToDrive.getValue(
    bucketId
  );
  assert.strictEqual(
    driveIdForBucket,
    driveId,
    "BucketToDrive should map back to drive_id"
  );
}

async function getFree(api, who) {
  const acc = await api.query.System.Account.getValue(who.address);
  return acc.data.free;
}

async function main() {
  await cryptoWaitReady();

  const provider = makeSigner(PROVIDER_SEED);
  const owner = makeSigner(OWNER_SEED);
  const member = makeSigner(MEMBER_SEED);

  console.log("Chain:", CHAIN_WS);
  console.log("Provider (%s) => %s", PROVIDER_SEED, provider.address);
  console.log("Owner    (%s) => %s", OWNER_SEED, owner.address);
  console.log("Member   (%s) => %s", MEMBER_SEED, member.address);

  const { papi, api } = await connect(CHAIN_WS);
  await waitForChainReady(api);
  await waitForBlockProduction(api);
  await waitForNextBlock(papi);

  let restoreOthers = null;
  try {
    console.log("\n=== Step 1: Ensure provider is ready ===");
    await ensureProviderRegistered(api, provider, PROVIDER_URL);
    // create_drive selects via query_available_providers[0] (hash-iter
    // order), so silence every other accepting dev provider — without this
    // the demo flakes when a co-registered //Charlie wins the iteration.
    restoreOthers = await ensureSoleAcceptingProvider(api, provider);

    console.log("\n=== Step 2: create_drive ===");
    const maxCapacity = 1_048_576n; // 1 MiB
    const storagePeriod = 200; // < 1000 -> auto-selects 1 provider
    const { driveId, bucketId, matchedProvider } = await createDrive(
      api,
      owner,
      `papi-demo-${Date.now()}`,
      {
        max_capacity: maxCapacity,
        storage_period: storagePeriod,
        payment: maxCapacity * BigInt(storagePeriod) * 2n,
        min_providers: 1,
      }
    );
    console.log("  drive_id         =", driveId);
    console.log("  bucket_id        =", bucketId);
    console.log("  matched provider =", matchedProvider);
    if (!sameAddress(matchedProvider, provider.address)) {
      throw new Error(
        `create_drive matched ${matchedProvider}, expected ${provider.address}. ` +
          `Only ${PROVIDER_SEED} can accept this agreement.`
      );
    }

    console.log("\n=== Step 3: Wait for provider to auto-accept agreement ===");
    // The provider node's agreement_coordinator polls every ~6s and accepts
    // pending requests automatically. Avoid racing it from the script.
    await waitForAgreementAcceptance(api, provider.address, bucketId);
    console.log("  Agreement accepted by", provider.address);

    console.log("\n=== Step 4: Query drive state ===");
    await printDriveInfo(api, owner, driveId, bucketId);

    console.log("\n=== Step 5: share_drive(Writer) ===");
    const shared = await shareDrive(api, owner, driveId, member, "Writer");
    console.log("  DriveShared with %s as %s", member.address, fmtRole(shared.role));
    const members = await printBucketMembers(api, bucketId, "after share");
    assert.ok(
      members.some((m) => sameAddress(m.account, member.address)),
      "member must appear in the underlying bucket's members"
    );

    console.log("\n=== Step 6: unshare_drive ===");
    await unshareDrive(api, owner, driveId, member);
    const after = await printBucketMembers(api, bucketId, "after unshare");
    assert.ok(
      !after.some((m) => sameAddress(m.account, member.address)),
      "member must be gone from the underlying bucket"
    );

    console.log("\n=== Step 7: delete_drive ===");
    const ownerBefore = await getFree(api, owner);
    const providerBefore = await getFree(api, provider);
    const deleted = await deleteDrive(api, owner, driveId);
    console.log("  refunded to owner =", deleted.refunded);
    const ownerAfter = await getFree(api, owner);
    const providerAfter = await getFree(api, provider);
    console.log("  owner free delta    =", ownerAfter - ownerBefore);
    console.log("  provider free delta =", providerAfter - providerBefore);

    const driveAfter = await api.query.DriveRegistry.Drives.getValue(driveId);
    assert.strictEqual(driveAfter, undefined, "Drive should be gone after delete");
    console.log("PASSED: drive lifecycle complete");
  } catch (err) {
    console.error("\nERROR:", err.message || err);
    if (err.stack) console.error(err.stack);
    process.exitCode = 1;
  } finally {
    if (restoreOthers) {
      try {
        await restoreOthers();
      } catch (err) {
        console.error("WARN: failed to restore other providers:", err.message || err);
      }
    }
    papi.destroy();
  }
}

main().then(() => console.log("\n=== Done ==="));
