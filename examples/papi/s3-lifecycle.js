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

import { Binary } from "@polkadot-api/substrate-bindings";
import { blake2AsU8a, cryptoWaitReady } from "@polkadot/util-crypto";
import assert from "node:assert";
import {
  connect,
  makeSigner,
  toHex,
  utf8,
  providerFetch,
  ensureProviderRegistered,
  ensureSoleAcceptingProvider,
  waitForAgreementAcceptance,
  requireOneEvent,
  sameAddress,
  submitTx,
  waitForBlockProduction,
  waitForChainReady,
  waitForNextBlock,
} from "./common.js";

const CHAIN_WS = process.argv[2] || "ws://127.0.0.1:2222";
const PROVIDER_URL = process.argv[3] || "http://127.0.0.1:3333";
const PROVIDER_SEED = process.argv[4] || "//Alice";
const CLIENT_SEED = process.argv[5] || "//Bob";

const BUCKET_NAME = `papi-demo-${Date.now()}`.slice(0, 63);
const OBJECT_KEYS = {
  hello: "docs/hello.txt",
  goodbye: "docs/goodbye.txt",
  helloCopy: "docs/hello-copy.txt",
};

async function createS3Bucket(api, client, name, params) {
  const result = await submitTx(
    api.tx.S3Registry.create_s3_bucket_with_storage({
      name: Binary.fromBytes(utf8(name)),
      ...params,
    }),
    client.signer,
    "create_s3_bucket_with_storage"
  );
  const event = requireOneEvent(
    result.events,
    api.event.S3Registry.S3BucketCreated,
    "S3BucketCreated"
  );
  // Find the matched provider from the Layer 0 AgreementRequested event so the
  // caller can verify the provider node at PROVIDER_URL is the one that will
  // need to accept it.
  const requested = api.event.StorageProvider.AgreementRequested.filter(
    result.events
  );
  const matchedProvider = requested[0]?.provider;
  console.log("  s3_bucket_id     =", event.s3_bucket_id);
  console.log("  layer0_bucket_id =", event.layer0_bucket_id);
  console.log("  matched provider =", matchedProvider);
  return {
    s3BucketId: event.s3_bucket_id,
    layer0BucketId: event.layer0_bucket_id,
    matchedProvider,
  };
}

async function uploadObject(providerUrl, layer0BucketId, payload) {
  const bytes = utf8(payload);
  const cid = blake2AsU8a(bytes);
  await providerFetch(providerUrl, "/node", {
    method: "PUT",
    body: {
      bucket_id: Number(layer0BucketId),
      hash: toHex(cid),
      data: Buffer.from(bytes).toString("base64"),
      children: null,
    },
  });
  return { cid, size: BigInt(bytes.length) };
}

async function putObject(api, client, s3BucketId, key, obj, contentType, userMetadata = []) {
  await submitTx(
    api.tx.S3Registry.put_object_metadata({
      s3_bucket_id: s3BucketId,
      key: Binary.fromBytes(utf8(key)),
      cid: Binary.fromBytes(obj.cid),
      size: obj.size,
      content_type: Binary.fromBytes(utf8(contentType)),
      user_metadata: userMetadata.map(([k, v]) => [
        Binary.fromBytes(utf8(k)),
        Binary.fromBytes(utf8(v)),
      ]),
    }),
    client.signer,
    `put_object_metadata(${key})`
  );
}

async function copyObject(api, client, srcBucketId, srcKey, dstBucketId, dstKey) {
  await submitTx(
    api.tx.S3Registry.copy_object_metadata({
      src_bucket_id: srcBucketId,
      src_key: Binary.fromBytes(utf8(srcKey)),
      dst_bucket_id: dstBucketId,
      dst_key: Binary.fromBytes(utf8(dstKey)),
    }),
    client.signer,
    `copy_object_metadata(${srcKey} -> ${dstKey})`
  );
}

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

async function deleteObject(api, client, s3BucketId, key) {
  await submitTx(
    api.tx.S3Registry.delete_object_metadata({
      s3_bucket_id: s3BucketId,
      key: Binary.fromBytes(utf8(key)),
    }),
    client.signer,
    `delete_object_metadata(${key})`
  );
}

async function deleteS3Bucket(api, client, s3BucketId) {
  const result = await submitTx(
    api.tx.S3Registry.delete_s3_bucket({ s3_bucket_id: s3BucketId }),
    client.signer,
    "delete_s3_bucket"
  );
  requireOneEvent(
    result.events,
    api.event.S3Registry.S3BucketDeleted,
    "S3BucketDeleted"
  );
  console.log("  Bucket deleted");
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
    const { s3BucketId, layer0BucketId, matchedProvider } = await createS3Bucket(
      api,
      client,
      BUCKET_NAME,
      {
        max_capacity: maxCapacity,
        duration,
        max_payment: maxCapacity * BigInt(duration) * 2n,
      }
    );
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
    const obj1 = await uploadObject(
      PROVIDER_URL,
      layer0BucketId,
      "hello world from the s3 demo\n"
    );
    const obj2 = await uploadObject(
      PROVIDER_URL,
      layer0BucketId,
      `goodbye world @ ${new Date().toISOString()}\n`
    );
    console.log("  obj1 cid=%s size=%s", toHex(obj1.cid), obj1.size);
    console.log("  obj2 cid=%s size=%s", toHex(obj2.cid), obj2.size);

    console.log("\n=== Step 5: put_object_metadata ===");
    await putObject(api, client, s3BucketId, OBJECT_KEYS.hello, obj1, "text/plain", [
      ["author", "alice"],
    ]);
    await putObject(api, client, s3BucketId, OBJECT_KEYS.goodbye, obj2, "text/plain");
    console.log("  put 2 objects");

    console.log("\n=== Step 6: copy_object_metadata ===");
    await copyObject(
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
      await deleteObject(api, client, s3BucketId, key);
    }
    const afterDelete = await api.query.S3Registry.Objects.getEntries(s3BucketId);
    assert.strictEqual(afterDelete.length, 0, "Expected empty bucket after delete");
    console.log("  Bucket is empty");

    console.log("\n=== Step 9: delete_s3_bucket ===");
    await deleteS3Bucket(api, client, s3BucketId);
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
