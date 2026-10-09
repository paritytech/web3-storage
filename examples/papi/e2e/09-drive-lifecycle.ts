// SPDX-License-Identifier: Apache-2.0

/**
 * E2E Workflow 09 — Drive Lifecycle
 *
 * Accounts: //Alice (provider), //Bob (owner), //Ferdie (member)
 *
 * Tests: a drive on a plain Layer 0 bucket through `FileSystemClient`: create,
 * file and directory ops, add/change/remove members, and failure cases. The
 * chain stores no drive name or drive record: a drive is its bucket id.
 *
 * Usage: node e2e/09-drive-lifecycle.js [chain_ws] [provider_url]
 */

import assert from "node:assert";
import { Enum } from "polkadot-api";
import {
  ensureProviderRegistered,
  makeSigner,
  READ_OPTS,
  sameAddress,
  type ChainSigner,
  type ParachainApi,
} from "@web3-storage/sdk";
import { FileSystemClient } from "@web3-storage/sdk/fs";
import { ensureSoleAcceptingProvider } from "../support.js";
import { runSuite, submitTxExpectFailure, setupChain } from "./helpers.js";

const CHAIN_WS = process.argv[2] || "ws://127.0.0.1:2222";
const PROVIDER_URL = process.argv[3] || "http://127.0.0.1:3333";

/** Bound on how long the provider may take to apply a membership change. */
const MEMBERSHIP_DEADLINE_MS = 60_000;

const enc = (s: string) => new TextEncoder().encode(s);
const dec = (b: Uint8Array) => new TextDecoder().decode(b);

/**
 * `FileSystemClient` for `signer` against the local provider. Chain writes
 * wait for finalization: the provider reads bucket membership from its
 * finalized view, so an in-block change would race the next HTTP request.
 */
function fsClientFor(api: ParachainApi, signer: ChainSigner) {
  return new FileSystemClient({
    api,
    signer,
    providerUrl: PROVIDER_URL,
    readOpts: READ_OPTS,
    submitMode: "finalized",
  });
}

/** Retry `fn` until it resolves; fail with the last error after the deadline. */
async function eventually<T>(fn: () => Promise<T>, label: string): Promise<T> {
  const started = Date.now();
  let last: unknown;
  while (Date.now() - started < MEMBERSHIP_DEADLINE_MS) {
    try {
      return await fn();
    } catch (err) {
      last = err;
    }
    await new Promise((resolve) => setTimeout(resolve, 500));
  }
  assert.fail(`${label}: not satisfied within ${MEMBERSHIP_DEADLINE_MS}ms: ${last}`);
}

/**
 * Retry until `fn` rejects with an error matching `expected`; rethrow any
 * other error at once; fail if it keeps resolving past the deadline.
 */
async function eventuallyRejects(
  fn: () => Promise<unknown>,
  expected: RegExp,
  label: string
): Promise<void> {
  const started = Date.now();
  while (Date.now() - started < MEMBERSHIP_DEADLINE_MS) {
    try {
      await fn();
    } catch (err) {
      if (expected.test((err as Error).message)) return;
      throw err;
    }
    await new Promise((resolve) => setTimeout(resolve, 500));
  }
  assert.fail(`${label}: still accepted after ${MEMBERSHIP_DEADLINE_MS}ms`);
}

