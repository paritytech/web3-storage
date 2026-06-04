/**
 * E2E Workflow 06 — Bucket Membership ACL
 *
 * Accounts: //Dave (admin), //Eve (writer), //Ferdie (reader)
 *
 * Tests: add/remove members, promote/demote, permission checks.
 *
 * Usage: node e2e/06-bucket-membership-acl.js [chain_ws] [provider_url]
 */

import assert from "node:assert";
import { createBucket, removeMember, setMember } from "../api.js";
import { makeSigner, READ_OPTS, sameAddress } from "../common.js";
import { runSuite, submitTxExpectFailure, setupChain } from "./helpers.js";

const CHAIN_WS = process.argv[2] || "ws://127.0.0.1:2222";

async function main() {
  const admin = makeSigner("//Dave");
  const writer = makeSigner("//Eve");
  const reader = makeSigner("//Ferdie");

  const { papi, api } = await setupChain(CHAIN_WS);

  let bucketId;

  const tests = [];

  // ── Setup ─────────────────────────────────────────────────────────────────

  tests.push({
    name: "6.0 Create bucket",
    fn: async () => {
      bucketId = await createBucket(api, admin);
      assert.ok(bucketId !== undefined, "Bucket should be created");
    },
  });

  // ── Success ───────────────────────────────────────────────────────────────

  tests.push({
    name: "6.1 Add Writer",
    fn: async () => {
      const result = await setMember(api, admin, bucketId, writer, "Writer");
      const events = api.event.StorageProvider.MemberSet.filter(result.events);
      assert.strictEqual(events.length, 1, "Expected MemberSet event");
      const bucket = await api.query.StorageProvider.Buckets.getValue(bucketId, READ_OPTS);
      assert.ok(
        bucket.members.some((m) => sameAddress(m.account, writer.address)),
        "Writer should be in members"
      );
    },
  });

  tests.push({
    name: "6.2 Add Reader",
    fn: async () => {
      const result = await setMember(api, admin, bucketId, reader, "Reader");
      const events = api.event.StorageProvider.MemberSet.filter(result.events);
      assert.strictEqual(events.length, 1, "Expected MemberSet event");
      const bucket = await api.query.StorageProvider.Buckets.getValue(bucketId, READ_OPTS);
      assert.ok(
        bucket.members.some((m) => sameAddress(m.account, reader.address)),
        "Reader should be in members"
      );
    },
  });

  tests.push({
    name: "6.3 Promote Writer to Admin",
    fn: async () => {
      await setMember(api, admin, bucketId, writer, "Admin");
      const bucket = await api.query.StorageProvider.Buckets.getValue(bucketId, READ_OPTS);
      const writerMember = bucket.members.find((m) => sameAddress(m.account, writer.address));
      assert.ok(writerMember, "Writer should still be in members");
      assert.strictEqual(writerMember.role.type, "Admin", "Writer should now be Admin");
    },
  });

  tests.push({
    name: "6.4 Demote non-last Admin to Writer (self-demotion)",
    fn: async () => {
      // Eve was promoted to Admin in 6.3. Pallet only allows self-demotion.
      await setMember(api, writer, bucketId, writer, "Writer");
      const bucket = await api.query.StorageProvider.Buckets.getValue(bucketId, READ_OPTS);
      const writerMember = bucket.members.find((m) => sameAddress(m.account, writer.address));
      assert.strictEqual(writerMember.role.type, "Writer", "Eve should be Writer again");
    },
  });

  tests.push({
    name: "6.5 Remove member",
    fn: async () => {
      const result = await removeMember(api, admin, bucketId, reader);
      const events = api.event.StorageProvider.MemberRemoved.filter(result.events);
      assert.strictEqual(events.length, 1, "Expected MemberRemoved event");
      const bucket = await api.query.StorageProvider.Buckets.getValue(bucketId, READ_OPTS);
      assert.ok(
        !bucket.members.some((m) => sameAddress(m.account, reader.address)),
        "Reader should be gone"
      );
      // Check reverse index is updated.
      const readerBuckets = await api.query.StorageProvider.MemberBuckets.getValue(reader.address, READ_OPTS);
      assert.ok(
        !readerBuckets.some((id) => id === bucketId),
        "Reader's MemberBuckets should not contain this bucket"
      );
    },
  });

  // ── Failure ───────────────────────────────────────────────────────────────

  tests.push({
    name: "6.6 Non-admin adds member",
    fn: async () => {
      // Eve is a Writer — should not be able to add members.
      const tx = api.tx.StorageProvider.set_member({
        bucket_id: bucketId,
        member: reader.address,
        role: (await import("@polkadot-api/substrate-bindings")).Enum("Reader"),
      });
      await submitTxExpectFailure(tx, writer.signer, "NotBucketAdmin", "6.6");
    },
  });

  tests.push({
    name: "6.7 Remove last admin",
    fn: async () => {
      // Dave is the only admin now. Removing self should fail.
      const tx = api.tx.StorageProvider.remove_member({
        bucket_id: bucketId,
        member: admin.address,
      });
      await submitTxExpectFailure(tx, admin.signer, "LastAdminCannotBeRemoved", "6.7");
    },
  });

  tests.push({
    name: "6.8 Remove non-existent member",
    fn: async () => {
      // Reader was already removed in 6.5.
      const tx = api.tx.StorageProvider.remove_member({
        bucket_id: bucketId,
        member: reader.address,
      });
      await submitTxExpectFailure(tx, admin.signer, "MemberNotFound", "6.8");
    },
  });

  tests.push({
    name: "6.9 Non-admin removes member",
    fn: async () => {
      const tx = api.tx.StorageProvider.remove_member({
        bucket_id: bucketId,
        member: admin.address,
      });
      await submitTxExpectFailure(tx, writer.signer, "NotBucketAdmin", "6.9");
    },
  });

  await runSuite("06 — Bucket Membership ACL", tests, { api, papi });
  papi.destroy();
}

main().catch((err) => {
  console.error(err);
  process.exitCode = 1;
}).finally(() => {
  process.exit(process.exitCode || 0);
});
