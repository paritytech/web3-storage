// SPDX-License-Identifier: Apache-2.0

import { afterEach, describe, expect, it, vi } from "vitest";
import {
  bytesToBase64,
  CidMismatchError,
  hashChildren,
  hashLeaf,
  toHex,
} from "@web3-storage/core";
import type { ChainSigner } from "./signers.js";
import { downloadChunk, putChunk, uploadChunk } from "./provider-http.js";

const PROVIDER = "http://provider.test";
const enc = new TextEncoder();

function jsonResponse(body: unknown): Response {
  return new Response(JSON.stringify(body), { status: 200 });
}

function stubFetch(handler: (url: URL, init?: RequestInit) => unknown) {
  const fetchMock = vi.fn(async (url: URL | string, init?: RequestInit) =>
    jsonResponse(handler(new URL(url), init)),
  );
  vi.stubGlobal("fetch", fetchMock);
  return fetchMock;
}

afterEach(() => {
  vi.unstubAllGlobals();
});

describe("downloadChunk", () => {
  it("returns a chunk that hashes as a leaf", async () => {
    const chunk = enc.encode("a chunk");
    stubFetch(() => ({ data: bytesToBase64(chunk), children: null }));
    await expect(downloadChunk(PROVIDER, toHex(hashLeaf(chunk)))).resolves.toEqual(chunk);
  });

  it("returns an internal node that hashes over its two children", async () => {
    const left = hashLeaf(enc.encode("left"));
    const right = hashLeaf(enc.encode("right"));
    const data = new Uint8Array([...left, ...right]);
    stubFetch(() => ({ data: bytesToBase64(data), children: [toHex(left), toHex(right)] }));
    await expect(downloadChunk(PROVIDER, toHex(hashChildren(left, right)))).resolves.toEqual(data);
  });

  it("throws CidMismatchError for tampered chunk bytes", async () => {
    const chunk = enc.encode("a chunk");
    stubFetch(() => ({ data: bytesToBase64(enc.encode("another")), children: null }));
    await expect(downloadChunk(PROVIDER, toHex(hashLeaf(chunk)))).rejects.toBeInstanceOf(
      CidMismatchError,
    );
  });

  it("throws CidMismatchError when a node's data is not its children", async () => {
    const left = hashLeaf(enc.encode("left"));
    const right = hashLeaf(enc.encode("right"));
    stubFetch(() => ({
      data: bytesToBase64(enc.encode("something else")),
      children: [toHex(left), toHex(right)],
    }));
    await expect(downloadChunk(PROVIDER, toHex(hashChildren(left, right)))).rejects.toBeInstanceOf(
      CidMismatchError,
    );
  });

  it("does not accept a node's bytes served as a chunk", async () => {
    const left = hashLeaf(enc.encode("left"));
    const right = hashLeaf(enc.encode("right"));
    const data = new Uint8Array([...left, ...right]);
    stubFetch(() => ({ data: bytesToBase64(data), children: null }));
    await expect(downloadChunk(PROVIDER, toHex(hashChildren(left, right)))).rejects.toBeInstanceOf(
      CidMismatchError,
    );
  });
});

describe("chunk uploads", () => {
  const signer = {
    signer: { publicKey: new Uint8Array(32), signBytes: async () => new Uint8Array(64) },
  } as unknown as ChainSigner;

  it("putChunk sends the leaf hash of the bytes", async () => {
    const fetchMock = stubFetch(() => ({}));
    const result = await putChunk(PROVIDER, 1n, "payload", signer);
    const expected = toHex(hashLeaf(enc.encode("payload")));
    expect(result.hash).toBe(expected);
    const body = JSON.parse(fetchMock.mock.calls[0][1]?.body as string);
    expect(body.hash).toBe(expected);
    expect(body.children).toBeNull();
  });

  it("putChunk with children sends the node hash and the child hashes", async () => {
    const fetchMock = stubFetch(() => ({}));
    const left = hashLeaf(enc.encode("left"));
    const right = hashLeaf(enc.encode("right"));
    const data = new Uint8Array([...left, ...right]);
    const result = await putChunk(PROVIDER, 1n, data, signer, [left, right]);
    const expected = toHex(hashChildren(left, right));
    expect(result.hash).toBe(expected);
    const body = JSON.parse(fetchMock.mock.calls[0][1]?.body as string);
    expect(body.hash).toBe(expected);
    expect(body.children).toEqual([toHex(left), toHex(right)]);
  });

  it("uploadChunk sends the leaf hash and commits it as the data root", async () => {
    const fetchMock = stubFetch(() => ({}));
    const result = await uploadChunk(PROVIDER, 1n, "payload", signer);
    const expected = toHex(hashLeaf(enc.encode("payload")));
    expect(result.hash).toBe(expected);
    const [put, commit] = fetchMock.mock.calls.map((call) => JSON.parse(call[1]?.body as string));
    expect(put.hash).toBe(expected);
    expect(commit.data_roots).toEqual([expected]);
  });
});
