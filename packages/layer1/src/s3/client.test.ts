// SPDX-License-Identifier: GPL-3.0-only

import { describe, expect, it, vi } from "vitest";
import { CidMismatchError, computeCid, toHex } from "@web3-storage/core";
import { makeSigner } from "@web3-storage/layer0";
import { S3Client } from "./client.js";

const BUCKET = { s3BucketId: 7n, layer0BucketId: 9n };

function makeClient(opts: {
  body?: Uint8Array | string;
  json?: unknown;
  onChainCid?: Uint8Array;
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
  const api = {
    query: {
      S3Registry: {
        Objects: {
          getValue: vi.fn(async () =>
            opts.onChainCid
              ? { cid: toHex(opts.onChainCid), size: 1n, content_type: undefined, user_metadata: [] }
              : undefined,
          ),
        },
      },
    },
  };
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

  it("getObject verifies single-chunk payloads against the on-chain cid", async () => {
    const body = new TextEncoder().encode("verified bytes");
    const { client } = makeClient({ body, onChainCid: computeCid(body) });
    const got = await client.getObject(BUCKET, "x");
    expect(got.verified).toBe(true);
    expect(got.data).toEqual(body);
  });

  it("getObject hard-fails a corrupted single-chunk payload", async () => {
    const body = new TextEncoder().encode("tampered");
    const { client } = makeClient({
      body,
      onChainCid: computeCid(new TextEncoder().encode("original")),
    });
    await expect(client.getObject(BUCKET, "x")).rejects.toThrow(CidMismatchError);
  });

  it("getObject flags verified=false when no on-chain metadata exists", async () => {
    const { client } = makeClient({ body: "anything" });
    const got = await client.getObject(BUCKET, "x");
    expect(got.verified).toBe(false);
  });

  it("listObjects maps the provider wire shape", async () => {
    const { client, calls } = makeClient({
      json: { contents: [{ key: "k", size: 3, last_modified: 10, etag: "e" }] },
    });
    const listed = await client.listObjects(BUCKET, "pre/");
    expect(calls[0].url).toBe("http://provider.test/s3/9/objects?prefix=pre%2F");
    expect(listed).toEqual([{ key: "k", size: 3, etag: "e", lastModified: 10_000 }]);
  });

  it("validates bucket names and object keys", () => {
    const { client } = makeClient({});
    expect(() => client.validateBucketName("ab")).toThrow("3-63");
    expect(() => client.validateBucketName("Bad_Name")).toThrow("lowercase");
    expect(() => client.validateObjectKey("")).toThrow("1-1024");
  });
});
