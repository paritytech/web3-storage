// SPDX-License-Identifier: Apache-2.0

import { describe, expect, it } from "vitest";
import { makeSigner } from "@web3-storage/layer0";

import { FakeProvider } from "../../../layer0/src/fake-provider.testing.js";
import { paginateKeys, S3Client } from "./client.js";

const BUCKET = 9n;
const te = new TextEncoder();
const td = new TextDecoder();

function makeClient() {
  const provider = new FakeProvider();
  const client = new S3Client({
    api: {} as never,
    signer: makeSigner("//Alice"),
    providerUrl: "http://provider.test",
    fetch: provider.fetch,
  });
  return { client, provider };
}

describe("S3Client objects", () => {
  it("put/get/head round-trip content, type, metadata and etag", async () => {
    const { client, provider } = makeClient();
    const put = await client.putObject(BUCKET, "dir/a b.txt", te.encode("body"), {
      contentType: "text/plain",
      metadata: { Origin: "test" },
    });
    const got = await client.getObject(BUCKET, "dir/a b.txt");
    expect(td.decode(got.data)).toBe("body");
    expect(got).toMatchObject({
      key: "dir/a b.txt",
      contentType: "text/plain",
      size: 4,
      metadata: { origin: "test" },
      etag: put.cid,
    });
    const head = await client.headObject(BUCKET, "dir/a b.txt");
    const { data: _data, ...meta } = got;
    expect(head).toEqual(meta);
    expect(provider.calls.every((c) => !c.path.startsWith("/s3") && !c.path.startsWith("/fs"))).toBe(true);
  });

  it("stores metadata lowercased and sorted, like the Rust client", async () => {
    const { client } = makeClient();
    await client.putObject(BUCKET, "k", te.encode("x"), { metadata: { Zeta: "2", alpha: "1" } });
    // Read order is stored order, so this checks the manifest is sorted by key.
    const head = await client.headObject(BUCKET, "k");
    expect(Object.keys(head.metadata)).toEqual(["alpha", "zeta"]);
    await expect(
      client.putObject(BUCKET, "k", te.encode("x"), { metadata: { A: "1", a: "2" } }),
    ).rejects.toThrow(/appears twice/);
  });

  it("get of a missing key fails with NoSuchKey", async () => {
    const { client } = makeClient();
    await client.putObject(BUCKET, "a/b", te.encode("x"));
    await expect(client.getObject(BUCKET, "a")).rejects.toThrow(/NoSuchKey: a/);
    await expect(client.headObject(BUCKET, "nope")).rejects.toThrow(/NoSuchKey: nope/);
    await expect(client.getObject(BUCKET, "a/b/c")).rejects.toThrow(/NoSuchKey/);
  });

  it("delete is a no-op for a missing key and prunes emptied directories", async () => {
    const { client } = makeClient();
    await client.deleteObject(BUCKET, "missing/key");
    await client.putObject(BUCKET, "a/b/c", te.encode("x"));
    await client.putObject(BUCKET, "z", te.encode("y"));
    await client.deleteObject(BUCKET, "a"); // a directory, not a key
    await client.deleteObject(BUCKET, "a/b/c");
    const listed = await client.listObjects(BUCKET);
    expect(listed.objects.map((o) => o.key)).toEqual(["z"]);
    expect(listed.commonPrefixes).toEqual([]);
  });

  it("validates object keys", () => {
    const { client } = makeClient();
    for (const key of ["", "/a", "a/", "a//b", "./a", "a/..", "x".repeat(1025), `${"x".repeat(257)}/a`, "a\ud800", "\udc00/b"]) {
      expect(() => client.validateObjectKey(key), key).toThrow(/Object key must/);
    }
    for (const key of ["a", "a/b/c", "é/ü", "a b", "x".repeat(256)]) client.validateObjectKey(key);
  });
});

describe("S3Client.listObjects", () => {
  const keys = ["photos/2024/b.jpg", "photos/2024/a.jpg", "photos/2025/c.jpg", "photos/x.jpg", "readme", "B", "é"];

  async function filled() {
    const { client } = makeClient();
    for (const key of keys) await client.putObject(BUCKET, key, te.encode(key));
    return client;
  }

  it("sorts all keys by bytes, not by tree walk order", async () => {
    const client = await filled();
    const r = await client.listObjects(BUCKET);
    expect(r.objects.map((o) => o.key)).toEqual([
      "B",
      "photos/2024/a.jpg",
      "photos/2024/b.jpg",
      "photos/2025/c.jpg",
      "photos/x.jpg",
      "readme",
      "é",
    ]);
    expect(r.isTruncated).toBe(false);
  });

  it("applies prefix, delimiter, startAfter and maxKeys", async () => {
    const client = await filled();
    expect((await client.listObjects(BUCKET, { prefix: "photos/20" })).objects.map((o) => o.key)).toEqual([
      "photos/2024/a.jpg",
      "photos/2024/b.jpg",
      "photos/2025/c.jpg",
    ]);
    const delimited = await client.listObjects(BUCKET, { prefix: "photos/", delimiter: "/" });
    expect(delimited.commonPrefixes).toEqual(["photos/2024/", "photos/2025/"]);
    expect(delimited.objects.map((o) => o.key)).toEqual(["photos/x.jpg"]);
    const top = await client.listObjects(BUCKET, { delimiter: "/" });
    expect(top.commonPrefixes).toEqual(["photos/"]);
    expect(top.objects.map((o) => o.key)).toEqual(["B", "readme", "é"]);
    const after = await client.listObjects(BUCKET, { startAfter: "photos/2025/c.jpg" });
    expect(after.objects.map((o) => o.key)).toEqual(["photos/x.jpg", "readme", "é"]);
    expect((await client.listObjects(BUCKET, { prefix: "nothing/here" })).objects).toEqual([]);
  });

  it("pages with maxKeys and nextStartAfter", async () => {
    const client = await filled();
    const seen: string[] = [];
    let startAfter: string | undefined;
    for (;;) {
      const page = await client.listObjects(BUCKET, { maxKeys: 3, startAfter });
      seen.push(...page.objects.map((o) => o.key));
      if (!page.isTruncated) break;
      expect(page.objects).toHaveLength(3);
      startAfter = page.nextStartAfter;
    }
    expect(seen).toHaveLength(keys.length);
  });
});

