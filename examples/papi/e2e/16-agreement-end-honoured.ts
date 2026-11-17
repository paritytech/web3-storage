// SPDX-License-Identifier: Apache-2.0

/**
 * E2E Workflow 16 — Ending an Agreement Is Honoured
 *
 * Accounts: //Alice (provider node 1), //Charlie (provider node 2),
 * //Dave (bucket owner and admin), //Ferdie (sentinel member)
 *
 * Needs two provider nodes (`just start-second-provider`). The provider keeps
 * its own agreement on a bucket in the cached bucket access, so an
 * `AgreementEnded` event has to drop that cache entry. Tests cover:
 * - 16.1-16.2: a primary agreement ended by its admin. //Alice then refuses
 *   the first upload to the bucket with `no_agreement`;
 * - 16.3-16.4: a replica agreement ended by its owner after expiry. //Alice
 *   served the private bucket to everyone as a replica and then asks for a
 *   signature.
 *
 * Each check follows the same steps. Read the bucket so the provider caches
 * its access. End the agreement. Wait until the provider has applied a later
 * block, using a membership change on a sentinel bucket (the provider applies
 * chain events in block order). Then probe once, so the probe cannot pass by
 * racing the provider's event feed.
 *
 * What stays out: once a bucket has local data, the provider keeps accepting
 * writes after its agreement ends, so the probe uses a bucket with no local
 * data.
 *
 * Usage: node e2e/16-agreement-end-honoured.js [chain_ws] [provider_url] [second_provider_url]
 */

import assert from "node:assert";
import {
  addReplicaProvider,
  endAgreement,
  ensureProviderRegistered,
  getCommitment,
  getProviderNodeInfo,
  HttpError,
  makeSigner,
  putChunk,
  READ_OPTS,
  removeMember,
  setMember,
  updateProviderMultiaddr,
  updateProviderSettings,
  waitForRelayBlock,
  type ChainSigner,
} from "@web3-storage/sdk";
import { negotiateAndEstablish, negotiateSigned, runSuite, setupChain } from "./helpers.js";

const CHAIN_WS = process.argv[2] || "ws://127.0.0.1:2222";
const PROVIDER_URL = process.argv[3] || "http://127.0.0.1:3333";
const SECOND_PROVIDER_URL = process.argv[4] || "http://127.0.0.1:3334";

/** Per-request ceiling, so a hung provider fails the test instead of the runner. */
const REQUEST_TIMEOUT_MS = 30_000;

/** How long a membership change may take to reach the provider. */
const CHANGE_DEADLINE_MS = 40_000;

interface Reply {
  status: number;
  /** The provider's error message, when the request failed. */
  error?: string;
}

/** The status and message of a failed provider request; anything else is rethrown. */
function failedReply(err: unknown): Reply {
  if (err instanceof HttpError) return { status: err.status, error: err.message };
  throw err;
}

/** `PUT /node` storing `text` in `bucketId`, signed by `who`. */
async function write(who: ChainSigner, bucketId: bigint, text: string): Promise<Reply> {
  try {
    await putChunk(PROVIDER_URL, bucketId, text, who);
    return { status: 200 };
  } catch (err) {
    return failedReply(err);
  }
}

/** `GET /commitment` for `bucketId`, signed by `who` or unsigned; 404 when there is no local data. */
async function readCommitment(bucketId: bigint, who?: ChainSigner): Promise<Reply> {
  try {
    const commitment = await getCommitment(PROVIDER_URL, bucketId, {
      signer: who,
      signal: AbortSignal.timeout(REQUEST_TIMEOUT_MS),
    });
    return { status: commitment === null ? 404 : 200 };
  } catch (err) {
    return failedReply(err);
  }
}

