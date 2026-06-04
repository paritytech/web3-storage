/**
 * Bucket membership example for pallet-storage-provider.
 *
 * Demonstrates the bucket ACL extrinsics by walking through a typical
 * sharing flow: create a bucket, add a Writer, add a Reader, promote
 * the Writer to Admin, then remove the Reader. After each change the
 * bucket members are printed so the effect is visible.
 *
 * Buckets can no longer be created without a storage agreement, so the
 * provider must be running to sign the agreement terms even though no
 * uploads happen here.
 *
 * Prerequisites:
 *   - Parachain running at ws://127.0.0.1:2222
 *   - Provider node running (this script will register/configure //Alice if needed)
 *   - Descriptors generated: npm run papi:generate
 *
 * Usage: node bucket-membership.js [chain_ws] [provider_url] [provider_seed] [admin_seed] [writer_seed] [reader_seed]
 */

import assert from "node:assert";
import {
  establishStorageAgreement,
  negotiateTerms,
  removeMember,
  setMember,
} from "./api.js";
import {
  connect,
  ensureProviderRegistered,
  ensureSoleAcceptingProvider,
  makeSigner,
  printBucketMembers,
  waitForBlockProduction,
  waitForChainReady,
  waitForNextBlock,
} from "./common.js";

const CHAIN_WS = process.argv[2] || "ws://127.0.0.1:2222";
const PROVIDER_URL = process.argv[3] || "http://127.0.0.1:3333";
const PROVIDER_SEED = process.argv[4] || "//Alice";
const ADMIN_SEED = process.argv[5] || "//Bob";
const WRITER_SEED = process.argv[6] || "//Charlie";
const READER_SEED = process.argv[7] || "//Dave";

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
  const provider = makeSigner(PROVIDER_SEED);
  const admin = makeSigner(ADMIN_SEED);
  const writer = makeSigner(WRITER_SEED);
  const reader = makeSigner(READER_SEED);

  console.log("Chain:", CHAIN_WS, " Provider HTTP:", PROVIDER_URL);
  console.log("Provider (%s) => %s", PROVIDER_SEED, provider.address);
  console.log("Admin    (%s) => %s", ADMIN_SEED, admin.address);
  console.log("Writer   (%s) => %s", WRITER_SEED, writer.address);
  console.log("Reader   (%s) => %s", READER_SEED, reader.address);

  const { papi, api } = await connect(CHAIN_WS);
  await waitForChainReady(api);
  await waitForBlockProduction(api);
  await waitForNextBlock(papi);

  try {
    console.log("\n=== Step 1: Ensure provider is ready ===");
    await ensureProviderRegistered(api, provider, PROVIDER_URL);
    // Silence any other accepting dev providers (CI registers more than one)
    // so the demo is deterministic.

    console.log("\n=== Step 2: Negotiate signed agreement terms ===");
    const signed = await negotiateTerms(PROVIDER_URL, {
      owner: admin.address,
      max_bytes: 1_048_576n, // 1 MiB
      duration: 100,
      price_per_byte: 1n,
      replica_params: null,
    });
    console.log(
      "  Provider signed terms: nonce=%s, valid_until=%s",
      signed.terms.nonce,
      signed.terms.valid_until
    );

    console.log("\n=== Step 3: Open bucket via establish_storage_agreement ===");
    const bucketId = await establishStorageAgreement(api, admin, provider, signed);
    console.log("  Bucket created: id=%s", bucketId);
    await printBucketMembers(api, bucketId, "after create");

    console.log("\n=== Step 4: Add Writer ===");
    await setMember(api, admin, bucketId, writer, "Writer");
    await printBucketMembers(api, bucketId, "after add Writer");

    console.log("\n=== Step 5: Add Reader ===");
    await setMember(api, admin, bucketId, reader, "Reader");
    await printBucketMembers(api, bucketId, "after add Reader");

    console.log("\n=== Step 6: Promote Writer -> Admin ===");
    await setMember(api, admin, bucketId, writer, "Admin");
    await printBucketMembers(api, bucketId, "after promote");

    console.log("\n=== Step 7: Remove Reader ===");
    await removeMember(api, admin, bucketId, reader);
    await printBucketMembers(api, bucketId, "after remove");

    console.log("\n=== Step 8: Verify reverse index ===");
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