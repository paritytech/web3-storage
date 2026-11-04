// SPDX-License-Identifier: Apache-2.0

/**
 * E2E Workflow 09 — Drive Lifecycle
 *
 * Accounts: //Alice (provider), //Bob (owner), //Ferdie (member)
 *
 * Tests: a drive on a plain Layer 0 bucket through `Web3Storage`: create,
 * file and directory ops, add/change/remove members, and failure cases. The
 * chain stores no drive name or drive record: a drive is its bucket id. The
 * directory tree is stored in the bucket and read and written through the
 * provider's Layer 0 routes only.
 *
 * Usage: node e2e/09-drive-lifecycle.js [chain_ws] [provider_url]
 */

import assert from "node:assert";
import { Enum } from "polkadot-api";
import {
  ensureProviderRegistered,
  makeSigner,
  sameAddress,
  type FsTree,
} from "@web3-storage/sdk";
import { ensureSoleAcceptingProvider } from "../support.js";
import {
  eventually,
  eventuallyRejects,
  readText,
  runSuite,
  setupChain,
  storageFor,
  submitTxExpectFailure,
} from "./helpers.js";

const CHAIN_WS = process.argv[2] || "ws://127.0.0.1:2222";
const PROVIDER_URL = process.argv[3] || "http://127.0.0.1:3333";

const enc = (s: string) => new TextEncoder().encode(s);
const dec = (b: Uint8Array) => new TextDecoder().decode(b);

