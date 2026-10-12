// SPDX-License-Identifier: Apache-2.0

import { describe, expect, it } from "vitest";
import {
  computeDataRoot,
  decodeDirectoryNode,
  DIRECTORY_NODE_MAX_SIZE,
  encodeDirectoryNode,
  hexToBytes,
  toHex,
} from "@web3-storage/core";
import { commitDataRoots, makeSigner, uploadBlob } from "@web3-storage/layer0";

import { FakeProvider } from "../../layer0/src/fake-provider.testing.js";
import { commitOrder, FileSystemError, FsTree, parsePath, providerBlobStore } from "./tree.js";

const PROVIDER = "http://provider.test";
const BUCKET = 3n;
const signer = makeSigner("//Alice");
const te = new TextEncoder();

function setup() {
  const provider = new FakeProvider();
  const tree = new FsTree(providerBlobStore(PROVIDER, BUCKET, signer, { fetch: provider.fetch }), () => 1_700_000_000n);
  return { provider, tree };
}

async function rootNode(provider: FakeProvider) {
  const roots = provider.committed(BUCKET);
  const node = provider.nodes.get(roots[roots.length - 1])!;
  return decodeDirectoryNode(node.data);
}

describe("root discovery", () => {
  it("an empty bucket is an empty drive", async () => {
    const { tree } = setup();
    expect(await tree.rootCid()).toBeNull();
    expect(await tree.list("/")).toEqual([]);
  });

  it("the root is the last committed leaf", async () => {
    const { provider, tree } = setup();
    await tree.putFile("/a.txt", te.encode("a"));
    const second = await tree.putFile("/b.txt", te.encode("b"));
    const committed = provider.committed(BUCKET);
    expect(committed[committed.length - 1]).toBe(second.rootCid);
    expect(await tree.rootCid()).toBe(second.rootCid);
    expect((await tree.list("/")).map((e) => e.name)).toEqual(["a.txt", "b.txt"]);
  });

  it("a bucket whose last leaf is not a directory node is not a file system", async () => {
    const { provider, tree } = setup();
    const { dataRoot } = await uploadBlob(PROVIDER, BUCKET, te.encode("raw data"), signer, { fetch: provider.fetch });
    await commitDataRoots(PROVIDER, BUCKET, [dataRoot], signer, { fetch: provider.fetch });
    await expect(tree.list("/")).rejects.toMatchObject({ code: "NotAFileSystem" });
    await expect(tree.rootCid()).rejects.toMatchObject({ code: "NotAFileSystem" });
  });

  async function commitRoot(provider: FakeProvider, bytes: Uint8Array) {
    const { dataRoot } = await uploadBlob(PROVIDER, BUCKET, bytes, signer, { fetch: provider.fetch });
    await commitDataRoots(PROVIDER, BUCKET, [dataRoot], signer, { fetch: provider.fetch });
  }

  it("rejects a root directory of another drive", async () => {
    const { provider, tree } = setup();
    await commitRoot(provider, encodeDirectoryNode({ driveId: BUCKET + 1n, children: [], metadata: [] }));
    await expect(tree.rootCid()).rejects.toThrow(/belongs to drive 4/);
  });

  it("rejects a root directory that breaks the format rules", async () => {
    const { provider, tree } = setup();
    const entry = (name: string) => ({
      name: te.encode(name),
      entryType: "file" as const,
      cid: new Uint8Array(32),
      size: 0n,
      mtime: 0n,
    });
    await commitRoot(provider, encodeDirectoryNode({ driveId: BUCKET, children: [entry("b"), entry("a")], metadata: [] }));
    await expect(tree.list("/")).rejects.toMatchObject({ code: "NotAFileSystem" });
  });

  it("rejects a root blob larger than a directory node can be", async () => {
    const { provider, tree } = setup();
    await commitRoot(provider, new Uint8Array(DIRECTORY_NODE_MAX_SIZE + 1));
    await expect(tree.list("/")).rejects.toMatchObject({ code: "NotAFileSystem" });
  });
});

