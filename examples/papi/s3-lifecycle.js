/**
 * S3 Registry lifecycle example (pallet-s3-registry).
 *
 * Walks the full S3-style object workflow on Layer 1:
 *   create_s3_bucket_with_storage  (atomic: Layer 0 bucket + agreement request)
 *   -> provider accepts agreement
 *   -> upload chunks to the provider (HTTP)
 *   -> put_object_metadata for each object (CID + size + content-type)
 *   -> copy_object_metadata
 *   -> list objects by iterating Objects storage
 *   -> delete_object_metadata for each object
 *   -> delete_s3_bucket
 *
 * Prerequisites:
 *   - Parachain at ws://127.0.0.1:2222
 *   - Provider node running (this script will register/configure //Alice if needed)
 *
 * Usage: node s3-lifecycle.js [chain_ws] [provider_url] [provider_seed] [client_seed]
 */

import assert from "node:assert";
import { cryptoWaitReady } from "@polkadot/util-crypto";
import {
  copyObjectMetadata,
  createS3BucketWithStorage,
  deleteObjectMetadata,
  deleteS3Bucket,
  putChunk,
  putObjectMetadata,
} from "./api.js";
import {
  connect,
  ensureProviderRegistered,
  ensureSoleAcceptingProvider,
  makeSigner,
  parseProviderClientArgs,
  sameAddress,
  toHex,
  waitForAgreementAcceptance,
  waitForBlockProduction,
  waitForChainReady,
  waitForNextBlock,
} from "./common.js";

const {
  chainWs: CHAIN_WS,
  providerUrl: PROVIDER_URL,
  providerSeed: PROVIDER_SEED,
  clientSeed: CLIENT_SEED,
} = parseProviderClientArgs();

const BUCKET_NAME = `papi-demo-${Date.now()}`.slice(0, 63);
const OBJECT_KEYS = {
  hello: "docs/hello.txt",
  goodbye: "docs/goodbye.txt",
  helloCopy: "docs/hello-copy.txt",
};

async function listObjects(api, s3BucketId) {
  const entries = await api.query.S3Registry.Objects.getEntries(s3BucketId);
  console.log("  bucket contains %d object(s):", entries.length);
  for (const { keyArgs, value } of entries) {
    const key = new TextDecoder().decode(keyArgs[1].asBytes());
    console.log(
      "    - %s  cid=%s  size=%s  ct=%s",
      key,
      toHex(value.cid.asBytes()),
      value.size,
      new TextDecoder().decode(value.content_type.asBytes())
    );
  }
  const bucketInfo = await api.query.S3Registry.S3Buckets.getValue(s3BucketId);
  console.log(
    "  bucket stats: object_count=%s total_size=%s",
    bucketInfo.object_count,
    bucketInfo.total_size
  );
  return entries;
}

async function main() {
  await cryptoWaitReady();

  const provider = makeSigner(PROVIDER_SEED);
  const client = makeSigner(CLIENT_SEED);

  console.log("Chain:", CHAIN_WS, " Provider HTTP:", PROVIDER_URL);
  console.log("Provider (%s) => %s", PROVIDER_SEED, provider.address);
  console.log("Client   (%s) => %s", CLIENT_SEED, client.address);
  console.log("S3 bucket name:", BUCKET_NAME);

  const { papi, api } = await connect(CHAIN_WS);
  await waitForChainReady(api);
  await waitForBlockProduction(api);
  await waitForNextBlock(papi);

  let restoreOthers = null;
  try {
    console.log("\n=== Step 1: Ensure provider is ready ===");
    await ensureProviderRegistered(api, provider, PROVIDER_URL);
    // create_s3_bucket_with_storage picks query_available_providers[0],
    // which iterates in storage-hash order. With multiple registered
    // providers (CI registers //Alice and //Charlie) the match is
    // non-deterministic — silence the others so the demo always matches
    // the one whose HTTP endpoint we'll talk to.
    restoreOthers = await ensureSoleAcceptingProvider(api, provider);

    console.log("\n=== Step 2: create_s3_bucket_with_storage ===");
    const maxCapacity = 1_048_576n; // 1 MiB
    const duration = 100;
    const { s3BucketId, layer0BucketId, matchedProvider } =
      await createS3BucketWithStorage(api, client, BUCKET_NAME, {
        max_capacity: maxCapacity,
        duration,
        max_payment: maxCapacity * BigInt(duration) * 2n,
      });
    console.log("  s3_bucket_id     =", s3BucketId);
    console.log("  layer0_bucket_id =", layer0BucketId);
    console.log("  matched provider =", matchedProvider);
    if (!sameAddress(matchedProvider, provider.address)) {
      throw new Error(
        `create_s3_bucket_with_storage matched ${matchedProvider}, expected ${provider.address}. ` +
          `The provider node at ${PROVIDER_URL} can only accept for ${PROVIDER_SEED}.`
      );
    }

    console.log("\n=== Step 3: Wait for provider to auto-accept agreement ===");
    // The provider node's agreement_coordinator polls every ~6s and accepts
    // pending requests automatically. Don't try to submit accept_agreement
    // ourselves — that races the coordinator and intermittently fails with
    // AgreementRequestNotFound when the coordinator wins.
    await waitForAgreementAcceptance(api, provider.address, layer0BucketId);
    console.log("  Agreement accepted by", provider.address);

    console.log("\n=== Step 4: Upload two objects to the provider ===");
    const obj1 = await putChunk(
      PROVIDER_URL,
      layer0BucketId,
      "hello world from the s3 demo\n"
    );
    const obj2 = await putChunk(
      PROVIDER_URL,
      layer0BucketId,
      `goodbye world @ ${new Date().toISOString()}\n`
    );
    console.log("  obj1 cid=%s size=%s", obj1.hash, obj1.size);
    console.log("  obj2 cid=%s size=%s", obj2.hash, obj2.size);

    console.log("\n=== Step 5: put_object_metadata ===");
    await putObjectMetadata(
      api,
      client,
      s3BucketId,
      OBJECT_KEYS.hello,
      obj1,
      "text/plain",
      [["author", "alice"]]
    );
    await putObjectMetadata(
      api,
      client,
      s3BucketId,
      OBJECT_KEYS.goodbye,
      obj2,
      "text/plain"
    );
    console.log("  put 2 objects");

    console.log("\n=== Step 6: copy_object_metadata ===");
    await copyObjectMetadata(
      api,
      client,
      s3BucketId,
      OBJECT_KEYS.hello,
      s3BucketId,
      OBJECT_KEYS.helloCopy
    );
    console.log("  copied %s -> %s", OBJECT_KEYS.hello, OBJECT_KEYS.helloCopy);

    console.log("\n=== Step 7: List objects ===");
    const entries = await listObjects(api, s3BucketId);
    assert.strictEqual(entries.length, 3, "Expected 3 objects after copy");

    console.log("\n=== Step 8: Delete all objects ===");
    for (const key of Object.values(OBJECT_KEYS)) {
      await deleteObjectMetadata(api, client, s3BucketId, key);
    }
    const afterDelete = await api.query.S3Registry.Objects.getEntries(s3BucketId);
    assert.strictEqual(afterDelete.length, 0, "Expected empty bucket after delete");
    console.log("  Bucket is empty");

    console.log("\n=== Step 9: delete_s3_bucket ===");
    await deleteS3Bucket(api, client, s3BucketId);
    console.log("  Bucket deleted");
    console.log("PASSED: full S3 object lifecycle exercised");
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
