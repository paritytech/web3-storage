// SPDX-License-Identifier: Apache-2.0

/**
 * E2E Workflow 03 — S3 Bucket and Objects
 *
 * Accounts: //Alice (provider), //Bob (client)
 *
 * Tests: S3 CRUD, bucket lifecycle, failure cases.
 *
 * Usage: node e2e/03-s3-bucket-and-objects.js [chain_ws] [provider_url]
 */

import assert from "node:assert";
import {
  buildSignedTermsArgs,
  copyObjectMetadata,
  createS3Bucket,
  deleteObjectMetadata,
  deleteS3Bucket,
  putChunk,
  putObjectMetadata,
} from "../api.js";
import {
  ensureProviderRegistered,
  ensureSoleAcceptingProvider,
  makeSigner,
  READ_OPTS,
  toHex,
} from "../common.js";
import {
  negotiateSigned,
  runSuite,
  submitTxExpectFailure,
  setupChain,
} from "./helpers.js";

const CHAIN_WS = process.argv[2] || "ws://127.0.0.1:2222";
const PROVIDER_URL = process.argv[3] || "http://127.0.0.1:3333";

/**
 * Create an S3 bucket: negotiate provider-signed terms, then redeem them via
 * `create_s3_bucket`, which opens the underlying Layer 0 bucket + primary
 * agreement atomically. Returns `{ s3BucketId, layer0BucketId }`.
 */
async function createS3BucketWithStorage(api, providerUrl, client, provider, name, { maxBytes, duration }) {
  const signed = await negotiateSigned(api, providerUrl, client, provider, {
    maxBytes,
    duration,
  });
  return createS3Bucket(api, client, name, provider, signed);
}