async function main() {
  const provider = makeSigner("//Alice");
  const owner = makeSigner("//Bob");
  const member = makeSigner("//Ferdie");

  const { papi, api } = await setupChain(CHAIN_WS);
  await ensureProviderRegistered(api, provider, PROVIDER_URL);
  const restore = await ensureSoleAcceptingProvider(api, provider);

  const ownerFs = fsClientFor(api, owner);
  const memberFs = fsClientFor(api, member);
  const createOpts = {
    maxCapacity: 1_048_576n,
    storagePeriod: 100,
    provider: { address: provider.address, url: PROVIDER_URL },
  };

  let bucketId: bigint;

  const roleOf = async (account: string) =>
    (await ownerFs.getBucketMembers(bucketId)).find((m) => sameAddress(m.account, account))
      ?.role;

  const tests: Array<{ name: string; fn: () => Promise<void> }> = [];

  // ── Success ───────────────────────────────────────────────────────────────

  tests.push({
    name: "9.1 Create drive",
    fn: async () => {
      const result = await ownerFs.createDrive(createOpts);
      bucketId = result.bucketId;
      assert.ok(sameAddress(result.provider, provider.address), "provider should be Alice");

      const drive = await ownerFs.getDrive(bucketId);
      assert.ok(drive, "getDrive should find the new bucket");
      assert.ok(
        drive.providerInfo.some((p) => sameAddress(p.account, provider.address)),
        "negotiated provider should be the bucket's primary"
      );
      assert.strictEqual(drive.visibility, "Private", "drives default to Private");
      assert.strictEqual(await roleOf(owner.address), "Admin", "creator should be Admin");

      const drives = await ownerFs.listDrives();
      assert.ok(
        drives.some((d) => d.bucketId === bucketId),
        "listDrives should include the new drive"
      );
    },
  });

  tests.push({
    name: "9.2 Upload and download a file",
    fn: async () => {
      const up = await ownerFs.uploadFile(bucketId, "/hello.txt", enc("hello drive"), {
        contentType: "text/plain",
      });
      assert.strictEqual(up.size, "hello drive".length);
      assert.strictEqual(dec(await ownerFs.downloadFile(bucketId, "/hello.txt")), "hello drive");
      const withType = await ownerFs.downloadFileWithType(bucketId, "/hello.txt");
      assert.ok(withType.contentType.startsWith("text/plain"), `content type: ${withType.contentType}`);
    },
  });

  tests.push({
    name: "9.3 Create a directory and list it",
    fn: async () => {
      await ownerFs.createDirectory(bucketId, "/docs");
      await ownerFs.uploadFile(bucketId, "/docs/notes.txt", enc("notes"));
      const root = await ownerFs.listDirectory(bucketId, "/");
      assert.ok(
        root.some((e) => e.name === "docs" && e.entryType === "directory"),
        "root should list the docs directory"
      );
      assert.ok(
        root.some((e) => e.name === "hello.txt" && e.entryType === "file"),
        "root should list hello.txt"
      );
      const docs = await ownerFs.listDirectory(bucketId, "/docs");
      assert.ok(docs.some((e) => e.name === "notes.txt"), "docs should list notes.txt");
    },
  });

  tests.push({
    name: "9.4 Add member (Writer)",
    fn: async () => {
      await ownerFs.addMember(bucketId, member.address, "Writer");
      assert.strictEqual(await roleOf(member.address), "Writer");
      const memberDrives = await memberFs.listDrives();
      assert.ok(
        memberDrives.some((d) => d.bucketId === bucketId),
        "listDrives for the member should include the shared drive"
      );
      await eventually(
        () => memberFs.uploadFile(bucketId, "/from-member.txt", enc("member write")),
        "Writer upload"
      );
      assert.strictEqual(dec(await ownerFs.downloadFile(bucketId, "/from-member.txt")), "member write");
    },
  });

  tests.push({
    name: "9.5 Change member role (Reader)",
    fn: async () => {
      await ownerFs.addMember(bucketId, member.address, "Reader");
      assert.strictEqual(await roleOf(member.address), "Reader");
      assert.strictEqual(dec(await memberFs.downloadFile(bucketId, "/hello.txt")), "hello drive");
      await eventuallyRejects(
        () => memberFs.uploadFile(bucketId, "/reader-write.txt", enc("nope")),
        /Upload failed: 403/,
        "Reader upload"
      );
    },
  });

  tests.push({
    name: "9.6 Remove member",
    fn: async () => {
      await ownerFs.removeMember(bucketId, member.address);
      assert.strictEqual(await roleOf(member.address), undefined, "member should be gone");
      const memberDrives = await memberFs.listDrives();
      assert.ok(
        !memberDrives.some((d) => d.bucketId === bucketId),
        "listDrives for the removed member should not include the drive"
      );
      await eventuallyRejects(
        () => memberFs.downloadFile(bucketId, "/hello.txt"),
        /Download failed: 403/,
        "removed member read of a Private drive"
      );
    },
  });

  tests.push({
    name: "9.7 Delete a file",
    fn: async () => {
      await ownerFs.deleteFile(bucketId, "/from-member.txt");
      const root = await ownerFs.listDirectory(bucketId, "/");
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
        memberFs.uploadFile(bucketId, "/intruder.txt", enc("nope")),
        /Upload failed: 403/,
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
