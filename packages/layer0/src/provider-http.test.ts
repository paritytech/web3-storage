// SPDX-License-Identifier: Apache-2.0

import { afterEach, describe, expect, it, vi } from "vitest";
import {
  bytesToBase64,
  CidMismatchError,
  computeCid,
  computeDataRoot,
  DEFAULT_CHUNK_SIZE,
  hashChildren,
  hashLeaf,
  toHex,
  verifyMmrProof,
} from "@web3-storage/core";

import type { ParachainApi } from "./address.js";
import { FakeProvider } from "./fake-provider.testing.js";
import {
  BlobVerificationError,
  commitDataRoots,
  downloadChunk,
  fetchChallengeProof,
  getCommitment,
  getMmrProof,
  putChunk,
  readBlob,
  uploadBlob,
  uploadChunk,
} from "./provider-http.js";
import { makeSigner, type ChainSigner } from "./signers.js";

const PROVIDER = "http://provider.test";
const BUCKET = 5n;
const signer = makeSigner("//Alice");

function blob(size: number): Uint8Array {
  return Uint8Array.from({ length: size }, (_, i) => (i * 31 + 7) % 251);
}

describe("uploadBlob / readBlob", () => {
  it.each([
    ["0 bytes (one empty chunk)", 0, 1, 0],
    ["1 chunk", 1000, 1, 0],
    ["3 chunks", 2 * DEFAULT_CHUNK_SIZE + 5, 3, 2],
  ])("%s", async (_name, size, chunks, internalNodes) => {
    const provider = new FakeProvider();
    const bytes = blob(size);
    const r = await uploadBlob(PROVIDER, BUCKET, bytes, signer, { fetch: provider.fetch });
    expect(r.dataRoot).toBe(toHex(computeDataRoot(bytes)));
    expect(r.chunkHashes).toHaveLength(chunks);
    const puts = provider.calls.filter((c) => c.method === "PUT");
    expect(puts.filter((c) => c.body.children === null)).toHaveLength(chunks);
    expect(puts.filter((c) => c.body.children !== null)).toHaveLength(internalNodes);
    // The root is uploaded last.
    expect(puts[puts.length - 1].body.hash).toBe(r.dataRoot);

    expect(await readBlob(PROVIDER, r.dataRoot, { fetch: provider.fetch })).toEqual(bytes);
    expect(await readBlob(PROVIDER, r.dataRoot, { fetch: provider.fetch, size })).toEqual(bytes);
  });

  it("uploads a repeated chunk once", async () => {
    const provider = new FakeProvider();
    await uploadBlob(PROVIDER, BUCKET, new Uint8Array(2 * DEFAULT_CHUNK_SIZE), signer, { fetch: provider.fetch });
    expect(provider.calls.filter((c) => c.method === "PUT" && c.body.children === null)).toHaveLength(1);
  });

  it("rejects tampered chunks on both read paths", async () => {
    const provider = new FakeProvider();
    const bytes = blob(DEFAULT_CHUNK_SIZE + 10);
    const r = await uploadBlob(PROVIDER, BUCKET, bytes, signer, { fetch: provider.fetch });
    provider.tamper(toHex(r.chunkHashes[1]), new Uint8Array(10));
    await expect(readBlob(PROVIDER, r.dataRoot, { fetch: provider.fetch })).rejects.toThrow(BlobVerificationError);
    await expect(readBlob(PROVIDER, r.dataRoot, { fetch: provider.fetch, size: bytes.length })).rejects.toThrow(
      /does not match its proof/,
    );
  });

  it("rejects a wrong size on the /read path", async () => {
    const provider = new FakeProvider();
    const bytes = blob(100);
    const r = await uploadBlob(PROVIDER, BUCKET, bytes, signer, { fetch: provider.fetch });
    await expect(readBlob(PROVIDER, r.dataRoot, { fetch: provider.fetch, size: 99 })).rejects.toThrow(/expected 99 bytes/);
  });

  it("enforces maxSize on both read paths", async () => {
    const provider = new FakeProvider();
    const small = blob(100);
    const s = await uploadBlob(PROVIDER, BUCKET, small, signer, { fetch: provider.fetch });
    expect(await readBlob(PROVIDER, s.dataRoot, { fetch: provider.fetch, maxSize: 100 })).toEqual(small);
    await expect(readBlob(PROVIDER, s.dataRoot, { fetch: provider.fetch, maxSize: 99 })).rejects.toThrow(
      /exceeds the limit of 99 bytes/,
    );
    await expect(readBlob(PROVIDER, s.dataRoot, { fetch: provider.fetch, size: 100, maxSize: 99 })).rejects.toThrow(
      /exceeds the limit/,
    );

    // Three chunks need depth 2; a one-chunk limit allows depth 0.
    const big = blob(2 * DEFAULT_CHUNK_SIZE + 5);
    const b = await uploadBlob(PROVIDER, BUCKET, big, signer, { fetch: provider.fetch });
    await expect(readBlob(PROVIDER, b.dataRoot, { fetch: provider.fetch, maxSize: DEFAULT_CHUNK_SIZE })).rejects.toThrow(
      /deeper than a blob/,
    );
    expect(await readBlob(PROVIDER, b.dataRoot, { fetch: provider.fetch, maxSize: big.length })).toEqual(big);
  });
});

