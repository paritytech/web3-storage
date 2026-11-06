// SPDX-License-Identifier: Apache-2.0

/**
 * The wasm tree end to end: the Rust tree over the JS blob store over a fake
 * provider. The Rust crate tests the tree itself; these tests cover the
 * boundary: results reach JS in the documented shapes, errors keep their
 * codes and messages, and the store's provider calls work.
 */

import { describe, expect, it } from "vitest";
import { makeSigner } from "@web3-storage/layer0";
import { FakeProvider } from "@web3-storage/layer0/testing";

import { providerBlobStore } from "./store.js";
import { FileSystemError, FsTree } from "./tree.js";

const PROVIDER = "http://provider.test";
const BUCKET = 3n;
const signer = makeSigner("//Alice");
const te = new TextEncoder();
const td = new TextDecoder();

function setup(fetch?: typeof globalThis.fetch) {
  const provider = new FakeProvider();
  const tree = new FsTree(providerBlobStore(PROVIDER, BUCKET, signer, { fetch: fetch ?? provider.fetch }));
  return { provider, tree };
}

describe("FsTree (wasm)", () => {
  it("an empty bucket has no root and no entries", async () => {
    const { tree } = setup();
    expect(await tree.rootCid()).toBeNull();
    expect(await tree.list("/")).toEqual([]);
  });

  it("writes and reads files; the root is the last committed leaf", async () => {
    const { provider, tree } = setup();
    const put = await tree.putFile("/docs/a.txt", te.encode("a"), {
      contentType: "text/plain",
      userMetadata: { origin: "test" },
    });
    expect(put.contentRoot, "CID of the one-chunk content \"a\" (shared test vector)").toBe("0x7234082e1dd0b5ec0acd71875d61c9f374af30c100bc4de7aa4eb3f15bbed686");
    expect(put.rootCid).toBe(provider.committed(BUCKET).at(-1));
    expect(await tree.rootCid()).toBe(put.rootCid);

    const file = await tree.getFile("/docs/a.txt");
    expect(td.decode(file.bytes)).toBe("a");
    expect(file).toMatchObject({
      path: "/docs/a.txt",
      size: 1n,
      contentType: "text/plain",
      contentRoot: put.contentRoot,
      manifestCid: put.manifestCid,
      userMetadata: { origin: "test" },
    });

    const [docs] = await tree.list("/");
    expect(docs).toMatchObject({ name: "docs", path: "/docs", entryType: "directory", size: 0n });
    expect(typeof docs.mtime).toBe("bigint");
  });

  it("mkdir and delete, with and without pruning empty parents", async () => {
    const { tree } = setup();
    await tree.mkdir("/a/b");
    await tree.putFile("/a/b/c.txt", te.encode("c"));
    await tree.delete("/a/b/c.txt");
    expect((await tree.list("/a")).map((e) => e.name)).toEqual(["b"]);
    await tree.putFile("/a/b/c.txt", te.encode("c"));
    await tree.delete("/a/b/c.txt", { pruneEmptyParents: true });
    expect(await tree.list("/")).toEqual([]);
  });

  it("errors carry the Rust variant name as code", async () => {
    const { tree } = setup();
    await tree.mkdir("/d");
    await expect(tree.getFile("/missing")).rejects.toMatchObject({ code: "PathNotFound" });
    await expect(tree.mkdir("/d")).rejects.toMatchObject({ code: "EntryExists" });
    await expect(tree.list("relative")).rejects.toBeInstanceOf(FileSystemError);
  });

  it("a provider rejection reaches the caller with its message", async () => {
    const provider = new FakeProvider();
    const forbidden = (async (input: RequestInfo | URL, init?: RequestInit) =>
      (init?.method ?? "GET").toUpperCase() === "PUT"
        ? new Response(JSON.stringify({ error: "forbidden" }), { status: 403 })
        : provider.fetch(input, init)) as typeof globalThis.fetch;
    const { tree } = setup(forbidden);
    await expect(tree.putFile("/x", te.encode("x"))).rejects.toThrow(/\/node: 403/);
  });

  it("rejects a root whose MMR proof is not for the last leaf", async () => {
    const provider = new FakeProvider();
    await setup(provider.fetch).tree.putFile("/f", te.encode("f"));
    const firstLeafProof = (async (input: RequestInfo | URL, init?: RequestInit) => {
      const url = new URL(String(input));
      if (url.pathname === "/mmr_proof") url.searchParams.set("leaf_index", "0");
      return provider.fetch(url, init);
    }) as typeof globalThis.fetch;
    await expect(setup(firstLeafProof).tree.list("/")).rejects.toMatchObject({ code: "InvalidMmrProof" });
  });

  it("rejects a blob that does not match its CID", async () => {
    const { provider, tree } = setup();
    const put = await tree.putFile("/f", te.encode("real"));
    provider.tamper(put.contentRoot, te.encode("fake"));
    await expect(tree.getFile("/f")).rejects.toBeInstanceOf(Error);
  });
});
