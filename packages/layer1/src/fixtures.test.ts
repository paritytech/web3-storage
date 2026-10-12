// SPDX-License-Identifier: Apache-2.0

/**
 * Cross-language fixtures for the in-bucket file-system format, in
 * `crates/primitives/file-system/fixtures/`. `rust-written.json` comes from
 * `clients/s3/src/fixtures.rs`; this test loads it into a fake provider and
 * reads it. It also runs the same write sequence and checks the result
 * against `ts-written.json`, which the Rust test reads. See the Rust module
 * for the fixture format.
 *
 * Regenerate `ts-written.json` with
 * `UPDATE_FS_FIXTURES=1 npm run -s test:unit -- fixtures` in packages/layer1.
 */

import { readFileSync, writeFileSync } from "node:fs";
import { afterEach, describe, expect, it, vi } from "vitest";
import {
  base64ToBytes,
  bytesToBase64,
  computeCid,
  computeDataRoot,
  DEFAULT_CHUNK_SIZE,
  splitChunks,
  toHex,
} from "@web3-storage/core";
import { commitDataRoots, makeSigner, readBlob, uploadBlob } from "@web3-storage/layer0";

import { FakeProvider } from "../../layer0/src/fake-provider.testing.js";
import { S3Client } from "./s3/client.js";
import { FsTree, providerBlobStore } from "./tree.js";

const PROVIDER = "http://provider.test";
const BUCKET = 7n;
const MTIME = 1_700_000_000n;
const GENERATOR = "byte i is i % 251";
const BIG_SIZE = 3 * DEFAULT_CHUNK_SIZE + 100;
const UPDATE_ENV = "UPDATE_FS_FIXTURES";
/** Hashing the multi-chunk file several times in JS takes a few seconds. */
const TIMEOUT_MS = 60_000;
const FIXTURES = new URL("../../../crates/primitives/file-system/fixtures/", import.meta.url);
const TS_ABOUT =
  "Written by packages/layer1/src/fixtures.test.ts; read by clients/s3/src/fixtures.rs. Regenerate: UPDATE_FS_FIXTURES=1 npm run -s test:unit -- fixtures (in packages/layer1)";

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

/** JSON with object keys sorted, formatted like `serde_json::to_string_pretty`. */
function canonicalJson(value: Json): string {
  const sortKeys = (v: Json): Json => {
    if (Array.isArray(v)) return v.map(sortKeys);
    if (v && typeof v === "object") {
      return Object.fromEntries(Object.keys(v).sort().map((k) => [k, sortKeys(v[k])]));
    }
    return v;
  };
  return JSON.stringify(sortKeys(value), null, 2) + "\n";
}

function setup(provider: FakeProvider) {
  const http = { fetch: provider.fetch };
  const tree = new FsTree(providerBlobStore(PROVIDER, BUCKET, signer, http), () => MTIME);
  const s3 = new S3Client({ api: {} as never, signer, providerUrl: PROVIDER, fetch: provider.fetch });
  return { tree, s3 };
}

/** The write sequence of `write_sequence` in clients/s3/src/fixtures.rs. */
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
      blake2_256: toHex(computeCid(file.bytes)),
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
        blake2_256: toHex(computeCid(data)),
      },
    },
  };
}

/** The fixture JSON for the bucket in `provider`. */
async function fixture(provider: FakeProvider): Promise<string> {
  const commits: string[][] = provider.calls
    .filter((c) => c.method === "POST" && c.path === "/commit")
    .map((c) => (c.body.data_roots as string[]).map((r) => r.toLowerCase()));
  const blobs: Record<string, string> = {};
  const generated: Json[] = [];
  for (const cid of [...new Set(commits.flat())].sort()) {
    const bytes = await readBlob(PROVIDER, cid, { fetch: provider.fetch });
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
  return canonicalJson({
    about: TS_ABOUT,
    bucket_id: Number(BUCKET),
    mtime: Number(MTIME),
    blobs,
    generated_blobs: generated,
    commits,
    expected: await expected(provider),
  });
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

function readFixture(name: string): Fixture {
  return JSON.parse(readFileSync(new URL(name, FIXTURES), "utf8")) as Fixture;
}

describe("cross-language fixtures", () => {
  afterEach(() => vi.useRealTimers());

  it("reads the bucket the Rust client wrote", async () => {
    const f = readFixture("rust-written.json");
    const provider = await load(f);
    const lastCommit = f.commits[f.commits.length - 1];
    expect(f.expected.root_cid).toBe(lastCommit[lastCommit.length - 1]);
    expect(await expected(provider)).toEqual(f.expected);

    // Both languages write the same bucket for the same operations.
    const ts = new FakeProvider();
    await writeSequence(ts);
    expect(await expected(ts)).toEqual(f.expected);
  }, TIMEOUT_MS);

  it("ts-written.json is current", async () => {
    const provider = new FakeProvider();
    await writeSequence(provider);
    const produced = await fixture(provider);

    // The fixture loads back and reads the same.
    const parsed = JSON.parse(produced) as Fixture;
    expect(await expected(await load(parsed))).toEqual(parsed.expected);

    const url = new URL("ts-written.json", FIXTURES);
    if (process.env[UPDATE_ENV]) writeFileSync(url, produced);
    expect(readFileSync(url, "utf8"), `ts-written.json is out of date; regenerate it with ${UPDATE_ENV}=1`).toBe(
      produced,
    );
  }, TIMEOUT_MS);
});
