/**
 * S3 Registry lifecycle example (pallet-s3-registry).
 *
 * Walks the full S3-style object workflow on Layer 1:
 *   negotiate provider-signed agreement terms over HTTP
 *   -> create_s3_bucket  (atomic: Layer 0 bucket + primary agreement + S3 bucket)
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
import {
  copyObjectMetadata,
  createS3Bucket,
  deleteObjectMetadata,
  deleteS3Bucket,
  negotiateTerms,
  putChunk,
  putObjectMetadata,
} from "./api.js";
import {
  connect,
  ensureProviderRegistered,
  makeSigner,
  parseProviderClientArgs,
  toHex,
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

  try {
    console.log("\n=== Step 1: Ensure provider is ready ===");
    await ensureProviderRegistered(api, provider, PROVIDER_URL);

    console.log("\n=== Step 2: Negotiate signed agreement terms ===");
    const maxBytes = 1_048_576n; // 1 MiB
    const duration = 100;
    const signed = await negotiateTerms(PROVIDER_URL, {
      owner: client.address,
      max_bytes: maxBytes,
      duration,
      price_per_byte: 1n,
      replica_params: null,
    });
    console.log(
      "  Provider signed terms: nonce=%s, valid_until=%s",
      signed.terms.nonce,
      signed.terms.valid_until
    );

    console.log("\n=== Step 3: create_s3_bucket ===");
    const { s3BucketId, layer0BucketId } = await createS3Bucket(
      api,
      client,
      BUCKET_NAME,
      provider,
      signed
    );
    console.log("  s3_bucket_id     =", s3BucketId);
    console.log("  layer0_bucket_id =", layer0BucketId);

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
    papi.destroy();
  }
}

main().then(() => console.log("\n=== Done ==="));
