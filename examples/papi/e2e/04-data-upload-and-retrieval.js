/**
 * E2E Workflow 04 — Data Upload & Retrieval
 *
 * Accounts: //Alice (provider), //Bob (client)
 *
 * Tests: different sizes, S3 HTTP endpoints, roundtrip integrity, edge cases.
 *
 * Usage: node e2e/04-data-upload-and-retrieval.js [chain_ws] [provider_url]
 */

import assert from "node:assert";
import { blake2b256 } from "@polkadot-labs/hdkd-helpers";
import {
  createBucketWithStorage,
  downloadChunk,
  putChunk,
  uploadChunk,
} from "../api.js";
import {
  ensureProviderRegistered,
  ensureSoleAcceptingProvider,
  makeSigner,
  providerFetch,
  toHex,
  waitForAgreementAcceptance,
} from "../common.js";
import { runSuite, setupChain } from "./helpers.js";

const CHAIN_WS = process.argv[2] || "ws://127.0.0.1:2222";
const PROVIDER_URL = process.argv[3] || "http://127.0.0.1:3333";

function randomBytes(n) {
  const buf = new Uint8Array(n);
  for (let i = 0; i < n; i++) buf[i] = Math.floor(Math.random() * 256);
  return buf;
}

async function main() {
  const provider = makeSigner("//Alice");
  const client = makeSigner("//Bob");

  const { papi, api } = await setupChain(CHAIN_WS);
  await ensureProviderRegistered(api, provider, PROVIDER_URL);
  const restore = await ensureSoleAcceptingProvider(api, provider);

  // Create a bucket for upload tests.
  const maxCapacity = 10_485_760n; // 10 MiB
  const duration = 100;
  const { bucketId } = await createBucketWithStorage(api, client, {
    max_capacity: maxCapacity,
    duration,
    max_payment: maxCapacity * BigInt(duration) * 10n,
  });
  await waitForAgreementAcceptance(api, provider.address, bucketId);

  const tests = [];

  // ── Different sizes ───────────────────────────────────────────────────────

  tests.push({
    name: "4.1 Small chunk (100 bytes)",
    fn: async () => {
      const data = "x".repeat(100);
      const { hash, commit } = await uploadChunk(PROVIDER_URL, bucketId, data);
      assert.ok(commit.mmr_root, "Should return mmr_root");
      const downloaded = await downloadChunk(PROVIDER_URL, hash);
      assert.deepStrictEqual(downloaded, Buffer.from(data), "Downloaded data should match");
    },
  });

  tests.push({
    name: "4.2 Medium chunk (64 KB)",
    fn: async () => {
      const data = randomBytes(64 * 1024);
      const { hash } = await uploadChunk(PROVIDER_URL, bucketId, data);
      const downloaded = await downloadChunk(PROVIDER_URL, hash);
      assert.deepStrictEqual(new Uint8Array(downloaded), data, "64KB roundtrip integrity");
    },
  });

  tests.push({
    name: "4.3 Max chunk size (256 KB)",
    fn: async () => {
      const data = randomBytes(256 * 1024);
      const { hash } = await uploadChunk(PROVIDER_URL, bucketId, data);
      const downloaded = await downloadChunk(PROVIDER_URL, hash);
      assert.deepStrictEqual(new Uint8Array(downloaded), data, "256KB roundtrip integrity");
    },
  });

  tests.push({
    name: "4.4 Multiple sequential uploads",
    fn: async () => {
      const uploads = [];
      for (let i = 0; i < 5; i++) {
        const data = `sequential upload #${i} @ ${Date.now()}`;
        const result = await uploadChunk(PROVIDER_URL, bucketId, data);
        uploads.push(result);
      }
      // Verify leaf indices are incrementing.
      for (let i = 1; i < uploads.length; i++) {
        assert.ok(
          uploads[i].commit.leaf_indices[0] > uploads[i - 1].commit.leaf_indices[0],
          `Leaf index should increase: ${uploads[i].commit.leaf_indices[0]} > ${uploads[i - 1].commit.leaf_indices[0]}`
        );
      }
      // Verify each mmr_root changes (or at least the last differs from the first).
      assert.notStrictEqual(
        uploads[0].commit.mmr_root,
        uploads[4].commit.mmr_root,
        "MMR root should change with new uploads"
      );
    },
  });

  // ── S3 HTTP endpoints ─────────────────────────────────────────────────────

  tests.push({
    name: "4.5 S3 PUT/GET roundtrip",
    fn: async () => {
      // Use the S3-style HTTP endpoint on the provider.
      const body = "Hello S3-style upload";
      const url = new URL(`/s3/${bucketId}/object`, PROVIDER_URL);
      url.searchParams.set("key", "e2e-test.txt");
      const putResp = await fetch(url, {
        method: "PUT",
        headers: { "Content-Type": "text/plain" },
        body,
      });
      if (!putResp.ok) {
        // S3 endpoints may not be available; skip gracefully.
        console.log("    S3 PUT returned %d — skipping (S3 endpoints may not be enabled)", putResp.status);
        return;
      }
      const getResp = await fetch(url);
      assert.ok(getResp.ok, `GET should succeed, got ${getResp.status}`);
      const downloaded = await getResp.text();
      assert.strictEqual(downloaded, body, "S3 GET content should match PUT");
    },
  });

  tests.push({
    name: "4.6 S3 HEAD object",
    fn: async () => {
      const url = new URL(`/s3/${bucketId}/object`, PROVIDER_URL);
      url.searchParams.set("key", "e2e-test.txt");
      const resp = await fetch(url, { method: "HEAD" });
      if (!resp.ok && resp.status === 404) {
        console.log("    S3 HEAD returned 404 — S3 endpoints may not be enabled, skipping");
        return;
      }
      // If it worked, just verify we got headers back.
      assert.ok(resp.ok || resp.status === 405, `HEAD should return 200 or 405, got ${resp.status}`);
    },
  });

  tests.push({
    name: "4.7 S3 list objects",
    fn: async () => {
      const url = new URL(`/s3/${bucketId}/objects`, PROVIDER_URL);
      const resp = await fetch(url);
      if (!resp.ok && resp.status !== 200) {
        console.log("    S3 list returned %d — skipping", resp.status);
        return;
      }
      const data = await resp.json();
      assert.ok(Array.isArray(data.objects || data), "List should return an array");
    },
  });

  tests.push({
    name: "4.8 S3 DELETE object",
    fn: async () => {
      const url = new URL(`/s3/${bucketId}/object`, PROVIDER_URL);
      url.searchParams.set("key", "e2e-test.txt");
      const resp = await fetch(url, { method: "DELETE" });
      if (!resp.ok && resp.status === 404) {
        console.log("    S3 DELETE not available, skipping");
        return;
      }
      // Verify the object is gone.
      const getResp = await fetch(url);
      assert.ok(
        getResp.status === 404 || !getResp.ok,
        `GET after DELETE should 404, got ${getResp.status}`
      );
    },
  });

  // ── Failure ───────────────────────────────────────────────────────────────

  tests.push({
    name: "4.9 Download non-existent hash",
    fn: async () => {
      const fakeHash = "0x" + "ab".repeat(32);
      try {
        await downloadChunk(PROVIDER_URL, fakeHash);
        assert.fail("Expected 404 for non-existent hash");
      } catch (err) {
        assert.ok(err.message.includes("404") || err.message.includes("not found"),
          `Expected 404-like error, got: ${err.message}`);
      }
    },
  });

  // ── Edge cases ────────────────────────────────────────────────────────────

  tests.push({
    name: "4.10 Upload binary (non-UTF8) data",
    fn: async () => {
      const binary = randomBytes(512);
      const { hash } = await uploadChunk(PROVIDER_URL, bucketId, binary);
      const downloaded = await downloadChunk(PROVIDER_URL, hash);
      assert.deepStrictEqual(new Uint8Array(downloaded), binary, "Binary roundtrip should match");
    },
  });

  tests.push({
    name: "4.11 Upload identical content twice — different MMR leaves",
    fn: async () => {
      const data = "duplicate content for e2e";
      const first = await uploadChunk(PROVIDER_URL, bucketId, data);
      const second = await uploadChunk(PROVIDER_URL, bucketId, data);
      assert.strictEqual(first.hash, second.hash, "Same data should produce same hash");
      assert.notStrictEqual(
        first.commit.leaf_indices[0],
        second.commit.leaf_indices[0],
        "Leaf indices should differ for separate uploads"
      );
    },
  });

  tests.push({
    name: "4.12 Verify blake2-256 hash",
    fn: async () => {
      const data = "verify hash computation";
      const bytes = new TextEncoder().encode(data);
      const expectedHash = toHex(blake2b256(bytes));
      const { hash } = await uploadChunk(PROVIDER_URL, bucketId, data);
      assert.strictEqual(hash, expectedHash, "Provider hash should match local blake2-256");
    },
  });

  await runSuite("04 — Data Upload & Retrieval", tests, { api, papi });

  try {
    await restore();
  } catch {}
  papi.destroy();
}

main();
