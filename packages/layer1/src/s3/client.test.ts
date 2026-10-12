// SPDX-License-Identifier: Apache-2.0

import { describe, expect, it, vi } from "vitest";
import { makeSigner } from "@web3-storage/layer0";
import { S3Client } from "./client.js";

const BUCKET = 9n;

function makeClient(opts: {
  body?: Uint8Array | string;
  json?: unknown;
  status?: number;
}) {
  const calls: Array<{ url: string; init?: RequestInit }> = [];
  const fetchImpl = vi.fn(async (url: RequestInfo | URL, init?: RequestInit) => {
    calls.push({ url: String(url), init });
    if (opts.body !== undefined) {
      const bytes = typeof opts.body === "string" ? new TextEncoder().encode(opts.body) : opts.body;
      return new Response((bytes as Uint8Array<ArrayBuffer>).slice(), { status: opts.status ?? 200 });
    }
    return new Response(JSON.stringify(opts.json ?? {}), { status: opts.status ?? 200 });
  });
  const api = {};
  const client = new S3Client({
    api: api as never,
    signer: makeSigner("//Alice"),
    providerUrl: "http://provider.test",
    fetch: fetchImpl as never,
  });
  return { client, calls, api };
}

describe("S3Client HTTP ops", () => {
  it("putObject hits the object route with auth + x-amz-meta headers", async () => {
    const { client, calls } = makeClient({ json: { data_root: "0xabc" } });
    const r = await client.putObject(BUCKET, "a b.txt", new Uint8Array([1]), {
      contentType: "text/plain",
      metadata: { origin: "test" },
    });
    expect(r.cid).toBe("0xabc");
    expect(calls[0].url).toBe("http://provider.test/s3/9/object?key=a%20b.txt");
    const headers = calls[0].init!.headers as Record<string, string>;
    expect(headers["Content-Type"]).toBe("text/plain");
    expect(headers["x-amz-meta-origin"]).toBe("test");
    expect(headers.Authorization).toMatch(/^Web3Storage 0x/);
  });

  it("getObject returns the bytes and the provider's content type", async () => {
    const body = new TextEncoder().encode("object bytes");
    const { client, calls } = makeClient({ body });
    const got = await client.getObject(BUCKET, "x");
    expect(calls[0].url).toBe("http://provider.test/s3/9/object?key=x");
    expect(got.data).toEqual(body);
    expect(got.contentType).toBe("application/octet-stream");
  });

  it("listObjects maps the provider wire shape", async () => {
    const { client, calls } = makeClient({
      json: { contents: [{ key: "k", size: 3, last_modified: 10, etag: "e" }] },
    });
    const listed = await client.listObjects(BUCKET, "pre/");
    expect(calls[0].url).toBe("http://provider.test/s3/9/objects?prefix=pre%2F");
    expect(listed).toEqual([{ key: "k", size: 3, etag: "e", lastModified: 10_000 }]);
  });

  it("validates object keys", () => {
    const { client } = makeClient({});
    expect(() => client.validateObjectKey("")).toThrow("1-1024");
  });
});
