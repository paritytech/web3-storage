// SPDX-License-Identifier: Apache-2.0

/**
 * E2E Workflow 03 — S3 Bucket and Objects
 *
 * Accounts: //Alice (provider), //Bob (owner), //Charlie (member)
 *
 * Tests: S3 object CRUD on a plain Layer 0 bucket through `Web3Storage`, member
 * writes, and failure cases. The chain stores no bucket name and no object
 * metadata: the client keeps the key -> content tree in the bucket itself and
 * uses the provider's Layer 0 routes only.
 *
 * Usage: node e2e/03-s3-bucket-and-objects.js [chain_ws] [provider_url]
 */

import assert from "node:assert";
import {
  ensureProviderRegistered,
  makeSigner,
  sameAddress,
  setMember,
  type Objects,
} from "@web3-storage/sdk";
import { ensureSoleAcceptingProvider } from "../support.js";
import { eventually, INSUFFICIENT_ROLE, runSuite, setupChain, storageFor } from "./helpers.js";

const CHAIN_WS = process.argv[2] || "ws://127.0.0.1:2222";
const PROVIDER_URL = process.argv[3] || "http://127.0.0.1:3333";

const enc = (s: string) => new TextEncoder().encode(s);
const dec = (b: Uint8Array) => new TextDecoder().decode(b);