describe("FS operations", () => {
  it("put creates parents, get returns bytes and metadata", async () => {
    const { tree } = setup();
    const r = await tree.putFile("/docs/2024/notes.txt", te.encode("hello"), { contentType: "text/plain" });
    expect(r.contentRoot).toBe(toHex(computeDataRoot(te.encode("hello"))));
    const file = await tree.getFile("/docs/2024/notes.txt");
    expect(file.bytes).toEqual(te.encode("hello"));
    expect(file).toMatchObject({ contentType: "text/plain", size: 5n, mtime: 1_700_000_000n, contentRoot: r.contentRoot });
    expect(await tree.list("/docs")).toEqual([
      expect.objectContaining({ name: "2024", path: "/docs/2024", entryType: "directory", size: 0n }),
    ]);
  });

  it("put replaces a file and keeps the default content type", async () => {
    const { tree } = setup();
    await tree.putFile("/f", te.encode("one"));
    await tree.putFile("/f", te.encode("two!"));
    const file = await tree.getFile("/f");
    expect(file.bytes).toEqual(te.encode("two!"));
    expect(file.contentType).toBe("application/octet-stream");
    expect((await tree.list("/")).length).toBe(1);
  });

  it("stores multi-chunk and empty files", async () => {
    const { tree } = setup();
    const big = Uint8Array.from({ length: 3 * 256 * 1024 + 1 }, (_, i) => i % 256);
    await tree.putFile("/big", big);
    await tree.putFile("/empty", new Uint8Array(0));
    expect((await tree.getFile("/big")).bytes).toEqual(big);
    expect((await tree.getFile("/empty")).bytes).toEqual(new Uint8Array(0));
  });

  it("put onto a directory and through a file fail", async () => {
    const { tree } = setup();
    await tree.mkdir("/d");
    await tree.putFile("/f", te.encode("x"));
    await expect(tree.putFile("/d", te.encode("x"))).rejects.toMatchObject({ code: "IsADirectory" });
    await expect(tree.putFile("/f/x", te.encode("x"))).rejects.toMatchObject({ code: "NotADirectory" });
  });

  it("mkdir creates parents and fails when the path exists", async () => {
    const { tree } = setup();
    await tree.mkdir("/a/b/c");
    expect((await tree.list("/a/b")).map((e) => e.path)).toEqual(["/a/b/c"]);
    await expect(tree.mkdir("/a/b")).rejects.toMatchObject({ code: "AlreadyExists" });
    await expect(tree.mkdir("/")).rejects.toMatchObject({ code: "AlreadyExists" });
  });

  it("delete removes files and empty directories only", async () => {
    const { tree } = setup();
    await tree.putFile("/d/f", te.encode("x"));
    await expect(tree.delete("/d")).rejects.toMatchObject({ code: "DirectoryNotEmpty" });
    await tree.delete("/d/f");
    expect(await tree.list("/d")).toEqual([]);
    await tree.delete("/d");
    expect(await tree.list("/")).toEqual([]);
    await expect(tree.delete("/d")).rejects.toMatchObject({ code: "NotFound" });
    await expect(tree.delete("/")).rejects.toMatchObject({ code: "InvalidPath" });
    expect(await tree.delete("/missing", { missingOk: true })).toBeNull();
  });

  it("delete with pruneEmptyParents removes emptied directories but not the root", async () => {
    const { tree } = setup();
    await tree.putFile("/a/b/c", te.encode("x"));
    await tree.putFile("/a/keep", te.encode("y"));
    await tree.delete("/a/b/c", { pruneEmptyParents: true });
    expect((await tree.list("/", { recursive: true })).map((e) => e.path)).toEqual(["/a", "/a/keep"]);
    await tree.delete("/a/keep", { pruneEmptyParents: true });
    expect(await tree.list("/")).toEqual([]);
  });

  it("list keeps children sorted by name bytes", async () => {
    const { tree } = setup();
    for (const name of ["b", "B", "a", "é", "aa"]) await tree.putFile(`/${name}`, te.encode(name));
    expect((await tree.list("/")).map((e) => e.name)).toEqual(["B", "a", "aa", "b", "é"]);
  });

  it("recursive list walks depth-first", async () => {
    const { tree } = setup();
    await tree.putFile("/x/1", te.encode("1"));
    await tree.putFile("/y", te.encode("2"));
    expect((await tree.list("/", { recursive: true })).map((e) => e.path)).toEqual(["/x", "/x/1", "/y"]);
  });
});