async function main() {
  const provider = makeSigner("//Alice");
  const client = makeSigner("//Bob");

  const { papi, api } = await setupChain(CHAIN_WS);
  await ensureProviderRegistered(api, provider, PROVIDER_URL);
  const restore = await ensureSoleAcceptingProvider(api, provider);

  let s3BucketId, layer0BucketId;
  const bucketName = `e2e-03-${Date.now()}`.slice(0, 63);

  const tests = [];

  // ── Success ───────────────────────────────────────────────────────────────

  tests.push({
    name: "3.1 Create S3 bucket with storage",
    fn: async () => {
      const maxCapacity = 1_048_576n;
      const duration = 100;
      const result = await createS3BucketWithStorage(
        api,
        PROVIDER_URL,
        client,
        provider,
        bucketName,
        { maxBytes: maxCapacity, duration }
      );
      s3BucketId = result.s3BucketId;
      layer0BucketId = result.layer0BucketId;
      assert.ok(s3BucketId !== undefined, "s3_bucket_id should be returned");
      assert.ok(layer0BucketId !== undefined, "layer0_bucket_id should be returned");
    },
  });

  tests.push({
    name: "3.2 Put object metadata",
    fn: async () => {
      const obj = await putChunk(PROVIDER_URL, layer0BucketId, "hello from e2e test");
      await putObjectMetadata(api, client, s3BucketId, "test.txt", obj, "text/plain");
      const stored = await api.query.S3Registry.Objects.getValue(
        s3BucketId,
        new TextEncoder().encode("test.txt"),
        READ_OPTS
      );
      assert.ok(stored, "Object should exist in storage");
      assert.strictEqual(stored.size, obj.size, "Size should match");
    },
  });

  tests.push({
    name: "3.3 Put with user metadata",
    fn: async () => {
      const obj = await putChunk(PROVIDER_URL, layer0BucketId, "data with metadata");
      await putObjectMetadata(api, client, s3BucketId, "meta.txt", obj, "text/plain", [
        ["author", "e2e-test"],
        ["version", "1"],
      ]);
      const stored = await api.query.S3Registry.Objects.getValue(
        s3BucketId,
        new TextEncoder().encode("meta.txt"),
        READ_OPTS
      );
      assert.ok(stored, "Object with metadata should exist");
    },
  });

  tests.push({
    name: "3.4 Copy object metadata",
    fn: async () => {
      await copyObjectMetadata(api, client, s3BucketId, "test.txt", s3BucketId, "test-copy.txt");
      const original = await api.query.S3Registry.Objects.getValue(
        s3BucketId,
        new TextEncoder().encode("test.txt"),
        READ_OPTS
      );
      const copy = await api.query.S3Registry.Objects.getValue(
        s3BucketId,
        new TextEncoder().encode("test-copy.txt"),
        READ_OPTS
      );
      assert.ok(copy, "Copy should exist");
      assert.deepStrictEqual(
        copy.cid,
        original.cid,
        "CID should be the same after copy"
      );
    },
  });

  tests.push({
    name: "3.5 Delete object metadata",
    fn: async () => {
      await deleteObjectMetadata(api, client, s3BucketId, "meta.txt");
      const stored = await api.query.S3Registry.Objects.getValue(
        s3BucketId,
        new TextEncoder().encode("meta.txt"),
        READ_OPTS
      );
      assert.strictEqual(stored, undefined, "Object should be gone after delete");
      const bucketInfo = await api.query.S3Registry.S3Buckets.getValue(s3BucketId, READ_OPTS);
      assert.ok(bucketInfo, "Bucket should still exist");
    },
  });

  tests.push({
    name: "3.6 Delete S3 bucket (after removing all objects)",
    fn: async () => {
      // Remove remaining objects.
      await deleteObjectMetadata(api, client, s3BucketId, "test.txt");
      await deleteObjectMetadata(api, client, s3BucketId, "test-copy.txt");
      const result = await deleteS3Bucket(api, client, s3BucketId);
      assert.ok(result, "Should get S3BucketDeleted event");
      const after = await api.query.S3Registry.S3Buckets.getValue(s3BucketId, READ_OPTS);
      assert.strictEqual(after, undefined, "Bucket should be gone");
    },
  });

  // ── Failure ───────────────────────────────────────────────────────────────

  tests.push({
    name: "3.7 Delete non-empty bucket",
    fn: async () => {
      // Create a new bucket for this test.
      const name2 = `e2e-03b-${Date.now()}`.slice(0, 63);
      const maxCapacity = 1_048_576n;
      const duration = 100;
      const { s3BucketId: bid, layer0BucketId: l0 } = await createS3BucketWithStorage(
        api,
        PROVIDER_URL,
        client,
        provider,
        name2,
        { maxBytes: maxCapacity, duration }
      );
      const obj = await putChunk(PROVIDER_URL, l0, "not empty");
      await putObjectMetadata(api, client, bid, "file.txt", obj, "text/plain");
      const tx = api.tx.S3Registry.delete_s3_bucket({ s3_bucket_id: bid });
      await submitTxExpectFailure(tx, client.signer, "BucketNotEmpty", "3.7");
      // Cleanup
      await deleteObjectMetadata(api, client, bid, "file.txt");
      await deleteS3Bucket(api, client, bid);
    },
  });

  tests.push({
    name: "3.8 Delete non-existent object",
    fn: async () => {
      const name3 = `e2e-03c-${Date.now()}`.slice(0, 63);
      const maxCapacity = 1_048_576n;
      const duration = 100;
      const { s3BucketId: bid } = await createS3BucketWithStorage(
        api,
        PROVIDER_URL,
        client,
        provider,
        name3,
        { maxBytes: maxCapacity, duration }
      );
      const tx = api.tx.S3Registry.delete_object_metadata({
        s3_bucket_id: bid,
        key: new TextEncoder().encode("does-not-exist.txt"),
      });
      await submitTxExpectFailure(tx, client.signer, "ObjectNotFound", "3.8");
      // Cleanup
      await deleteS3Bucket(api, client, bid);
    },
  });

  tests.push({
    name: "3.9 Duplicate bucket name",
    fn: async () => {
      const dupName = `e2e-03dup-${Date.now()}`.slice(0, 63);
      const maxCapacity = 1_048_576n;
      const duration = 100;
      await createS3BucketWithStorage(api, PROVIDER_URL, client, provider, dupName, {
        maxBytes: maxCapacity,
        duration,
      });
      // A second create with the same name fails atomically — even with a
      // fresh, valid signed quote.
      const signed = await negotiateSigned(api, PROVIDER_URL, client, provider, {
        maxBytes: maxCapacity,
        duration,
      });
      const tx = api.tx.S3Registry.create_s3_bucket({
        name: new TextEncoder().encode(dupName),
        ...buildSignedTermsArgs(provider, signed),
      });
      await submitTxExpectFailure(tx, client.signer, "BucketNameExists", "3.9");
    },
  });

  // ── Edge cases ────────────────────────────────────────────────────────────

  tests.push({
    name: "3.10 Object key with path separators",
    fn: async () => {
      const edgeName = `e2e-03edge-${Date.now()}`.slice(0, 63);
      const maxCapacity = 1_048_576n;
      const duration = 100;
      const { s3BucketId: bid, layer0BucketId: l0 } = await createS3BucketWithStorage(
        api,
        PROVIDER_URL,
        client,
        provider,
        edgeName,
        { maxBytes: maxCapacity, duration }
      );
      const obj = await putChunk(PROVIDER_URL, l0, "deep nested");
      await putObjectMetadata(api, client, bid, "a/b/c/d.txt", obj, "text/plain");
      const stored = await api.query.S3Registry.Objects.getValue(
        bid,
        new TextEncoder().encode("a/b/c/d.txt"),
        READ_OPTS
      );
      assert.ok(stored, "Object with nested path key should exist");
      // Cleanup
      await deleteObjectMetadata(api, client, bid, "a/b/c/d.txt");
      await deleteS3Bucket(api, client, bid);
    },
  });

  tests.push({
    name: "3.11 Overwrite existing key (upsert)",
    fn: async () => {
      const upsertName = `e2e-03ups-${Date.now()}`.slice(0, 63);
      const maxCapacity = 1_048_576n;
      const duration = 100;
      const { s3BucketId: bid, layer0BucketId: l0 } = await createS3BucketWithStorage(
        api,
        PROVIDER_URL,
        client,
        provider,
        upsertName,
        { maxBytes: maxCapacity, duration }
      );
      const obj1 = await putChunk(PROVIDER_URL, l0, "version 1");
      await putObjectMetadata(api, client, bid, "file.txt", obj1, "text/plain");
      const obj2 = await putChunk(PROVIDER_URL, l0, "version 2 updated");
      await putObjectMetadata(api, client, bid, "file.txt", obj2, "text/plain");
      const stored = await api.query.S3Registry.Objects.getValue(
        bid,
        new TextEncoder().encode("file.txt"),
        READ_OPTS
      );
      assert.strictEqual(stored.size, obj2.size, "Size should reflect the upserted object");
      // Cleanup
      await deleteObjectMetadata(api, client, bid, "file.txt");
      await deleteS3Bucket(api, client, bid);
    },
  });

  await runSuite("03 — S3 Bucket and Objects", tests, { api, papi });

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
