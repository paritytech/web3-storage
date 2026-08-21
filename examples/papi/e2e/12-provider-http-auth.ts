// SPDX-License-Identifier: Apache-2.0

/**
 * E2E Workflow 12 — Provider HTTP Auth
 *
 * Accounts: //Dave (bucket admin), //Eve (writer), //Ferdie (reader),
 * //Charlie (never a member), //Alice (provider)
 *
 * Tests the provider node's signed-request guard end to end: the role ladder
 * as enforced over HTTP, rejection of unsigned/tampered/expired signatures,
 * that granting or revoking a role on chain is honoured by the provider, and
 * that a signed walk of the bucket-id space is refused without disturbing a
 * real member.
 * What this workflow adds over the Rust tests is the wiring — real client-built
 * signatures, real status codes, real chain events — not the cache mechanics,
 * which are covered deterministically in crates/providers/auth and
 * provider-node/tests/coordinators/membership.rs.
 *
 * Workflow 06 covers the same ACL on-chain; this one is the off-chain half —
 * it is the only workflow that sends an `Authorization` header.
 *
 * Usage: node e2e/12-provider-http-auth.js [chain_ws] [provider_url]
 */

import assert from "node:assert";
import {
  bytesToBase64,
  computeCid,
  ensureProviderRegistered,
  makeSigner,
  putChunk,
  removeMember,
  setMember,
  signProviderRequest,
  toHex,
  type ChainSigner,
} from "@web3-storage/sdk";
import { negotiateAndEstablish, runSuite, setupChain } from "./helpers.js";

const CHAIN_WS = process.argv[2] || "ws://127.0.0.1:2222";
const PROVIDER_URL = process.argv[3] || "http://127.0.0.1:3333";

/**
 * How long a membership change may take to reach the provider: the event feed
 * normally lands it a block or two after finality, and the membership cache's
 * TTL (`--auth-cache-ttl`, 30s by default) is the backstop if the feed ever
 * missed it — so allow for either.
 *
 * Which of the two did the work is deliberately not asserted here. Over HTTP
 * the mechanisms are separable only by timing, and only on a chain that
 * finalizes faster than the TTL, so an assertion about it would pass or fail on
 * the chain's finality speed rather than on the code. That is pinned instead
 * where it is deterministic: `membership_event_forces_the_next_lookup_to_re_resolve`
 * in provider-node/tests/coordinators/membership.rs runs with a 300s TTL, so
 * only an event can explain the re-resolve it observes.
 */
const CHANGE_DEADLINE_MS = 40_000;

/**
 * How many distinct bucket ids 12.13 walks - deliberately above
 * `--auth-cache-max-entries`' 10,000 default, so the walk reaches the cache's
 * ceiling rather than just exercising the insert path. Sequential, one chain
 * storage query per id: ~11s against a local dev chain.
 *
 * The provider does not report its cache config, so that coupling cannot be
 * asserted here: raise the default past this number and the test quietly
 * weakens to a smoke test.
 */
const SCAN_IDS = 12_000;

/** Send one request and return just the status; the body is drained and dropped. */
async function statusOf(method: string, path: string, init: RequestInit = {}): Promise<number> {
  const res = await fetch(new URL(path, PROVIDER_URL), { method, ...init });
  await res.text();
  return res.status;
}

interface SignedProbe {
  /** Verb to sign, when it must differ from the one sent (tamper case). */
  signMethod?: string;
  /** Bucket to sign for, when it must differ from the one addressed (tamper case). */
  signBucket?: bigint;
  /** JSON body; sent with `Content-Type: application/json`. */
  body?: string;
}

/**
 * Status of a request signed by `who` and addressed at `bucketId`. The signed
 * verb and bucket default to the ones actually sent, so overriding either is
 * how the tamper cases below prove the signature is bound to both.
 */
async function signedStatus(
  who: ChainSigner,
  method: string,
  path: string,
  bucketId: bigint,
  { signMethod = method, signBucket = bucketId, body }: SignedProbe = {},
): Promise<number> {
  const headers: Record<string, string> = await signProviderRequest(
    who.signer,
    signMethod,
    signBucket,
  );
  if (body) headers["Content-Type"] = "application/json";
  return statusOf(method, path, { headers, body });
}

/**
 * `signProviderRequest` always stamps `now`, so a skew case has to build the
 * header by hand — same format, a timestamp `ageSeconds` in the past.
 */
async function staleAuthHeader(
  who: ChainSigner,
  method: string,
  bucketId: bigint,
  ageSeconds: number,
): Promise<Record<string, string>> {
  const timestamp = String(Math.floor(Date.now() / 1000) - ageSeconds);
  const message = `web3storage:${method}:${bucketId}:${timestamp}`;
  const signature = await who.signer.signBytes(new TextEncoder().encode(message));
  return { Authorization: `Web3Storage ${toHex(who.publicKey)}:${toHex(signature)}:${timestamp}` };
}