describe("commitDataRoots / getCommitment / getMmrProof", () => {
  it("returns null for a bucket the provider has no data for", async () => {
    const provider = new FakeProvider();
    expect(await getCommitment(PROVIDER, BUCKET, { fetch: provider.fetch })).toBeNull();
  });

  it("commits roots in order and proves the last leaf", async () => {
    const provider = new FakeProvider();
    const roots: string[] = [];
    for (const size of [1, 2, 3]) {
      roots.push((await uploadBlob(PROVIDER, BUCKET, blob(size), signer, { fetch: provider.fetch })).dataRoot);
    }
    const commit = await commitDataRoots(PROVIDER, BUCKET, roots, signer, { fetch: provider.fetch });
    expect(commit.leafIndices).toEqual([0n, 1n, 2n]);
    expect(provider.committed(BUCKET)).toEqual(roots);

    const commitment = (await getCommitment(PROVIDER, BUCKET, { fetch: provider.fetch }))!;
    expect(commitment.leafCount).toBe(3n);
    expect(toHex(commitment.mmrRoot)).toBe(commit.mmrRoot);
    for (const index of [0n, 1n, 2n]) {
      const proof = await getMmrProof(PROVIDER, BUCKET, index, { fetch: provider.fetch });
      expect(toHex(proof.leaf.dataRoot)).toBe(roots[Number(index)]);
      expect(verifyMmrProof(proof, index, commitment.mmrRoot, commitment.leafCount)).toBe(true);
    }
  });

  it("signs writes", async () => {
    const provider = new FakeProvider();
    await uploadBlob(PROVIDER, BUCKET, blob(1), signer, { fetch: provider.fetch });
    const root = toHex(computeCid(blob(1)));
    await commitDataRoots(PROVIDER, BUCKET, [root], signer, { fetch: provider.fetch });
    // The fake rejects unsigned writes with 401; both calls succeeded.
    expect(provider.calls.map((c) => `${c.method} ${c.path}`)).toEqual(["PUT /node", "POST /commit"]);
  });
});

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

  it("throws CidMismatchError when a node does not have exactly two children", async () => {
    const left = hashLeaf(enc.encode("left"));
    stubFetch(() => ({ data: bytesToBase64(left), children: [toHex(left)] }));
    await expect(downloadChunk(PROVIDER, toHex(hashLeaf(left)))).rejects.toBeInstanceOf(CidMismatchError);
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

  it("putChunk and uploadChunk throw for a leaf above DEFAULT_CHUNK_SIZE without a request", async () => {
    const fetchMock = stubFetch(() => ({}));
    const oversized = new Uint8Array(DEFAULT_CHUNK_SIZE + 1);
    await expect(putChunk(PROVIDER, 1n, oversized, signer)).rejects.toThrow(RangeError);
    await expect(uploadChunk(PROVIDER, 1n, oversized, signer)).rejects.toThrow(RangeError);
    expect(fetchMock).not.toHaveBeenCalled();
  });

  it("putChunk and uploadChunk accept a leaf of DEFAULT_CHUNK_SIZE", async () => {
    const fetchMock = stubFetch(() => ({}));
    const leaf = new Uint8Array(DEFAULT_CHUNK_SIZE);
    await putChunk(PROVIDER, 1n, leaf, signer);
    await uploadChunk(PROVIDER, 1n, leaf, signer);
    // One PUT from putChunk, then a PUT and a commit from uploadChunk.
    expect(fetchMock).toHaveBeenCalledTimes(3);
  });
});