describe("S3Client.listObjects limits", () => {
  it("clamps maxKeys to 1-1000", async () => {
    const { client } = makeClient();
    for (let i = 0; i < 3; i++) await client.putObject(BUCKET, `k${i}`, te.encode("x"));
    const zero = await client.listObjects(BUCKET, { maxKeys: 0 });
    expect(zero.objects.map((o) => o.key)).toEqual(["k0"]);
    expect(zero.isTruncated).toBe(true);
    expect(zero.nextStartAfter).toBe("k0");
    const huge = await client.listObjects(BUCKET, { maxKeys: Number.MAX_SAFE_INTEGER });
    expect(huge.objects).toHaveLength(3);
    expect(huge.isTruncated).toBe(false);
  });

  it("listAllObjects returns every key from one root read", async () => {
    const { client, provider } = makeClient();
    for (const key of ["d/b", "d/a", "d/e/f", "z"]) await client.putObject(BUCKET, key, te.encode("x"));
    const before = provider.calls.length;
    const all = await client.listAllObjects(BUCKET, { prefix: "d/" });
    expect(all.objects.map((o) => o.key)).toEqual(["d/a", "d/b", "d/e/f"]);
    expect(all.commonPrefixes).toEqual([]);
    const rootReads = provider.calls.slice(before).filter((c) => c.path === "/commitment");
    expect(rootReads).toHaveLength(1);
    expect(await client.listAllObjects(BUCKET, { delimiter: "/" })).toMatchObject({
      objects: [{ key: "z" }],
      commonPrefixes: ["d/"],
    });
  });
});

describe("paginateKeys", () => {
  // The keys of the Rust `list_page` tests (clients/s3/src/lib.rs).
  const KEYS = ["a.txt", "b/1.txt", "b/2.txt", "b/c/3.txt", "b0.txt", "d/4.txt"];
  const objects = KEYS.map((key) => ({ key, size: 1 }));
  const page = (prefix: string, delimiter: string | undefined, startAfter: string | undefined, maxKeys: number) =>
    paginateKeys(objects, { prefix, delimiter, startAfter, maxKeys });
  const keysOf = (r: { objects: { key: string }[] }) => r.objects.map((o) => o.key);

  it("sorts keys by bytes", () => {
    // `b/` (0x2f) sorts before `b0` (0x30): S3 order, not tree walk order.
    const shuffled = [...objects].reverse();
    expect(keysOf(paginateKeys(shuffled, { prefix: "", maxKeys: 1000 }))).toEqual(KEYS);
  });

  it("without delimiter returns all matching keys", () => {
    const all = page("", undefined, undefined, 1000);
    expect(keysOf(all)).toEqual(KEYS);
    expect(all.isTruncated).toBe(false);
    expect(all.nextStartAfter).toBeUndefined();
    expect(keysOf(page("b/", undefined, undefined, 1000))).toEqual(["b/1.txt", "b/2.txt", "b/c/3.txt"]);
    expect(keysOf(page("b", undefined, undefined, 1000))).toEqual(["b/1.txt", "b/2.txt", "b/c/3.txt", "b0.txt"]);
  });

  it("with delimiter groups common prefixes", () => {
    const top = page("", "/", undefined, 1000);
    expect(keysOf(top)).toEqual(["a.txt", "b0.txt"]);
    expect(top.commonPrefixes).toEqual(["b/", "d/"]);
    const sub = page("b/", "/", undefined, 1000);
    expect(keysOf(sub)).toEqual(["b/1.txt", "b/2.txt"]);
    expect(sub.commonPrefixes).toEqual(["b/c/"]);
    // An empty delimiter means no delimiter.
    expect(keysOf(page("", "", undefined, 1000))).toEqual(KEYS);
  });

  it("startAfter skips keys and prefixes", () => {
    expect(keysOf(page("", undefined, "b/2.txt", 1000))).toEqual(["b/c/3.txt", "b0.txt", "d/4.txt"]);
    // A common prefix as startAfter skips every key under it.
    const r = page("", "/", "b/", 1000);
    expect(keysOf(r)).toEqual(["b0.txt"]);
    expect(r.commonPrefixes).toEqual(["d/"]);
  });

  it("maxKeys truncates and pages", () => {
    const first = page("", "/", undefined, 2);
    expect(keysOf(first)).toEqual(["a.txt"]);
    expect(first.commonPrefixes).toEqual(["b/"]);
    expect(first.isTruncated).toBe(true);
    expect(first.nextStartAfter).toBe("b/");

    const second = page("", "/", first.nextStartAfter, 2);
    expect(keysOf(second)).toEqual(["b0.txt"]);
    expect(second.commonPrefixes).toEqual(["d/"]);
    expect(second.isTruncated).toBe(false);
    expect(second.nextStartAfter).toBeUndefined();

    expect(page("", undefined, undefined, KEYS.length).isTruncated).toBe(false);
  });
});