/** Reader-guarded endpoint: the S3 index root, which any member may read. */
const readPath = (bucketId: bigint) => `/s3/${bucketId}/index_root`;

/**
 * Body for the Writer-guarded `PUT /node`. It has to be well-formed: axum
 * parses the JSON before the role check runs, so a junk body would 400 without
 * ever reaching the guard under test.
 */
function nodeBody(bucketId: bigint, text: string): string {
  const bytes = new TextEncoder().encode(text);
  return JSON.stringify({
    bucket_id: Number(bucketId),
    hash: toHex(computeCid(bytes)),
    data: bytesToBase64(bytes),
    children: null,
  });
}

/**
 * Body for the Admin-guarded `POST /delete` (L0 prune). Only ever sent by a
 * non-admin here: a prune that got past the guard would rewrite the bucket's
 * MMR, so the authorized path stays with the storage tests.
 */
const pruneBody = (bucketId: bigint) =>
  JSON.stringify({ bucket_id: Number(bucketId), new_start_seq: 0, nonce: 0 });

/** Poll `probe` until it returns `want`; fail if the deadline passes first. */
async function waitForStatus(
  want: number,
  probe: () => Promise<number>,
  deadlineMs = CHANGE_DEADLINE_MS,
): Promise<number> {
  const started = Date.now();
  let last: number | null = null;
  while (Date.now() - started < deadlineMs) {
    last = await probe();
    if (last === want) return Date.now() - started;
    await new Promise((resolve) => setTimeout(resolve, 500));
  }
  assert.fail(`expected status ${want} within ${deadlineMs}ms, last saw ${last}`);
}

/** Assert the provider honours an on-chain membership change, and say how fast. */
async function assertChangeHonoured(want: number, probe: () => Promise<number>): Promise<void> {
  const elapsed = await waitForStatus(want, probe);
  console.log(`          ${want} ${elapsed}ms after the change`);
}

