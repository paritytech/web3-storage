/**
 * Bucket membership example for pallet-storage-provider.
 *
 * Demonstrates the bucket ACL extrinsics by walking through a typical
 * sharing flow: create a bucket, add a Writer, add a Reader, promote
 * the Writer to Admin, then remove the Reader. After each change the
 * bucket members are printed so the effect is visible.
 *
 * This example is pure on-chain — no provider node, uploads, or
 * agreements are involved.
 *
 * Prerequisites:
 *   - Parachain running at ws://127.0.0.1:2222
 *   - Descriptors generated: npm run papi:generate
 *
 * Usage: node bucket-membership.js [chain_ws] [admin_seed] [writer_seed] [reader_seed]
 */

import { Enum } from "@polkadot-api/substrate-bindings";
import assert from "node:assert";
import {
  connect,
  makeSigner,
  printBucketMembers,
  requireOneEvent,
  waitForBlockProduction,
  waitForChainReady,
  waitForNextBlock,
} from "./common.js";
import { cryptoWaitReady } from "@polkadot/util-crypto";

const CHAIN_WS = process.argv[2] || "ws://127.0.0.1:2222";
const ADMIN_SEED = process.argv[3] || "//Alice";
const WRITER_SEED = process.argv[4] || "//Bob";
const READER_SEED = process.argv[5] || "//Charlie";

async function createBucket(api, admin) {
  const result = await api.tx.StorageProvider.create_bucket({
    min_providers: 1,
  }).signAndSubmit(admin.signer);
  const event = requireOneEvent(
    result.events,
    api.event.StorageProvider.BucketCreated,
    "BucketCreated"
  );
  console.log("  Bucket created: id=%s", event.bucket_id);
  return event.bucket_id;
}

async function setMember(api, admin, bucketId, member, role) {
  await api.tx.StorageProvider.set_member({
    bucket_id: bucketId,
    member: member.address,
    role: Enum(role),
  }).signAndSubmit(admin.signer);
}

async function removeMember(api, admin, bucketId, member) {
  await api.tx.StorageProvider.remove_member({
    bucket_id: bucketId,
    member: member.address,
  }).signAndSubmit(admin.signer);
}

async function verifyReverseIndex(api, member, bucketId, shouldContain) {
  const buckets = await api.query.StorageProvider.MemberBuckets.getValue(
    member.address
  );
  console.log("  MemberBuckets[%s] = %o", member.address, buckets);
  const has = buckets.some((id) => id === bucketId);
  assert.strictEqual(
    has,
    shouldContain,
    `reverse index for ${member.address} should ${shouldContain ? "contain" : "exclude"} bucket ${bucketId}`
  );
}

async function main() {
  await cryptoWaitReady();

  const admin = makeSigner(ADMIN_SEED);
  const writer = makeSigner(WRITER_SEED);
  const reader = makeSigner(READER_SEED);

  console.log("Chain:", CHAIN_WS);
  console.log("Admin  (%s) => %s", ADMIN_SEED, admin.address);
  console.log("Writer (%s) => %s", WRITER_SEED, writer.address);
  console.log("Reader (%s) => %s", READER_SEED, reader.address);

  const { papi, api } = await connect(CHAIN_WS);
  await waitForChainReady(api);
  await waitForBlockProduction(api);
  await waitForNextBlock(papi);

  try {
    console.log("\n=== Step 1: Create bucket ===");
    const bucketId = await createBucket(api, admin);
    await printBucketMembers(api, bucketId, "after create");

    console.log("\n=== Step 2: Add Writer ===");
    await setMember(api, admin, bucketId, writer, "Writer");
    await printBucketMembers(api, bucketId, "after add Writer");

    console.log("\n=== Step 3: Add Reader ===");
    await setMember(api, admin, bucketId, reader, "Reader");
    await printBucketMembers(api, bucketId, "after add Reader");

    console.log("\n=== Step 4: Promote Writer -> Admin ===");
    await setMember(api, admin, bucketId, writer, "Admin");
    await printBucketMembers(api, bucketId, "after promote");

    console.log("\n=== Step 5: Remove Reader ===");
    await removeMember(api, admin, bucketId, reader);
    await printBucketMembers(api, bucketId, "after remove");

    console.log("\n=== Step 6: Verify reverse index ===");
    await verifyReverseIndex(api, writer, bucketId, true);
    await verifyReverseIndex(api, reader, bucketId, false);
    console.log("PASSED: ACL transitions applied as expected");
  } catch (err) {
    console.error("\nERROR:", err.message || err);
    if (err.stack) console.error(err.stack);
    process.exitCode = 1;
  } finally {
    papi.destroy();
  }
}

main().then(() => console.log("\n=== Done ==="));