describe("fetchChallengeProof", () => {
  const challengeId = { deadline: 10, index: 0 };
  const getValue = vi.fn(async () => ({
    bucket_id: 1n,
    target: { leaf_index: 2n, chunk_index: 3n },
  }));
  const api = {
    query: { StorageProvider: { Challenges: { getValue } } },
  } as unknown as ParachainApi;
  const leaf = { data_root: toHex(new Uint8Array(32).fill(1)), data_size: 5, total_size: 5 };
  const item = toHex(new Uint8Array(32).fill(2));
  const chunkProof = {
    chunk_data: bytesToBase64(enc.encode("chunk")),
    proof: [item],
  };

  it("builds the proof from /mmr_proof and /chunk_proof", async () => {
    const fetchMock = stubFetch((url) =>
      url.pathname === "/mmr_proof" ? { leaf, items: [item] } : chunkProof,
    );
    const proof = await fetchChallengeProof(api, PROVIDER, challengeId);
    expect(proof).toEqual({
      chunk_data: enc.encode("chunk"),
      mmr_proof: { leaf: { data_root: leaf.data_root, data_size: 5n, total_size: 5n }, items: [item] },
      chunk_proof: [item],
    });
    expect(getValue).toHaveBeenCalledWith(10, 0, expect.anything());
    expect(fetchMock).toHaveBeenCalledTimes(2);
    const [mmrUrl, chunkUrl] = fetchMock.mock.calls.map((call) => new URL(call[0]));
    expect(mmrUrl.pathname).toBe("/mmr_proof");
    expect(mmrUrl.searchParams.get("bucket_id")).toBe("1");
    expect(mmrUrl.searchParams.get("leaf_index")).toBe("2");
    expect(chunkUrl.pathname).toBe("/chunk_proof");
    expect(chunkUrl.searchParams.get("data_root")).toBe(leaf.data_root);
    expect(chunkUrl.searchParams.get("chunk_index")).toBe("3");
  });

  it("throws for an /mmr_proof response without items, before /chunk_proof", async () => {
    const fetchMock = stubFetch(() => ({ leaf, peaks: [item], leaf_proof: { siblings: [] } }));
    await expect(fetchChallengeProof(api, PROVIDER, challengeId)).rejects.toThrow(
      "the provider node is older than this client",
    );
    expect(fetchMock).toHaveBeenCalledTimes(1);
  });

  it("throws for a /chunk_proof response with siblings and path", async () => {
    stubFetch((url) =>
      url.pathname === "/mmr_proof"
        ? { leaf, items: [item] }
        : { ...chunkProof, proof: { siblings: [item], path: [true] } },
    );
    await expect(fetchChallengeProof(api, PROVIDER, challengeId)).rejects.toThrow(
      "the provider node is older than this client",
    );
  });
});
