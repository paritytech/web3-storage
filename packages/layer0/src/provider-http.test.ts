// SPDX-License-Identifier: Apache-2.0

import { afterEach, describe, expect, it, vi } from "vitest";
import {
  bytesToBase64,
  CidMismatchError,
  concatBytes,
  DEFAULT_CHUNK_SIZE,
  hashChildren,
  hashLeaf,
  LEAF_VERSION,
  toHex,
} from "@web3-storage/core";
import type { ParachainApi } from "./address.js";
import type { ChainSigner } from "./signers.js";
import {
  downloadChunk,
  downloadNode,
  fetchChallengeProof,
  putChunk,
  putInternalNode,
  uploadChunk,
} from "./provider-http.js";

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

  it("throws for an internal node", async () => {
    const left = hashLeaf(enc.encode("left"));
    const right = hashLeaf(enc.encode("right"));
    stubFetch(() => ({ children: [toHex(left), toHex(right)] }));
    await expect(downloadChunk(PROVIDER, toHex(hashChildren(left, right)))).rejects.toThrow(
      "is an internal node, not a chunk",
    );
  });

  it("throws CidMismatchError for tampered chunk bytes", async () => {
    const chunk = enc.encode("a chunk");
    stubFetch(() => ({ data: bytesToBase64(enc.encode("another")), children: null }));
    await expect(downloadChunk(PROVIDER, toHex(hashLeaf(chunk)))).rejects.toBeInstanceOf(
      CidMismatchError,
    );
  });

  it("throws CidMismatchError when a node has both data and children", async () => {
    const left = hashLeaf(enc.encode("left"));
    const right = hashLeaf(enc.encode("right"));
    stubFetch(() => ({
      data: bytesToBase64(concatBytes(left, right)),
      children: [toHex(left), toHex(right)],
    }));
    await expect(downloadChunk(PROVIDER, toHex(hashChildren(left, right)))).rejects.toBeInstanceOf(
      CidMismatchError,
    );
  });

  it("throws CidMismatchError when a node does not have exactly two children", async () => {
    const left = hashLeaf(enc.encode("left"));
    stubFetch(() => ({ children: [toHex(left)] }));
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

describe("downloadNode", () => {
  it("returns a chunk's bytes", async () => {
    const chunk = enc.encode("a chunk");
    stubFetch(() => ({ data: bytesToBase64(chunk) }));
    await expect(downloadNode(PROVIDER, toHex(hashLeaf(chunk)))).resolves.toEqual({ data: chunk });
  });

  it("returns an internal node's two child hashes", async () => {
    const left = hashLeaf(enc.encode("left"));
    const right = hashLeaf(enc.encode("right"));
    stubFetch(() => ({ children: [toHex(left), toHex(right)] }));
    await expect(downloadNode(PROVIDER, toHex(hashChildren(left, right)))).resolves.toEqual({
      children: [left, right],
    });
  });

  it("throws CidMismatchError for children that do not hash to the requested hash", async () => {
    const left = hashLeaf(enc.encode("left"));
    const right = hashLeaf(enc.encode("right"));
    stubFetch(() => ({ children: [toHex(right), toHex(left)] }));
    await expect(downloadNode(PROVIDER, toHex(hashChildren(left, right)))).rejects.toBeInstanceOf(
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
    expect(body.data).toBe(bytesToBase64(enc.encode("payload")));
    expect(body.children).toBeUndefined();
  });

  it("putInternalNode sends the node hash and the child hashes", async () => {
    const fetchMock = stubFetch(() => ({}));
    const left = hashLeaf(enc.encode("left"));
    const right = hashLeaf(enc.encode("right"));
    const result = await putInternalNode(PROVIDER, 1n, left, right, signer);
    const expected = toHex(hashChildren(left, right));
    expect(result.hash).toBe(expected);
    const body = JSON.parse(fetchMock.mock.calls[0][1]?.body as string);
    expect(body.hash).toBe(expected);
    expect(body.children).toEqual([toHex(left), toHex(right)]);
    expect(body.data).toBeUndefined();
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
    target: { leaf_index: 2n, chunk_index: 0n },
  }));
  const api = {
    query: { StorageProvider: { Challenges: { getValue } } },
  } as unknown as ParachainApi;
  const leaf = {
    leaf_version: LEAF_VERSION,
    data_root: toHex(new Uint8Array(32).fill(1)),
    data_size: 5,
    total_size: 5,
  };
  const item = toHex(new Uint8Array(32).fill(2));
  const chunkProof = {
    chunk_data: bytesToBase64(enc.encode("chunk")),
    proof: [item],
  };

  it("builds the proof from /mmr_proof and /chunk_proof", async () => {
    const fetchMock = stubFetch((url) =>
      url.pathname === "/mmr_proof" ? { leaf, items: [item] } : chunkProof,
    );
    const response = await fetchChallengeProof(api, PROVIDER, challengeId);
    expect(response).toEqual({
      type: "Proof",
      value: {
        chunk_data: enc.encode("chunk"),
        mmr_proof: {
          leaf: { ...leaf, data_size: 5n, total_size: 5n },
          items: [item],
        },
        chunk_proof: [item],
      },
    });
    expect(getValue).toHaveBeenCalledWith(10, 0, expect.anything());
    expect(fetchMock).toHaveBeenCalledTimes(2);
    const [mmrUrl, chunkUrl] = fetchMock.mock.calls.map((call) => new URL(call[0]));
    expect(mmrUrl.pathname).toBe("/mmr_proof");
    expect(mmrUrl.searchParams.get("bucket_id")).toBe("1");
    expect(mmrUrl.searchParams.get("leaf_index")).toBe("2");
    expect(chunkUrl.pathname).toBe("/chunk_proof");
    expect(chunkUrl.searchParams.get("data_root")).toBe(leaf.data_root);
    expect(chunkUrl.searchParams.get("chunk_index")).toBe("0");
  });

  it("answers a chunk index past the leaf's last chunk with ChunkOutOfRange, without /chunk_proof", async () => {
    getValue.mockResolvedValueOnce({ bucket_id: 1n, target: { leaf_index: 2n, chunk_index: 1n } });
    const fetchMock = stubFetch(() => ({ leaf, items: [item] }));
    const response = await fetchChallengeProof(api, PROVIDER, challengeId);
    expect(response).toEqual({
      type: "ChunkOutOfRange",
      value: {
        mmr_proof: {
          leaf: { ...leaf, data_size: 5n, total_size: 5n },
          items: [item],
        },
      },
    });
    expect(fetchMock).toHaveBeenCalledTimes(1);
  });

  it("throws for an /mmr_proof response without items, before /chunk_proof", async () => {
    const fetchMock = stubFetch(() => ({ leaf, peaks: [item], leaf_proof: { siblings: [] } }));
    await expect(fetchChallengeProof(api, PROVIDER, challengeId)).rejects.toThrow(
      "the provider node is older than this client",
    );
    expect(fetchMock).toHaveBeenCalledTimes(1);
  });

  it("throws for an /mmr_proof leaf without leaf_version, before /chunk_proof", async () => {
    const { leaf_version: _, ...oldLeaf } = leaf;
    const fetchMock = stubFetch(() => ({ leaf: oldLeaf, items: [item] }));
    await expect(fetchChallengeProof(api, PROVIDER, challengeId)).rejects.toThrow(
      "leaf has no `leaf_version`",
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