async function main() {
  const provider = makeSigner("//Alice");
  const admin = makeSigner("//Dave");
  const writer = makeSigner("//Eve");
  const reader = makeSigner("//Ferdie");
  const stranger = makeSigner("//Charlie");

  const { papi, api } = await setupChain(CHAIN_WS);
  await ensureProviderRegistered(api, provider, PROVIDER_URL);

  let bucketId: bigint;

  const tests: Array<{ name: string; fn: () => Promise<void> }> = [];

  // ── Setup ─────────────────────────────────────────────────────────────────

  tests.push({
    name: "12.0 Bucket with one member per role",
    fn: async () => {
      // Finalized throughout: every assertion below is an HTTP request the
      // provider authorizes from chain state, so each membership change has to
      // be visible to it before the next probe rather than a block later.
      ({ bucketId } = await negotiateAndEstablish(
        api,
        PROVIDER_URL,
        admin,
        provider,
        { maxBytes: 1_048_576n, duration: 100 },
        true,
      ));
      await setMember(api, admin, bucketId, writer, "Writer", { mode: "finalized" });
      await setMember(api, admin, bucketId, reader, "Reader", { mode: "finalized" });
    },
  });

  // ── Role ladder over HTTP ─────────────────────────────────────────────────

  tests.push({
    name: "12.1 Admin reads",
    fn: async () => {
      assert.strictEqual(await signedStatus(admin, "GET", readPath(bucketId), bucketId), 200);
    },
  });

  tests.push({
    name: "12.2 Reader reads",
    fn: async () => {
      assert.strictEqual(await signedStatus(reader, "GET", readPath(bucketId), bucketId), 200);
    },
  });

  tests.push({
    name: "12.3 Writer writes",
    fn: async () => {
      // Through the SDK rather than a raw probe: the happy path is worth
      // exercising exactly as a client drives it, header building included.
      const { hash } = await putChunk(PROVIDER_URL, bucketId, "hello from workflow 12", writer);
      assert.ok(hash.startsWith("0x"), "writer's upload should be accepted");
    },
  });

  tests.push({
    name: "12.4 Reader may not write",
    fn: async () => {
      const status = await signedStatus(reader, "PUT", "/node", bucketId, {
        body: nodeBody(bucketId, "reader should not be able to store this"),
      });
      assert.strictEqual(status, 403, "a Reader must not satisfy a Writer endpoint");
    },
  });

  tests.push({
    name: "12.5 Writer may not prune",
    fn: async () => {
      const status = await signedStatus(writer, "POST", "/delete", bucketId, {
        body: pruneBody(bucketId),
      });
      assert.strictEqual(status, 403, "a Writer must not satisfy an Admin endpoint");
    },
  });

  tests.push({
    name: "12.6 Non-member is refused",
    fn: async () => {
      const status = await signedStatus(stranger, "GET", readPath(bucketId), bucketId);
      assert.strictEqual(status, 403, "a validly signed non-member must still be refused");
    },
  });

  // ── Signature and timestamp checks ────────────────────────────────────────

  tests.push({
    name: "12.7 Unsigned request is rejected",
    fn: async () => {
      assert.strictEqual(await statusOf("GET", readPath(bucketId)), 401);
    },
  });

  tests.push({
    name: "12.8 Timestamp outside max_skew is rejected",
    fn: async () => {
      // An hour old, against the 300s `--auth-max-skew` default: a captured
      // header must not stay replayable indefinitely.
      const headers = await staleAuthHeader(reader, "GET", bucketId, 3600);
      assert.strictEqual(await statusOf("GET", readPath(bucketId), { headers }), 401);
    },
  });

  tests.push({
    name: "12.9 Signature is bound to the HTTP method",
    fn: async () => {
      // Eve holds Writer, so only the method mismatch can fail this: the
      // provider rebuilds the message from the verb it actually received.
      const status = await signedStatus(writer, "PUT", "/node", bucketId, {
        signMethod: "GET",
        body: nodeBody(bucketId, "signed as a read, sent as a write"),
      });
      assert.strictEqual(status, 401, "a GET signature must not authorize a PUT");
    },
  });

  tests.push({
    name: "12.10 Signature is bound to the bucket",
    fn: async () => {
      const status = await signedStatus(writer, "PUT", "/node", bucketId, {
        signBucket: bucketId + 1n,
        body: nodeBody(bucketId, "signed for a different bucket"),
      });
      assert.strictEqual(status, 401, "a signature for another bucket must not authorize this one");
    },
  });

  // ── Cache invalidation from chain events ──────────────────────────────────

  tests.push({
    name: "12.11 Removal is honoured",
    fn: async () => {
      // Establish the starting point: the reader is authorized *and* cached, so
      // the 403 below has to come from the removal rather than from a cold
      // cache that never held them.
      assert.strictEqual(
        await signedStatus(reader, "GET", readPath(bucketId), bucketId),
        200,
        "reader should still be authorized before the removal",
      );

      await removeMember(api, admin, bucketId, reader, { mode: "finalized" });

      await assertChangeHonoured(403, () =>
        signedStatus(reader, "GET", readPath(bucketId), bucketId),
      );
    },
  });

  tests.push({
    name: "12.12 Re-adding is honoured",
    fn: async () => {
      // The granting direction: a member added back must not have to wait out a
      // cached set that excludes them.
      assert.strictEqual(
        await signedStatus(reader, "GET", readPath(bucketId), bucketId),
        403,
        "reader should still be refused before being re-added",
      );

      await setMember(api, admin, bucketId, reader, "Reader", { mode: "finalized" });

      await assertChangeHonoured(200, () =>
        signedStatus(reader, "GET", readPath(bucketId), bucketId),
      );
    },
  });

  // ── Bucket-id scan against the membership cache ───────────────────────────

  tests.push({
    name: "12.13 A signed bucket-id scan is refused and leaves a member authorized",
    fn: async () => {
      // Any keypair can walk the id space: auth runs before any
      // bucket-existence check and the read routes are not rate limited, so
      // each distinct id caches one membership entry. Past the cache's ceiling
      // the walk must still be refused and must not cost a member their
      // access. Resident counts are asserted in
      // crates/providers/auth/src/membership.rs instead - the provider reports
      // no cache size.
      const base = bucketId + 1_000_000n; // clear of every id this suite creates
      const started = Date.now();
      for (let i = 0n; i < BigInt(SCAN_IDS); i++) {
        const scanned = base + i;
        const status = await signedStatus(stranger, "GET", readPath(scanned), scanned);
        assert.strictEqual(status, 403, `scanned bucket ${scanned} should be refused`);
      }
      console.log(`          ${SCAN_IDS} ids scanned in ${Date.now() - started}ms`);

      // Whether Ferdie's entry was evicted or kept is invisible here, and that
      // is fine: an evicted entry just re-resolves from chain.
      assert.strictEqual(
        await signedStatus(reader, "GET", readPath(bucketId), bucketId),
        200,
        "a member must still be authorized after a bucket-id scan",
      );

      // Refusing every id is no good if the node dies holding them all.
      assert.strictEqual(await statusOf("GET", "/health"), 200, "provider should still be healthy");
    },
  });

  await runSuite("12 — Provider HTTP Auth", tests, { api, papi });
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
