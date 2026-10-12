// SPDX-License-Identifier: Apache-2.0

import { describe, expect, it } from "vitest";
import { computeCid, computeDataRoot, DEFAULT_CHUNK_SIZE, toHex, verifyMmrProof } from "@web3-storage/core";

import { FakeProvider } from "./fake-provider.testing.js";
import {
  BlobVerificationError,
  commitDataRoots,
  getCommitment,
  getMmrProof,
  readBlob,
  uploadBlob,
} from "./provider-http.js";
import { makeSigner } from "./signers.js";

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
    ["3 chunks (padded to 4)", 2 * DEFAULT_CHUNK_SIZE + 5, 3, 3],
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
      expect(verifyMmrProof(proof, commitment.mmrRoot)).toBe(true);
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