describe("paths", () => {
  const invalid = ["", "a", "//", "/a/", "/a//b", "/.", "/a/..", `/${"x".repeat(257)}`, "/a\ud800", "/\udc00/b"];
  it.each(invalid)("rejects %j", (path) => {
    expect(() => parsePath(path)).toThrow(FileSystemError);
  });

  it("accepts the root and nested names", () => {
    expect(parsePath("/")).toEqual([]);
    expect(parsePath("/a/b c/é")).toEqual(["a", "b c", "é"]);
  });
});

describe("writes", () => {
  it("commit lists content, manifest, directories bottom-up, root last", async () => {
    const { provider, tree } = setup();
    const r = await tree.putFile("/d/e/f.txt", te.encode("data"));
    const commits = provider.calls.filter((c) => c.path === "/commit");
    expect(commits).toHaveLength(1);
    const roots: string[] = commits[0].body.data_roots;
    expect(roots[0]).toBe(r.contentRoot);
    expect(roots[1]).toBe(r.manifestCid);
    expect(roots).toHaveLength(5);
    expect(roots[4]).toBe(r.rootCid);
    // Each directory blob links to the one before it.
    const e = decodeDirectoryNode(provider.nodes.get(roots[2])!.data);
    const d = decodeDirectoryNode(provider.nodes.get(roots[3])!.data);
    const root = decodeDirectoryNode(provider.nodes.get(roots[4])!.data);
    expect(toHex(e.children[0].cid)).toBe(r.manifestCid);
    expect(toHex(d.children[0].cid)).toBe(roots[2]);
    expect(toHex(root.children[0].cid)).toBe(roots[3]);
    expect(root.driveId).toBe(BUCKET);
  });

  it("dedupes CIDs within one commit and keeps the root last", () => {
    const [a, b, root] = [1, 2, 3].map((n) => new Uint8Array(32).fill(n));
    expect(commitOrder([a, b, a, root, b, root])).toEqual([a, b, root]);
    expect(commitOrder([root, a, root])).toEqual([a, root]);
  });

  it("copy-on-write: an untouched subtree keeps its CID", async () => {
    const { provider, tree } = setup();
    await tree.putFile("/keep/a", te.encode("a"));
    await tree.putFile("/change/b", te.encode("b"));
    const before = await rootNode(provider);
    await tree.putFile("/change/c", te.encode("c"));
    const after = await rootNode(provider);
    const cidOf = (node: typeof before, name: string) =>
      toHex(node.children.find((c) => new TextDecoder().decode(c.name) === name)!.cid);
    expect(cidOf(after, "keep")).toBe(cidOf(before, "keep"));
    expect(cidOf(after, "change")).not.toBe(cidOf(before, "change"));
    const last = provider.calls.filter((c) => c.path === "/commit").pop()!;
    expect(last.body.data_roots).not.toContain(cidOf(after, "keep"));
  });

  it("reads detect a tampered manifest", async () => {
    const { provider, tree } = setup();
    const r = await tree.putFile("/f", te.encode("x"));
    provider.tamper(r.manifestCid, hexToBytes("0x00"));
    await expect(tree.getFile("/f")).rejects.toThrow(/failed verification/);
  });
});
