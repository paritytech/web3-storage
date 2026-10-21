// SPDX-License-Identifier: Apache-2.0

import { describe, expect, it } from "vitest";
import { makeSigner } from "@web3-storage/layer0";

import { FakeProvider } from "../../../layer0/src/fake-provider.testing.js";
import { FileSystemClient } from "./client.js";

const BUCKET = 1n;
const te = new TextEncoder();

function makeClient(signer: ReturnType<typeof makeSigner> | null = makeSigner("//Bob")) {
  const provider = new FakeProvider();
  const client = new FileSystemClient({
    api: {} as never,
    signer,
    providerUrl: "http://provider.test",
    fetch: provider.fetch,
  });
  return { client, provider };
}

describe("FileSystemClient", () => {
  it("uploads, lists and downloads through Layer 0 routes only", async () => {
    const { client, provider } = makeClient();
    const up = await client.uploadFile(BUCKET, "/docs/a.txt", te.encode("hello"), { contentType: "text/plain" });
    expect(up.size).toBe(5);
    expect(await client.getRootCid(BUCKET)).toBe(up.rootCid);

    const root = await client.listDirectory(BUCKET, "/");
    expect(root).toEqual([
      expect.objectContaining({ name: "docs", path: "/docs", entryType: "directory", size: 0 }),
    ]);
    const docs = await client.listDirectory(BUCKET, "/docs");
    expect(docs[0]).toMatchObject({ name: "a.txt", entryType: "file", size: 5, cid: up.manifestCid });
    expect(docs[0].mtime % 1000).toBe(0);

    expect(await client.downloadFile(BUCKET, "/docs/a.txt")).toEqual(te.encode("hello"));
    expect(await client.downloadFileWithType(BUCKET, "/docs/a.txt")).toEqual({
      bytes: te.encode("hello"),
      contentType: "text/plain",
    });
    expect(await client.downloadByCid("http://provider.test", up.dataRoot)).toEqual(te.encode("hello"));

    const paths = new Set(provider.calls.map((c) => c.path));
    expect([...paths].every((p) => !p.startsWith("/fs") && !p.startsWith("/s3"))).toBe(true);
  });

  it("listDirectory with recursive returns every descendant", async () => {
    const { client } = makeClient();
    await client.createDirectory(BUCKET, "/a/b");
    await client.uploadFile(BUCKET, "/a/b/c", te.encode("c"));
    const all = await client.listDirectory(BUCKET, "/", { recursive: true });
    expect(all.map((e) => e.path)).toEqual(["/a", "/a/b", "/a/b/c"]);
  });

  it("deleteFile removes a file; createDirectory fails on an existing path", async () => {
    const { client } = makeClient();
    await client.uploadFile(BUCKET, "/f", te.encode("x"));
    await expect(client.createDirectory(BUCKET, "/f")).rejects.toThrow(/Already exists/);
    await client.deleteFile(BUCKET, "/f");
    expect(await client.listDirectory(BUCKET, "/")).toEqual([]);
    await expect(client.downloadFile(BUCKET, "/f")).rejects.toThrow(/No such file/);
  });

  it("reads need no signer; writes do", async () => {
    const { client, provider } = makeClient();
    await client.uploadFile(BUCKET, "/f", te.encode("x"));
    const reader = new FileSystemClient({
      api: {} as never,
      providerUrl: "http://provider.test",
      fetch: provider.fetch,
    });
    expect(await reader.downloadFile(BUCKET, "/f")).toEqual(te.encode("x"));
    await expect(reader.uploadFile(BUCKET, "/g", te.encode("y"))).rejects.toThrow(/Signer not set/);
  });

  it("reports a provider HTTP error with the action and status", async () => {
    const provider = new FakeProvider();
    const failing = (async (input: RequestInfo | URL, init?: RequestInit) =>
      init?.method === "PUT" ? new Response("forbidden", { status: 403 }) : provider.fetch(input, init)) as typeof fetch;
    const client = new FileSystemClient({
      api: {} as never,
      signer: makeSigner("//Bob"),
      providerUrl: "http://provider.test",
      fetch: failing,
    });
    await expect(client.uploadFile(BUCKET, "/f", te.encode("x"))).rejects.toThrow(/^Upload failed: 403/);
  });

  it("an empty drive has no root CID", async () => {
    const { client } = makeClient();
    expect(await client.getRootCid(BUCKET)).toBeNull();
  });
});