async function main() {
  const provider = makeSigner("//Alice");
  const owner = makeSigner("//Bob");
  const member = makeSigner("//Ferdie");

  const { papi, api } = await setupChain(CHAIN_WS);
  await ensureProviderRegistered(api, provider, PROVIDER_URL);
  const restore = await ensureSoleAcceptingProvider(api, provider);

  const ownerStorage = storageFor(api, owner, PROVIDER_URL);
  const memberStorage = storageFor(api, member, PROVIDER_URL);
  const createOpts = {
    maxCapacity: 1_048_576n,
    duration: 100,
    provider: { address: provider.address, url: PROVIDER_URL },
  };

  let bucketId: bigint;
  /** The drive as the owner and as the member, opened by 9.1. */
  let ownerDrive: FsTree;
  let memberDrive: FsTree;

  const roleOf = async (account: string) =>
    (await ownerStorage.getBucket(bucketId))?.members.find((m) => sameAddress(m.account, account))?.role;

  const tests: Array<{ name: string; fn: () => Promise<void> }> = [];

  // ── Success ───────────────────────────────────────────────────────────────

  tests.push({
    name: "9.1 Create drive",
    fn: async () => {
      const result = await ownerStorage.createBucket(createOpts);
      bucketId = result.bucketId;
      ownerDrive = await ownerStorage.drive(bucketId);
      memberDrive = await memberStorage.drive(bucketId);
      assert.ok(sameAddress(result.provider, provider.address), "provider should be Alice");

      const info = await ownerStorage.getBucket(bucketId);
      assert.ok(info, "getBucket should find the new bucket");
      assert.ok(
        info.providerInfo.some((p) => sameAddress(p.account, provider.address)),
        "negotiated provider should be the bucket's primary"
      );
      assert.strictEqual(info.visibility, "Private", "drives default to Private");
      assert.strictEqual(await roleOf(owner.address), "Admin", "creator should be Admin");

      const drives = await ownerStorage.listBuckets();
      assert.ok(
        drives.some((d) => d.bucketId === bucketId),
        "listBuckets should include the new drive"
      );
    },
  });

  tests.push({
    name: "9.2 Upload and download a file",
    fn: async () => {
      const up = await ownerDrive.putFile("/hello.txt", enc("hello drive"), { contentType: "text/plain" });
      assert.strictEqual(up.size, "hello drive".length);
      assert.strictEqual(await ownerDrive.rootCid(), up.rootCid, "the drive root should be the upload's new root");
      const file = await ownerDrive.getFile("/hello.txt");
      assert.strictEqual(dec(file.bytes), "hello drive");
      assert.ok(file.contentType.startsWith("text/plain"), `content type: ${file.contentType}`);
    },
  });

  tests.push({
    name: "9.3 Create a directory and list it",
    fn: async () => {
      await ownerDrive.mkdir("/docs");
      await ownerDrive.putFile("/docs/notes.txt", enc("notes"));
      const root = await ownerDrive.list("/");
      assert.ok(
        root.some((e) => e.name === "docs" && e.entryType === "directory"),
        "root should list the docs directory"
      );
      assert.ok(
        root.some((e) => e.name === "hello.txt" && e.entryType === "file"),
        "root should list hello.txt"
      );
      const docs = await ownerDrive.list("/docs");
      assert.ok(docs.some((e) => e.name === "notes.txt"), "docs should list notes.txt");
    },
  });

  tests.push({
    name: "9.4 Add member (Writer)",
    fn: async () => {
      await ownerStorage.setMember(bucketId, member.address, "Writer");
      assert.strictEqual(await roleOf(member.address), "Writer");
      const memberDrives = await memberStorage.listBuckets();
      assert.ok(
        memberDrives.some((d) => d.bucketId === bucketId),
        "listBuckets for the member should include the shared drive"
      );
      await eventually(
        async () => memberDrive.putFile("/from-member.txt", enc("member write")),
        "Writer upload"
      );
      assert.strictEqual(await readText(ownerDrive, "/from-member.txt"), "member write");
    },
  });

  tests.push({
    name: "9.5 Change member role (Reader)",
    fn: async () => {
      await ownerStorage.setMember(bucketId, member.address, "Reader");
      assert.strictEqual(await roleOf(member.address), "Reader");
      assert.strictEqual(await readText(memberDrive, "/hello.txt"), "hello drive");
      await eventuallyRejects(
        async () => memberDrive.putFile("/reader-write.txt", enc("nope")),
        /\/node: 403/,
        "Reader upload"
      );
    },
  });

  tests.push({
    name: "9.6 Remove member",
    fn: async () => {
      await ownerStorage.removeMember(bucketId, member.address);
      assert.strictEqual(await roleOf(member.address), undefined, "member should be gone");
      const memberDrives = await memberStorage.listBuckets();
      assert.ok(
        !memberDrives.some((d) => d.bucketId === bucketId),
        "listBuckets for the removed member should not include the drive"
      );
      // Check writes, not reads: the client reads over Layer 0, where reads
      // are unauthenticated (anyone who knows a CID can read the blob,
      // #383/#396), so a removed member can still read. This differs from
      // "Bucket Visibility & Access" in docs/design/scalable-web3-storage.md
      // (private buckets serve reads only to members). Confidential files
      // need client-side encryption.
      await eventuallyRejects(
        async () => memberDrive.putFile("/removed-write.txt", enc("nope")),
        /\/node: 403/,
        "removed member upload"
      );
    },
  });

  tests.push({
    name: "9.7 Delete a file",
    fn: async () => {
      await ownerDrive.delete("/from-member.txt");
      const root = await ownerDrive.list("/");
      assert.ok(!root.some((e) => e.name === "from-member.txt"), "deleted file should not be listed");
      assert.ok(root.some((e) => e.name === "hello.txt"), "other files should remain");
    },
  });

  // ── Failure ───────────────────────────────────────────────────────────────

  tests.push({
    name: "9.8 Non-admin adds a member",
    fn: async () => {
      const tx = api.tx.StorageProvider.set_member({
        bucket_id: bucketId,
        member: member.address,
        role: Enum("Writer"),
      });
      await submitTxExpectFailure(tx, member.signer, "NotBucketAdmin", "9.8");
    },
  });

  tests.push({
    name: "9.9 Non-member cannot upload",
    fn: async () => {
      await assert.rejects(
        memberDrive.putFile("/intruder.txt", enc("nope")),
        /\/node: 403/,
        "a non-member upload should be refused"
      );
    },
  });

  await runSuite("09 — Drive Lifecycle", tests, { api, papi });

  try {
    await restore();
  } catch {}
  papi.destroy();
}

main()
  .catch((err) => {
    console.error(err);
    process.exitCode = 1;
  })
  .finally(() => {
    process.exit(process.exitCode || 0);
  });