/** Poll `probe` until `done` accepts its reply. */
async function waitForReply(
  probe: () => Promise<Reply>,
  done: (reply: Reply) => boolean,
  what: string,
): Promise<void> {
  const started = Date.now();
  let last: Reply | undefined;
  while (Date.now() - started < CHANGE_DEADLINE_MS) {
    last = await probe();
    if (done(last)) return;
    await new Promise((resolve) => setTimeout(resolve, 500));
  }
  assert.fail(`${what} within ${CHANGE_DEADLINE_MS}ms, last saw ${JSON.stringify(last)}`);
}

async function main() {
  const first = makeSigner("//Alice");
  const second = makeSigner("//Charlie");
  const owner = makeSigner("//Dave");
  const sentinelMember = makeSigner("//Ferdie");

  const { papi, api } = await setupChain(CHAIN_WS);
  try {
    await getProviderNodeInfo(SECOND_PROVIDER_URL);
  } catch (err) {
    throw new Error(
      `Second provider is not reachable at ${SECOND_PROVIDER_URL} ` +
        `(${(err as Error).message}). Start it with 'just start-second-provider'.`,
    );
  }
  await ensureProviderRegistered(api, first, PROVIDER_URL);
  await ensureProviderRegistered(api, second, SECOND_PROVIDER_URL);
  const secondAddr = `/ip4/127.0.0.1/tcp/${new URL(SECOND_PROVIDER_URL).port}`;
  const secondInfo = await api.query.StorageProvider.Providers.getValue(second.address, READ_OPTS);
  if (new TextDecoder().decode(secondInfo!.multiaddr) !== secondAddr) {
    await updateProviderMultiaddr(api, second, secondAddr);
  }

  const maxBytes = 1_048_576n;

  // A bucket whose agreement stays active. //Ferdie's membership on it is
  // toggled to learn that //Alice has applied a block.
  let sentinel: bigint;
  let sentinelHasMember = false;

  /**
   * Returns once //Alice has applied a block later than the caller's last
   * transaction. The sentinel's cached access is read just before the change,
   * so the change only becomes visible through the event feed.
   */
  async function providerHasCaughtUp(): Promise<void> {
    const wasMember = sentinelHasMember;
    const expected = wasMember ? 403 : 200;
    const before = wasMember ? 200 : 403;
    assert.strictEqual(
      (await write(sentinelMember, sentinel, "before the barrier")).status,
      before,
      "the sentinel must be cached in the state the barrier changes",
    );
    if (wasMember) {
      await removeMember(api, owner, sentinel, sentinelMember, {
        mode: "finalized",
      });
    } else {
      await setMember(api, owner, sentinel, sentinelMember, "Writer", {
        mode: "finalized",
      });
    }
    sentinelHasMember = !wasMember;
    await waitForReply(
      () => write(sentinelMember, sentinel, "after the barrier"),
      (reply) => reply.status === expected,
      `sentinel write to become ${expected}`,
    );
  }

  let primaryBucket: bigint;
  let replicaBucket: bigint;

  const tests: Array<{ name: string; fn: () => Promise<void> }> = [];

  // ── Setup ─────────────────────────────────────────────────────────────────

  tests.push({
    name: "16.0 Sentinel bucket on //Alice",
    fn: async () => {
      // Finalized throughout: the provider reads chain state from finalized
      // blocks, so each change has to be visible before the next probe.
      ({ bucketId: sentinel } = await negotiateAndEstablish(
        api,
        PROVIDER_URL,
        owner,
        first,
        { maxBytes, duration: 100 },
        true,
      ));
    },
  });

  // ── Primary agreement ─────────────────────────────────────────────────────

  tests.push({
    name: "16.1 Ended primary agreement: the first upload is refused",
    fn: async () => {
      ({ bucketId: primaryBucket } = await negotiateAndEstablish(
        api,
        PROVIDER_URL,
        owner,
        first,
        { maxBytes, duration: 100 },
        true,
      ));

      // Reading the bucket caches its access without creating local data.
      const read = await readCommitment(primaryBucket, owner);
      assert.strictEqual(read.status, 404, "the bucket has no local data yet");

      await endAgreement(api, owner, first, primaryBucket, "Pay", undefined, {
        mode: "finalized",
      });
      await providerHasCaughtUp();

      const refused = await write(owner, primaryBucket, "after the agreement ended");
      assert.strictEqual(refused.status, 403, "the provider must refuse the first upload");
      assert.ok(
        refused.error?.includes("no_agreement"),
        `expected no_agreement, got ${refused.error}`,
      );
    },
  });

  tests.push({
    name: "16.2 Ended primary agreement: nothing was stored",
    fn: async () => {
      const read = await readCommitment(primaryBucket, owner);
      assert.strictEqual(read.status, 404, "a refused upload must not create the bucket");

      // The sentinel keeps its agreement and keeps accepting writes, so the
      // refusal above comes from the ended agreement and not from the node.
      assert.strictEqual((await write(owner, sentinel, "sentinel still writable")).status, 200);
    },
  });

  // ── Replica agreement ─────────────────────────────────────────────────────

  tests.push({
    name: "16.3 Replica agreement: the private bucket is served to everyone",
    fn: async () => {
      // //Alice must accept replicas.
      const info = await api.query.StorageProvider.Providers.getValue(first.address, READ_OPTS);
      await updateProviderSettings(api, first, {
        ...info!.settings,
        replica_sync_price: 1n,
      });
      // //Charlie holds the bucket as its primary; //Alice replicates it.
      ({ bucketId: replicaBucket } = await negotiateAndEstablish(
        api,
        SECOND_PROVIDER_URL,
        owner,
        second,
        { maxBytes, duration: 200 },
        true,
      ));
      // The node rejects a replica quote until it has seen the setting.
      const started = Date.now();
      let terms: Awaited<ReturnType<typeof negotiateSigned>>;
      for (;;) {
        try {
          terms = await negotiateSigned(api, PROVIDER_URL, owner, first, {
            maxBytes,
            duration: 20,
            bucketId: replicaBucket,
            // Plain numbers and an explicit sync_price: the provider decodes
            // replica terms as JSON numbers and signs them as sent.
            replicaParams: {
              sync_balance: 10_000_000_000,
              min_sync_interval: 10,
              sync_price: 1,
            },
          });
          break;
        } catch (err) {
          if (Date.now() - started > CHANGE_DEADLINE_MS) throw err;
          await new Promise((resolve) => setTimeout(resolve, 1000));
        }
      }
      await addReplicaProvider(api, owner, first, terms, { mode: "finalized" });

      // A private bucket needs a signature on a primary. A replica serves
      // everyone, so the unsigned read is not refused.
      await waitForReply(
        () => readCommitment(replicaBucket),
        (reply) => reply.status !== 401,
        "the replica to serve the private bucket without a signature",
      );
    },
  });

  tests.push({
    name: "16.4 Ended replica agreement: the private bucket asks for a signature",
    fn: async () => {
      assert.ok(replicaBucket !== undefined, "16.3 must pass first");
      const agreement = (await api.query.StorageProvider.StorageAgreements.getValue(
        replicaBucket,
        first.address,
        READ_OPTS,
      ))!;
      console.log("    Waiting for expiry at block %d...", Number(agreement.expires_at));
      await waitForRelayBlock(papi, api, Number(agreement.expires_at));

      // Cached again just before the end, so the change has to arrive as an event.
      assert.notStrictEqual((await readCommitment(replicaBucket)).status, 401);

      // A replica agreement can only end after expiry, by its owner.
      await endAgreement(api, owner, first, replicaBucket, "Pay", undefined, {
        mode: "finalized",
      });
      await providerHasCaughtUp();

      const reply = await readCommitment(replicaBucket);
      assert.strictEqual(
        reply.status,
        401,
        "without its agreement //Alice no longer serves the private bucket to everyone",
      );
    },
  });

  await runSuite("16 — Agreement End Is Honoured", tests, { api, papi });
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
