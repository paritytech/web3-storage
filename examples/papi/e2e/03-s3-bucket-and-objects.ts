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
  copyObjectMetadata,
  ensureProviderRegistered,
  hexToBytes,
  makeSigner,
  putChunk,
  READ_OPTS,
  sameAddress,
  waitForAgreementAcceptance,
} from "@web3-storage/sdk";
import { S3Client } from "@web3-storage/sdk/s3";
import { ensureSoleAcceptingProvider } from "../support.js";
import { runSuite, submitTxExpectFailure, setupChain } from "./helpers.js";

const CHAIN_WS = process.argv[2] || "ws://127.0.0.1:2222";
const PROVIDER_URL = process.argv[3] || "http://127.0.0.1:3333";

async function main() {
  const provider = makeSigner("//Alice");
  const client = makeSigner("//Bob");

  const { papi, api } = await setupChain(CHAIN_WS);
  await ensureProviderRegistered(api, provider, PROVIDER_URL);
  const restore = await ensureSoleAcceptingProvider(api, provider);

  // The suite drives the S3 surface through the layer-1 client (chain ops
  // silent-by-default, object HTTP signed + verified); putChunk keeps one
  // raw layer-0 upload path covered in 3.2.
  const s3 = new S3Client({
    api,
    signer: client,
    providerUrl: PROVIDER_URL,
    // Test semantics: in-block submission + best-block reads (READ_OPTS).
    readOpts: READ_OPTS,
    submitMode: "best",
  });

  let s3BucketId: bigint, layer0BucketId: bigint;
  const bucketName = `e2e-03-${Date.now()}`.slice(0, 63);

  const tests: Array<{ name: string; fn: () => Promise<void> }> = [];

  const mkBucket = async (name: string) => {
    const maxCapacity = 1_048_576n;
    const duration = 100;
    const r = await s3.createBucketWithStorage(name, {
      maxCapacity,
      duration,
      maxPayment: maxCapacity * BigInt(duration) * 10n,
    });
    await waitForAgreementAcceptance(api, provider.address, r.layer0BucketId);
    return r;
  };

  // ── Success ───────────────────────────────────────────────────────────────

  tests.push({
    name: "3.1 Create S3 bucket with storage",
    fn: async () => {
      const maxCapacity = 1_048_576n;
      const duration = 100;
      const result = await s3.createBucketWithStorage(bucketName, {
        maxCapacity,
        duration,
        maxPayment: maxCapacity * BigInt(duration) * 10n,
      });
      s3BucketId = result.s3BucketId;
      layer0BucketId = result.layer0BucketId;
      assert.ok(s3BucketId !== undefined, "s3_bucket_id should be returned");
      assert.ok(layer0BucketId !== undefined, "layer0_bucket_id should be returned");
      assert.ok(
        sameAddress(result.matchedProvider!, provider.address),
        "Should match expected provider"
      );
      await waitForAgreementAcceptance(api, provider.address, layer0BucketId);
    },
  });

  tests.push({
    name: "3.2 Put object metadata",
    fn: async () => {
      const obj = await putChunk(PROVIDER_URL, layer0BucketId, "hello from e2e test");
      await s3.putObjectMetadata(s3BucketId, "test.txt", obj, "text/plain");
      const stored = await s3.getObjectMetadata(s3BucketId, "test.txt");
      assert.ok(stored, "Object should exist in storage");
      assert.strictEqual(stored.size, obj.size, "Size should match");
    },
  });

  tests.push({
    name: "3.3 Put with user metadata",
    fn: async () => {
      const obj = await putChunk(PROVIDER_URL, layer0BucketId, "data with metadata");
      await s3.putObjectMetadata(s3BucketId, "meta.txt", obj, "text/plain", [
        ["author", "e2e-test"],
        ["version", "1"],
      ]);
      const stored = await s3.getObjectMetadata(s3BucketId, "meta.txt");
      assert.ok(stored, "Object with metadata should exist");
      assert.strictEqual(stored.userMetadata.author, "e2e-test", "User metadata round-trips");
    },
  });

  tests.push({
    name: "3.4 Copy object metadata",
    fn: async () => {
      await copyObjectMetadata(api, client, s3BucketId, "test.txt", s3BucketId, "test-copy.txt");
      const original = await s3.getObjectMetadata(s3BucketId, "test.txt");
      const copy = await s3.getObjectMetadata(s3BucketId, "test-copy.txt");
      assert.ok(copy, "Copy should exist");
      assert.deepStrictEqual(copy.cid, original!.cid, "CID should be the same after copy");
    },
  });

  tests.push({
    name: "3.5 Delete object metadata",
    fn: async () => {
      await s3.deleteObjectMetadata(s3BucketId, "meta.txt");
      const stored = await s3.getObjectMetadata(s3BucketId, "meta.txt");
      assert.strictEqual(stored, null, "Object should be gone after delete");
      const bucketInfo = await s3.headBucket(bucketName);
      assert.ok(bucketInfo, "Bucket should still exist");
    },
  });

  tests.push({
    name: "3.6 Delete S3 bucket (after removing all objects)",
    fn: async () => {
      // Remove remaining objects.
      await s3.deleteObjectMetadata(s3BucketId, "test.txt");
      await s3.deleteObjectMetadata(s3BucketId, "test-copy.txt");
      await s3.deleteBucket(s3BucketId);
      const after = await s3.headBucket(bucketName);
      assert.strictEqual(after, null, "Bucket should be gone");
    },
  });

  // ── Failure ───────────────────────────────────────────────────────────────

  tests.push({
    name: "3.7 Delete non-empty bucket",
    fn: async () => {
      // Create a new bucket for this test.
      const { s3BucketId: bid, layer0BucketId: l0 } = await mkBucket(
        `e2e-03b-${Date.now()}`.slice(0, 63),
      );
      const obj = await putChunk(PROVIDER_URL, l0, "not empty");
      await s3.putObjectMetadata(bid, "file.txt", obj, "text/plain");
      const tx = api.tx.S3Registry.delete_s3_bucket({ s3_bucket_id: bid });
      await submitTxExpectFailure(tx, client.signer, "BucketNotEmpty", "3.7");
      // Cleanup
      await s3.deleteObjectMetadata(bid, "file.txt");
      await s3.deleteBucket(bid);
    },
  });

  tests.push({
    name: "3.8 Delete non-existent object",
    fn: async () => {
      const { s3BucketId: bid } = await mkBucket(
        `e2e-03c-${Date.now()}`.slice(0, 63),
      );
      const tx = api.tx.S3Registry.delete_object_metadata({
        s3_bucket_id: bid,
        key: new TextEncoder().encode("does-not-exist.txt"),
      });
      await submitTxExpectFailure(tx, client.signer, "ObjectNotFound", "3.8");
      // Cleanup
      await s3.deleteBucket(bid);
    },
  });

  tests.push({
    name: "3.9 Duplicate bucket name",
    fn: async () => {
      const dupName = `e2e-03dup-${Date.now()}`.slice(0, 63);
      await mkBucket(dupName);
      const tx = api.tx.S3Registry.create_s3_bucket_with_storage({
        name: new TextEncoder().encode(dupName),
        max_capacity: 1_048_576n,
        duration: 100,
        max_payment: 1_048_576n * 100n * 10n,
      });
      await submitTxExpectFailure(tx, client.signer, "BucketNameExists", "3.9");
    },
  });

  // ── Edge cases ────────────────────────────────────────────────────────────

  tests.push({
    name: "3.10 Object key with path separators",
    fn: async () => {
      const { s3BucketId: bid, layer0BucketId: l0 } = await mkBucket(
        `e2e-03edge-${Date.now()}`.slice(0, 63),
      );
      const obj = await putChunk(PROVIDER_URL, l0, "deep nested");
      await s3.putObjectMetadata(bid, "a/b/c/d.txt", obj, "text/plain");
      const stored = await s3.getObjectMetadata(bid, "a/b/c/d.txt");
      assert.ok(stored, "Object with nested path key should exist");
      // Cleanup
      await s3.deleteObjectMetadata(bid, "a/b/c/d.txt");
      await s3.deleteBucket(bid);
    },
  });

  tests.push({
    name: "3.11 Overwrite existing key (upsert)",
    fn: async () => {
      const { s3BucketId: bid, layer0BucketId: l0 } = await mkBucket(
        `e2e-03ups-${Date.now()}`.slice(0, 63),
      );
      const obj1 = await putChunk(PROVIDER_URL, l0, "version 1");
      await s3.putObjectMetadata(bid, "file.txt", obj1, "text/plain");
      const obj2 = await putChunk(PROVIDER_URL, l0, "version 2 updated");
      await s3.putObjectMetadata(bid, "file.txt", obj2, "text/plain");
      const stored = await s3.getObjectMetadata(bid, "file.txt");
      assert.strictEqual(stored!.size, obj2.size, "Size should reflect the upserted object");
      // Cleanup
      await s3.deleteObjectMetadata(bid, "file.txt");
      await s3.deleteBucket(bid);
    },
  });

  tests.push({
    name: "3.12 S3Client object HTTP round-trip (put/get/list/delete + CID verification)",
    fn: async () => {
      const { s3BucketId: bid, layer0BucketId: l0 } = await mkBucket(
        `e2e-03http-${Date.now()}`.slice(0, 63),
      );
      const bucket = { s3BucketId: bid, layer0BucketId: l0 };
      const body = new TextEncoder().encode(`round-trip ${Date.now()}`);

      const put = await s3.putObject(bucket, "rt.txt", body, {
        contentType: "text/plain",
        metadata: { origin: "e2e-03" },
      });
      assert.ok(put.cid, "putObject should echo a data_root cid");

      // Anchor the cid on chain so getObject can verify against it.
      await s3.putObjectMetadata(bid, "rt.txt", { cid: hexToBytes(put.cid!), size: BigInt(body.length) }, "text/plain");

      const got = await s3.getObject(bucket, "rt.txt");
      assert.strictEqual(got.verified, true, "single-chunk download should verify against the on-chain cid");
      assert.deepStrictEqual(got.data, body, "downloaded bytes should match upload");

      const listed = await s3.listObjects(bucket);
      assert.ok(listed.some((o) => o.key === "rt.txt"), "listObjects should contain the key");

      await s3.deleteObject(bucket, "rt.txt");
      await s3.deleteObjectMetadata(bid, "rt.txt");
      await s3.deleteBucket(bid);
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
