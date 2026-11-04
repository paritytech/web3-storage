// SPDX-License-Identifier: Apache-2.0

/**
 * Cross-language check of the in-bucket file-system format. The Rust test
 * `clients/file-system/src/s3/tests/fixtures.rs` runs a write sequence and writes the
 * bucket to `.test-vectors/fs-bucket.json` (`just fs-fixture`). This test
 * loads that bucket into a fake provider and reads it, then runs the same
 * write sequence and checks that it produces the same blobs and commits.
 * See the Rust module for the fixture format. Skipped locally if the file is
 * missing; required in CI.
 */

import { existsSync, readFileSync } from "node:fs";
import { afterEach, describe, expect, it, vi } from "vitest";
import {
  base64ToBytes,
  bytesToBase64,
  computeCid,
  computeDataRoot,
  DEFAULT_CHUNK_SIZE,
  hashLeaf,
  splitChunks,
  toHex,
} from "@web3-storage/core";
import { commitDataRoots, makeSigner, readBlob, uploadBlob } from "@web3-storage/layer0";

import { FakeProvider } from "@web3-storage/layer0/testing";
import { S3Client } from "./s3/client.js";
import { FsTree, providerBlobStore } from "./tree.js";

const PROVIDER = "http://provider.test";
const BUCKET = 7n;
const MTIME = 1_700_000_000n;
const GENERATOR = "byte i is i % 251";
const BIG_SIZE = 3 * DEFAULT_CHUNK_SIZE + 100;
/** Hashing the multi-chunk file several times in JS takes a few seconds. */
const TIMEOUT_MS = 60_000;
const FIXTURE = new URL("../../../.test-vectors/fs-bucket.json", import.meta.url);
const fixtureMissing = !process.env.CI && !existsSync(FIXTURE);

const signer = makeSigner("//Alice");
const te = new TextEncoder();

type Json = null | boolean | number | string | Json[] | { [key: string]: Json };

interface Fixture {
  bucket_id: number;
  blobs: Record<string, string>;
  generated_blobs: Array<{ cid: string; size: number; generator: string; chunk_hashes: string[] }>;
  commits: string[][];
  expected: { root_cid: string; [key: string]: Json };
}

function generate(size: number): Uint8Array {
  return Uint8Array.from({ length: size }, (_, i) => i % 251);
}

function setup(provider: FakeProvider) {
  const http = { fetch: provider.fetch };
  const tree = new FsTree(providerBlobStore(PROVIDER, BUCKET, signer, http), () => MTIME);
  const s3 = new S3Client({ api: {} as never, signer, providerUrl: PROVIDER, fetch: provider.fetch });
  return { tree, s3 };
}

/** The write sequence of `write_sequence` in clients/file-system/src/s3/tests/fixtures.rs. */
async function writeSequence(provider: FakeProvider): Promise<void> {
  const { tree, s3 } = setup(provider);
  await tree.mkdir("/docs");
  await tree.putFile("/docs/a.txt", te.encode("hello from a.txt\n"), { contentType: "text/plain" });
  await tree.putFile("/docs/old.txt", te.encode("deleted later"), { contentType: "text/plain" });
  await tree.putFile("/big.bin", generate(BIG_SIZE));
  await tree.putFile("/empty", new Uint8Array(0));
  // S3Client uses the system clock for mtime.
  vi.useFakeTimers({ toFake: ["Date"], now: Number(MTIME) * 1000 });
  try {
    await s3.putObject(BUCKET, "photos/cat.jpg", te.encode("meow"), {
      contentType: "image/jpeg",
      metadata: { Zeta: "last", alpha: "first", "Camera-Model": "X100" },
    });
  } finally {
    vi.useRealTimers();
  }
  await tree.delete("/docs/old.txt");
}