async function main() {
  const provider = makeSigner("//Alice");
  const owner = makeSigner("//Bob");
  const member = makeSigner("//Charlie");

  const { papi, api } = await setupChain(CHAIN_WS);
  await ensureProviderRegistered(api, provider, PROVIDER_URL);
  const restore = await ensureSoleAcceptingProvider(api, provider);

  const ownerStorage = storageFor(api, owner, PROVIDER_URL);
  const memberStorage = storageFor(api, member, PROVIDER_URL);
  const createOpts = {
    maxCapacity: 1_048_576n,
    duration: 100,
    provider: { address: provider.address, url: PROVIDER_URL },
  };

  let bucketId: bigint;
  /** The bucket's objects as the owner and as the member, opened by 3.1. */
  let ownerS3: Objects;
  let memberS3: Objects;

  const tests: Array<{ name: string; fn: () => Promise<void> }> = [];

  // ── Success ───────────────────────────────────────────────────────────────

  tests.push({
    name: "3.1 Create bucket",
    fn: async () => {
      const result = await ownerStorage.createBucket(createOpts);
      bucketId = result.bucketId;
      ownerS3 = await ownerStorage.objects(bucketId);
      memberS3 = await memberStorage.objects(bucketId);
      assert.ok(sameAddress(result.provider, provider.address), "provider should be Alice");

      const info = await ownerStorage.getBucket(bucketId);
      assert.ok(info, "getBucket should find the new bucket");
      assert.ok(
        info.members.some((m) => sameAddress(m.account, owner.address) && m.role === "Admin"),
        "creator should be the bucket's Admin"
      );
      assert.ok(
        info.providerInfo.some((p) => sameAddress(p.account, provider.address)),
        "negotiated provider should be the bucket's primary"
      );
      assert.strictEqual(info.maxCapacity, createOpts.maxCapacity, "maxCapacity should match the terms");

      const listed = await ownerStorage.listBuckets();
      assert.ok(
        listed.some((b) => b.bucketId === bucketId),
        "listBuckets should include the new bucket"
      );
    },
  });

  tests.push({
    name: "3.2 Put and get object",
    fn: async () => {
      const put = await ownerS3.putObject("test.txt", enc("hello from e2e test"), {
        contentType: "text/plain",
      });
      assert.strictEqual(put.size, "hello from e2e test".length, "put size should match");
      const got = await ownerS3.getObject("test.txt");
      assert.strictEqual(dec(got.data), "hello from e2e test", "object bytes should round-trip");
      assert.ok(got.contentType.startsWith("text/plain"), `content type: ${got.contentType}`);
    },
  });

  tests.push({
    name: "3.3 Put with user metadata",
    fn: async () => {
      await ownerS3.putObject("meta.txt", enc("data with metadata"), {
        contentType: "text/plain",
        metadata: { author: "e2e-test", version: "1" },
      });
      const got = await ownerS3.getObject("meta.txt");
      assert.strictEqual(dec(got.data), "data with metadata");
      assert.deepStrictEqual(got.metadata, { author: "e2e-test", version: "1" }, "user metadata should round-trip");
      const head = await ownerS3.headObject("meta.txt");
      assert.strictEqual(head.size, "data with metadata".length, "head size");
      assert.strictEqual(head.etag, got.etag, "head and get should report the same etag");
    },
  });

  tests.push({
    name: "3.4 List objects (all and by prefix)",
    fn: async () => {
      await ownerS3.putObject("docs/readme.md", enc("# readme"));
      const all = await ownerS3.listObjects();
      for (const key of ["test.txt", "meta.txt", "docs/readme.md"]) {
        assert.ok(all.objects.some((o) => o.key === key), `listObjects should include ${key}`);
      }
      const docs = await ownerS3.listObjects({ prefix: "docs/" });
      assert.deepStrictEqual(
        docs.objects.map((o) => o.key),
        ["docs/readme.md"],
        "prefix listing should return only docs/ keys"
      );
      const top = await ownerS3.listObjects({ delimiter: "/" });
      assert.deepStrictEqual(top.commonPrefixes, ["docs/"], "delimiter listing should group docs/ keys");
      assert.ok(!top.objects.some((o) => o.key.includes("/")), "delimiter listing should not return nested keys");
    },
  });

  tests.push({
    name: "3.5 Delete object",
    fn: async () => {
      await ownerS3.deleteObject("meta.txt");
      const after = await ownerS3.listObjects();
      assert.ok(!after.objects.some((o) => o.key === "meta.txt"), "deleted key should not be listed");
      assert.ok(after.objects.some((o) => o.key === "test.txt"), "other keys should remain");
      await assert.rejects(ownerS3.getObject("meta.txt"), { code: "ObjectNotFound" }, "deleted key should not be readable");
    },
  });

  tests.push({
    name: "3.6 Writer member puts an object",
    fn: async () => {
      await setMember(api, owner, bucketId, member, "Writer", { mode: "finalized" });
      await eventually(
        async () => memberS3.putObject("from-member.txt", enc("member write")),
        "Writer put"
      );
      const got = await ownerS3.getObject("from-member.txt");
      assert.strictEqual(dec(got.data), "member write");
      const memberBuckets = await memberStorage.listBuckets();
      assert.ok(
        memberBuckets.some((b) => b.bucketId === bucketId),
        "listBuckets for the member should include the shared bucket"
      );
    },
  });

  // ── Failure ───────────────────────────────────────────────────────────────

  tests.push({
    name: "3.7 Get non-existent key",
    fn: async () => {
      await assert.rejects(
        ownerS3.getObject("does-not-exist.txt"),
        { code: "ObjectNotFound" },
        "missing key should fail"
      );
    },
  });

  tests.push({
    name: "3.8 Non-member cannot put",
    fn: async () => {
      // A second bucket where Charlie holds no role.
      const { bucketId: other } = await ownerStorage.createBucket(createOpts);
      const intruder = await memberStorage.objects(other);
      await assert.rejects(
        intruder.putObject("intruder.txt", enc("nope")),
        INSUFFICIENT_ROLE,
        "a non-member put should be refused"
      );
    },
  });

  tests.push({
    name: "3.9 Empty object key is rejected client-side",
    fn: async () => {
      await assert.rejects(ownerS3.putObject("", enc("x")), { code: "InvalidObjectKey" });
    },
  });

  // ── Edge cases ────────────────────────────────────────────────────────────

  tests.push({
    name: "3.10 Object key with path separators",
    fn: async () => {
      await ownerS3.putObject("a/b/c/d.txt", enc("deep nested"));
      const got = await ownerS3.getObject("a/b/c/d.txt");
      assert.strictEqual(dec(got.data), "deep nested");
    },
  });

  tests.push({
    name: "3.11 Overwrite existing key (upsert)",
    fn: async () => {
      await ownerS3.putObject("file.txt", enc("version 1"));
      await ownerS3.putObject("file.txt", enc("version 2 updated"));
      const got = await ownerS3.getObject("file.txt");
      assert.strictEqual(dec(got.data), "version 2 updated", "get should return the last write");
      const listed = (await ownerS3.listObjects()).objects.filter((o) => o.key === "file.txt");
      assert.strictEqual(listed.length, 1, "the key should be listed once");
      assert.strictEqual(listed[0]!.size, "version 2 updated".length, "size should reflect the last write");
    },
  });

  await runSuite("03 — S3 Bucket and Objects", tests, { api, papi });

  try {
    await restore();
  } catch {}
  papi.destroy();
}

main()
  .catch((err) => {
    console.error(err);
    process.exitCode = 1;
  })
  .finally(() => {
    process.exit(process.exitCode || 0);
  });
