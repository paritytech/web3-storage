// SPDX-License-Identifier: GPL-3.0-only

import { describe, expect, it, vi } from "vitest";
import { makeSigner } from "@web3-storage/layer0";
import { FileSystemClient } from "./client.js";

function makeClient(json: unknown = {}, body?: Uint8Array, headers?: HeadersInit) {
  const calls: Array<{ url: string; init?: RequestInit }> = [];
  const fetchImpl = vi.fn(async (url: RequestInfo | URL, init?: RequestInit) => {
    calls.push({ url: String(url), init });
    if (body) return new Response((body as Uint8Array<ArrayBuffer>).slice(), { status: 200, headers });
    return new Response(JSON.stringify(json), { status: 200 });
  });
  const client = new FileSystemClient({
    api: {} as never,
    signer: makeSigner("//Bob"),
    providerUrl: "http://provider.test",
    fetch: fetchImpl as never,
  });
  return { client, calls };
}

describe("FileSystemClient HTTP ops", () => {
  it("listDirectory maps wire entries and mtime to millis", async () => {
    const { client, calls } = makeClient({
      entries: [{ name: "f", path: "/d/f", entry_type: "file", size: 5, mtime: 2 }],
    });
    const entries = await client.listDirectory(1n, "/d");
    expect(calls[0].url).toBe("http://provider.test/fs/1/ls?path=%2Fd");
    expect(entries).toEqual([
      { name: "f", path: "/d/f", entryType: "file", size: 5, mtime: 2_000 },
    ]);
  });

  it("uploadFile PUTs bytes with contentType + auth and echoes data_root", async () => {
    const { client, calls } = makeClient({ data_root: "0xroot" });
    const r = await client.uploadFile(1n, "/a.txt", new Uint8Array([1, 2]), {
      contentType: "text/plain",
    });
    expect(r).toEqual({ dataRoot: "0xroot", size: 2 });
    expect(calls[0].init!.method).toBe("PUT");
    const headers = calls[0].init!.headers as Record<string, string>;
    expect(headers["Content-Type"]).toBe("text/plain");
    expect(headers.Authorization).toMatch(/^Web3Storage 0x/);
  });

  it("downloadFile returns raw bytes (documented unverified path)", async () => {
    const body = new TextEncoder().encode("file body");
    const { client } = makeClient({}, body);
    expect(await client.downloadFile(1n, "/a.txt")).toEqual(body);
  });

  it("downloadFileWithType returns bytes + content-type from the response header", async () => {
    const body = new TextEncoder().encode("img");
    const { client } = makeClient({}, body, { "content-type": "image/jpeg" });
    expect(await client.downloadFileWithType(1n, "/a.jpg")).toEqual({
      bytes: body,
      contentType: "image/jpeg",
    });
  });

  it("downloadFileWithType falls back to application/octet-stream without a content-type header", async () => {
    const body = new TextEncoder().encode("bin");
    const { client } = makeClient({}, body);
    expect(await client.downloadFileWithType(1n, "/a.bin")).toEqual({
      bytes: body,
      contentType: "application/octet-stream",
    });
  });

  it("uses the explicit providerUrl override without chain resolution", async () => {
    const { client, calls } = makeClient({ entries: [] });
    await client.listDirectory(42n, "/");
    expect(calls[0].url.startsWith("http://provider.test/fs/42/")).toBe(true);
  });

  it("listDirectory passes recursive=true when requested", async () => {
    const { client, calls } = makeClient({ entries: [] });
    await client.listDirectory(1n, "/", { recursive: true });
    expect(calls[0].url).toBe("http://provider.test/fs/1/ls?path=%2F&recursive=true");
  });

  it("getIndexRoot maps the wire metadata_merkle_root + counts", async () => {
    const { client, calls } = makeClient({
      bucket_id: 1,
      metadata_merkle_root: "0xabc",
      file_count: 3,
      dir_count: 2,
      total_size: 99,
    });
    const idx = await client.getIndexRoot(1n);
    expect(calls[0].url).toBe("http://provider.test/fs/1/index_root");
    expect(idx).toEqual({ indexRoot: "0xabc", fileCount: 3, dirCount: 2, totalSize: 99n });
  });
});
