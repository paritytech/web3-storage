// SPDX-License-Identifier: Apache-2.0

/** The wasm object mapping end to end, over a fake provider (see tree.test.ts). */

import { describe, expect, it } from "vitest";
import { makeSigner } from "@web3-storage/layer0";
import { FakeProvider } from "@web3-storage/layer0/testing";

import { ObjectError, S3Bucket } from "./objects.js";
import { providerBlobStore } from "./store.js";
import { FsTree } from "./tree.js";

const signer = makeSigner("//Alice");
const te = new TextEncoder();
const td = new TextDecoder();

function setup() {
  const provider = new FakeProvider();
  const tree = new FsTree(providerBlobStore("http://provider.test", 5n, signer, { fetch: provider.fetch }));
  return { tree, objects: new S3Bucket(tree) };
}

describe("S3Bucket (wasm)", () => {
  it("put, get, head and delete an object; the key is a file in the tree", async () => {
    const { tree, objects } = setup();
    const put = await objects.putObject("photos/cat.jpg", te.encode("meow"), {
      contentType: "image/jpeg",
      metadata: { "Camera-Model": "X100" },
    });
    expect(put).toMatchObject({ etag: put.cid, size: 4 });

    const got = await objects.getObject("photos/cat.jpg");
    expect(td.decode(got.data)).toBe("meow");
    expect(got).toMatchObject({ key: "photos/cat.jpg", contentType: "image/jpeg", metadata: { "camera-model": "X100" } });
    expect(got.lastModified % 1000).toBe(0);
    expect(await objects.headObject("photos/cat.jpg")).toMatchObject({ etag: put.cid, size: 4 });
    expect(td.decode((await tree.getFile("/photos/cat.jpg")).bytes)).toBe("meow");

    await objects.deleteObject("photos/cat.jpg");
    expect(await tree.list("/")).toEqual([]);
    await expect(objects.getObject("photos/cat.jpg")).rejects.toMatchObject({ code: "ObjectNotFound" });
  });

  it("lists by prefix and delimiter, and pages with startAfter", async () => {
    const { objects } = setup();
    for (const key of ["a.txt", "b/1.txt", "b/2.txt", "c.txt"]) await objects.putObject(key, te.encode(key));
    const top = await objects.listObjects({ delimiter: "/" });
    expect(top.objects.map((o) => o.key)).toEqual(["a.txt", "c.txt"]);
    expect(top.commonPrefixes).toEqual(["b/"]);
    const first = await objects.listObjects({ maxKeys: 2 });
    expect(first).toMatchObject({ isTruncated: true, nextStartAfter: "b/1.txt" });
    expect((await objects.listAllObjects()).objects.map((o) => o.key)).toEqual(["a.txt", "b/1.txt", "b/2.txt", "c.txt"]);
  });

  it("rejects an invalid key with an ObjectError", async () => {
    const { objects } = setup();
    await expect(objects.putObject("", te.encode("x"))).rejects.toBeInstanceOf(ObjectError);
    await expect(objects.putObject("a/../b", te.encode("x"))).rejects.toMatchObject({ code: "InvalidObjectKey" });
  });
});