/** What a reader of the bucket sees; same shape as `expected` in the Rust test. */
async function expected(provider: FakeProvider): Promise<Json> {
  const { tree, s3 } = setup(provider);
  const listings: Record<string, Json> = {};
  for (const dir of ["/", "/docs", "/photos"]) {
    listings[dir] = (await tree.list(dir)).map((e) => ({
      name: e.name,
      entry_type: e.entryType,
      cid: e.cid,
      size: Number(e.size),
      mtime: Number(e.mtime),
    }));
  }
  const files: Record<string, Json> = {};
  for (const path of ["/big.bin", "/docs/a.txt", "/empty", "/photos/cat.jpg"]) {
    const file = await tree.getFile(path);
    files[path] = {
      size: Number(file.size),
      content_type: file.contentType,
      leaf_hash: toHex(hashLeaf(file.bytes)),
      content_root: file.contentRoot,
      manifest_cid: file.manifestCid,
      mtime: Number(file.mtime),
      user_metadata: Object.entries(file.userMetadata).map(([k, v]) => [k, v]),
    };
  }
  const object = await s3.getObject(BUCKET, "photos/cat.jpg");
  const { data, key: _key, ...head } = object;
  expect(await s3.headObject(BUCKET, "photos/cat.jpg")).toEqual({ key: "photos/cat.jpg", ...head });
  return {
    root_cid: await tree.rootCid(),
    listings,
    files,
    objects: {
      "photos/cat.jpg": {
        etag: object.etag,
        content_type: object.contentType,
        size: object.size,
        last_modified: object.lastModified / 1000,
        metadata: object.metadata,
        leaf_hash: toHex(hashLeaf(data)),
      },
    },
  };
}

/** The blobs and commits of the bucket in `provider`, in the fixture format. */
async function written(provider: FakeProvider): Promise<Pick<Fixture, "blobs" | "generated_blobs" | "commits">> {
  const commits: string[][] = provider.calls
    .filter((c) => c.method === "POST" && c.path === "/commit")
    .map((c) => (c.body.data_roots as string[]).map((r) => r.toLowerCase()));
  const blobs: Record<string, string> = {};
  const generated: Fixture["generated_blobs"] = [];
  for (const cid of [...new Set(commits.flat())].sort()) {
    const bytes = await readBlob(PROVIDER, BUCKET, cid, { fetch: provider.fetch });
    if (bytes.length > DEFAULT_CHUNK_SIZE) {
      expect(bytes, "only generated blobs span chunks").toEqual(generate(bytes.length));
      generated.push({
        cid,
        size: bytes.length,
        generator: GENERATOR,
        chunk_hashes: splitChunks(bytes).map((c) => toHex(computeCid(c))),
      });
    } else {
      blobs[cid] = bytesToBase64(bytes);
    }
  }
  return { blobs, generated_blobs: generated, commits };
}

/**
 * A fake provider with the fixture's blobs uploaded and its commits made in
 * order. Checks each blob's CID and each generated blob's chunk hashes.
 */
async function load(f: Fixture): Promise<FakeProvider> {
  expect(BigInt(f.bucket_id)).toBe(BUCKET);
  const provider = new FakeProvider();
  const http = { fetch: provider.fetch };
  const upload = async (cid: string, bytes: Uint8Array) => {
    const { dataRoot } = await uploadBlob(PROVIDER, BUCKET, bytes, signer, http);
    expect(dataRoot.toLowerCase()).toBe(cid);
  };
  for (const [cid, b64] of Object.entries(f.blobs)) await upload(cid, base64ToBytes(b64));
  for (const blob of f.generated_blobs) {
    expect(blob.generator).toBe(GENERATOR);
    const bytes = generate(blob.size);
    expect(splitChunks(bytes).map((c) => toHex(computeCid(c)))).toEqual(blob.chunk_hashes);
    expect(toHex(computeDataRoot(bytes))).toBe(blob.cid);
    await upload(blob.cid, bytes);
  }
  for (const roots of f.commits) await commitDataRoots(PROVIDER, BUCKET, roots, signer, http);
  return provider;
}

describe("cross-language fixture", () => {
  afterEach(() => vi.useRealTimers());

  it.skipIf(fixtureMissing)(
    "reads the bucket the Rust client wrote and writes the same bucket",
    async () => {
      expect(existsSync(FIXTURE), "FS fixture missing; run `just fs-fixture`").toBe(true);
      const f = JSON.parse(readFileSync(FIXTURE, "utf8")) as Fixture;
      const provider = await load(f);
      const lastCommit = f.commits[f.commits.length - 1];
      expect(f.expected.root_cid).toBe(lastCommit[lastCommit.length - 1]);
      expect(await expected(provider)).toEqual(f.expected);

      // Both languages write the same bucket for the same operations.
      const ts = new FakeProvider();
      await writeSequence(ts);
      expect(await written(ts)).toEqual({ blobs: f.blobs, generated_blobs: f.generated_blobs, commits: f.commits });
      expect(await expected(ts)).toEqual(f.expected);
    },
    TIMEOUT_MS,
  );
});
