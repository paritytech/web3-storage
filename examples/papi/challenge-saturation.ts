// SPDX-License-Identifier: Apache-2.0

/**
 * Challenge-saturation prototype for paritytech/web3-storage#267.
 *
 * Answers the headline stress-test unknown: what does a wave of maturing
 * challenges cost, and can `MaxChallengesPerDeadline` (1,000) even be reached?
 *
 * Every challenge created while the chain sits on relay parent `r` matures at
 * relay block `r + ChallengeTimeout` (`pallet/src/impls/challenges.rs`) — all
 * deadlines are RELAY chain blocks, the pallet's anchor clock. If the provider
 * never responds, the `on_initialize` sweep drains the deadline key once it
 * becomes unrespondable, slashing at most `min(MaxChallengesPerDeadline,
 * MAX_SWEEP_SLASH_BUDGET = 100)` challenges per parachain block and carrying
 * the remainder over via the `LastSweptChallengeBlock` cursor. A fully loaded
 * deadline therefore drains over several consecutive blocks, not one. This
 * script:
 *
 *  1. creates N challenges in a single block (one `Utility.force_batch` of
 *     `challenge_offchain`, all reusing one uploaded commitment) so they share
 *     a relay-block deadline;
 *  2. leaves them undefended and waits for the relay chain to pass the
 *     deadline;
 *  3. measures every sweep block until the slash count reconciles: on-chain
 *     `System.BlockWeight`, block time, slash events, provider balance
 *     trajectory, and residual state after the sweep.
 *
 * It emits a JSON result (`--json`) alongside a human summary. This is the
 * "provider ignores all" flavor; the "provider defends all" flavor (proof
 * generation + `respond_to_challenge` per challenge) is a follow-up.
 *
 * Prereqs: a running chain and provider node (`just start-chain` +
 * `just start-provider`). Run `lower-timeouts.ts --profile fast` first so the
 * deadline is minutes, not days, away.
 *
 * Usage:
 *   pnpm --filter web3-storage-papi-demo run challenge-saturation -- [options]
 *
 *   --chain-rpc <url>     default ws://127.0.0.1:2222
 *   --provider-url <url>  default http://127.0.0.1:3333
 *   --provider-seed <s>   provider account, default //Alice
 *   --challenger-seed <s> challenger account, default //Bob
 *   --count <n>           challenges to create at one deadline, default 800
 *   --json <path>         also write the result object as JSON
 */

import { parseArgs } from "node:util";
import { writeFileSync } from "node:fs";

import type { PolkadotClient } from "polkadot-api";
import { Twox128, u32 } from "@polkadot-api/substrate-bindings";
import {
  addStake,
  currentRelayBlock,
  disconnect,
  ensureProviderRegistered,
  makeSigner,
  READ_OPTS,
  submitTx,
  toHex,
  uploadChunk,
  waitForBlock,
  waitForRelayBlock,
  type ChainSigner,
  type ParachainApi,
} from "@web3-storage/sdk";
import { negotiateAndEstablish, setupChain } from "./e2e/helpers.js";
import { ensureSoleAcceptingProvider } from "./support.js";

// Benchmarked weights from
// `runtimes/web3-storage-local/src/weights/pallet_storage_provider.rs`, used
// only for the pre-flight estimate. Picoseconds of ref_time and bytes of
// proof size (PoV).
const BENCH = {
  // challenge_off_chain(): per-challenge creation cost.
  createRefTime: 111_789_000,
  createProof: 4148,
  // on_initialize_slash_challenges(c): the sweep cost, per challenge slashed.
  sweepRefTimePerChallenge: 69_473_177,
  sweepProofPerChallenge: 5206,
  sweepRefTimeBase: 11_535_000,
  sweepProofBase: 3639,
};
// pallet/src/lib.rs MAX_SWEEP_SLASH_BUDGET: the sweep slashes at most this
// many challenges per parachain block (MaxChallengesPerDeadline is larger, so
// it is the effective budget here) and parks its cursor to resume next block.
const SWEEP_SLASH_BUDGET = 100;
// runtimes/web3-storage-local/src/constants.rs: 2s of ref_time, 5 MiB PoV, and
// the 75% Normal-dispatch ratio that user extrinsics (the create path) live in.
const MAX_BLOCK_REF_TIME = 2_000_000_000_000;
const MAX_POV = 5 * 1024 * 1024;
const NORMAL_RATIO = 0.75;
const MiB = 1024 * 1024;

function parseCli() {
  const { values } = parseArgs({
    options: {
      "chain-rpc": { type: "string", default: "ws://127.0.0.1:2222" },
      "provider-url": { type: "string", default: "http://127.0.0.1:3333" },
      "provider-seed": { type: "string", default: "//Alice" },
      "challenger-seed": { type: "string", default: "//Bob" },
      count: { type: "string", default: "800" },
      json: { type: "string" },
      help: { type: "boolean", default: false },
    },
  });
  if (values.help) {
    console.log(USAGE);
    process.exit(0);
  }
  const count = Number(values.count);
  if (!Number.isInteger(count) || count < 1) {
    throw new Error(`--count must be a positive integer, got ${values.count}`);
  }
  return {
    chainRpc: values["chain-rpc"],
    providerUrl: values["provider-url"],
    providerSeed: values["provider-seed"],
    challengerSeed: values["challenger-seed"],
    count,
    jsonPath: values.json,
  };
}

const USAGE = `challenge-saturation: create N challenges at one deadline and measure the sweep

  --chain-rpc <url>     default ws://127.0.0.1:2222
  --provider-url <url>  default http://127.0.0.1:3333
  --provider-seed <s>   provider account, default //Alice
  --challenger-seed <s> challenger account, default //Bob
  --count <n>           challenges to create at one deadline, default 800
  --json <path>         also write the result object as JSON

Run lower-timeouts.ts --profile fast first so the deadline is minutes away.`;

interface ClassWeight {
  refTime: bigint;
  proof: bigint;
}
interface BlockWeight {
  normal: ClassWeight;
  operational: ClassWeight;
  mandatory: ClassWeight;
  totalRefTime: bigint;
  totalProof: bigint;
}

async function readBlockWeight(api: ParachainApi, at: string): Promise<BlockWeight> {
  const w = await api.query.System.BlockWeight.getValue({ at });
  const cls = (c: { ref_time: bigint; proof_size: bigint }): ClassWeight => ({
    refTime: c.ref_time,
    proof: c.proof_size,
  });
  const normal = cls(w.normal);
  const operational = cls(w.operational);
  const mandatory = cls(w.mandatory);
  return {
    normal,
    operational,
    mandatory,
    totalRefTime: normal.refTime + operational.refTime + mandatory.refTime,
    totalProof: normal.proof + operational.proof + mandatory.proof,
  };
}

async function timestampAt(api: ParachainApi, at: string): Promise<number> {
  return Number(await api.query.Timestamp.Now.getValue({ at }));
}

/**
 * Effective `ChallengeTimeout` (relay blocks) as the pallet sees it. The
 * storage-backed param lives at `twox128(":ChallengeTimeout:")` and overrides
 * the compiled default; `api.constants` only exposes the compile-time value,
 * so it can't be trusted once `lower-timeouts.ts` has run. Falls back to the
 * constant when unset.
 */
async function effectiveChallengeTimeout(papi: PolkadotClient, api: ParachainApi): Promise<number> {
  const key = toHex(Twox128(new TextEncoder().encode(":ChallengeTimeout:")));
  const raw = await papi._request<string | null>("state_getStorage", [key]);
  if (raw != null) return u32.dec(raw);
  return Number(await api.constants.StorageProvider.ChallengeTimeout());
}

/** Canonical block hash for a height, via the legacy RPC (chainHead has no by-number lookup). */
async function blockHashAt(papi: PolkadotClient, height: number): Promise<string> {
  const hash = await papi._request<string>("chain_getBlockHash", [height]);
  if (!hash) throw new Error(`no block hash for height ${height}`);
  return hash;
}

function countSlashEvents(records: any[]): number {
  return records.filter(
    (r) => r.event?.type === "StorageProvider" && r.event?.value?.type === "ChallengeSlashed",
  ).length;
}

/** Provider stake + free/reserved balance, the quantities a slash moves. */
async function providerBalances(api: ParachainApi, provider: ChainSigner, at = "best") {
  const acc = await api.query.System.Account.getValue(provider.address, { at });
  const info = await api.query.StorageProvider.Providers.getValue(provider.address, { at });
  return {
    free: acc.data.free,
    reserved: acc.data.reserved,
    stake: info?.stake ?? 0n,
  };
}

function fmtMiB(bytes: bigint | number): string {
  return `${(Number(bytes) / MiB).toFixed(2)} MiB`;
}
function pct(x: number): string {
  return `${(x * 100).toFixed(0)}%`;
}

function preflight(count: number, maxPerDeadline: number, challengeTimeout: number) {
  const normalPov = MAX_POV * NORMAL_RATIO;
  const normalRef = MAX_BLOCK_REF_TIME * NORMAL_RATIO;
  const createProof = BENCH.createProof * count;
  const createRef = BENCH.createRefTime * count;
  const maxCreatableByPov = Math.floor(normalPov / BENCH.createProof);
  const sweepBlocks = Math.ceil(count / SWEEP_SLASH_BUDGET);
  const perBlock = Math.min(count, SWEEP_SLASH_BUDGET);
  const sweepProof = BENCH.sweepProofPerChallenge * perBlock + BENCH.sweepProofBase;
  const sweepRef = BENCH.sweepRefTimePerChallenge * perBlock + BENCH.sweepRefTimeBase;

  console.log("Pre-flight (from benchmarked weights):");
  console.log(
    `  MaxChallengesPerDeadline = ${maxPerDeadline}, ChallengeTimeout = ${challengeTimeout} relay blocks`,
  );
  console.log(
    `  creating ${count}: PoV ${fmtMiB(createProof)} (${pct(createProof / normalPov)} of Normal budget), ` +
      `ref_time ${pct(createRef / normalRef)} of Normal budget`,
  );
  console.log(
    `  Normal-class PoV budget caps single-block creation at ~${maxCreatableByPov} ` +
      `challenge_offchain calls`,
  );
  console.log(
    `  sweep: ${SWEEP_SLASH_BUDGET} slashes/block budget → expect ~${sweepBlocks} sweep block(s); ` +
      `a full block sweeps PoV ${fmtMiB(sweepProof)} (${pct(sweepProof / MAX_POV)} of block PoV), ` +
      `ref_time ${pct(sweepRef / MAX_BLOCK_REF_TIME)} of block`,
  );
  if (count > maxCreatableByPov) {
    console.log(
      `  ⚠️  count ${count} exceeds the ~${maxCreatableByPov} single-block ceiling; the ` +
        `force_batch will not fit one block and the run will stall. Lower --count.`,
    );
  }
  console.log("");
}

interface SweepBlock {
  height: number;
  hash: string;
  slashes: number;
  blockTimeMs: number;
  weight: BlockWeight;
}

async function main() {
  const opts = parseCli();
  const provider = makeSigner(opts.providerSeed);
  const challenger = makeSigner(opts.challengerSeed);

  const { papi, api } = await setupChain(opts.chainRpc);
  try {
    const challengeTimeout = await effectiveChallengeTimeout(papi, api);
    const maxPerDeadline = Number(await api.constants.StorageProvider.MaxChallengesPerDeadline());
    const count = Math.min(opts.count, maxPerDeadline);
    if (count < opts.count) {
      console.log(`Capping count ${opts.count} to MaxChallengesPerDeadline ${maxPerDeadline}\n`);
    }
    preflight(count, maxPerDeadline, challengeTimeout);

    if (challengeTimeout > 600) {
      console.log(
        `⚠️  ChallengeTimeout is ${challengeTimeout} relay blocks (~${Math.round(
          (challengeTimeout * 6) / 60,
        )} min at 6s). Run lower-timeouts.ts --profile fast first for a quick run.\n`,
      );
    }

    // ── Setup: provider + agreement + one signed commitment ──────────────────
    console.log("Setup: provider, bucket, agreement, upload...");
    await ensureProviderRegistered(api, provider, opts.providerUrl);
    const restore = await ensureSoleAcceptingProvider(api, provider);

    // Every run slashes the provider's whole stake to 0, so a second run on the
    // same chain would fail the agreement's stake check. Top back up to a fixed
    // target (the slash leaves the provider registered, just unstaked).
    const STAKE_TARGET = 2_000_000_000_000_000n;
    const stakeNow = (
      await api.query.StorageProvider.Providers.getValue(provider.address, READ_OPTS)
    )?.stake;
    if ((stakeNow ?? 0n) < STAKE_TARGET) {
      await addStake(api, provider, STAKE_TARGET - (stakeNow ?? 0n), {
        label: "top-up provider stake",
      });
    }

    // Agreement must outlive creation (challengeability is checked at creation,
    // not at the deadline). Durations are relay blocks; keep it comfortably
    // longer than the whole run. Finalized: the provider node reads bucket
    // membership from finalized state, and the upload follows immediately.
    const duration = challengeTimeout * 2 + 400;
    const { bucketId } = await negotiateAndEstablish(
      api,
      opts.providerUrl,
      challenger,
      provider,
      { maxBytes: 1_048_576n, duration },
      true,
    );

    // The commitment nonce must be the relay block (the pallet's recency check
    // runs on the anchor clock), never `System.Number`.
    const uploadNonce = await currentRelayBlock(api);
    const upload = await uploadChunk(
      opts.providerUrl,
      bucketId,
      `saturation @ ${uploadNonce}`,
      uploadNonce,
      challenger,
    );
    const commit = upload.commit;

    // ── Create N challenges in one block, sharing one deadline ───────────────
    // One signed commitment backs every challenge: `challenge_offchain` re-checks
    // the signature but does not dedupe by target, so varying leaf_index yields
    // distinct challenges from a single upload.
    console.log(`Creating ${count} challenges in one force_batch...`);
    const calls = Array.from({ length: count }, (_, i) =>
      api.tx.StorageProvider.challenge_offchain({
        bucket_id: bucketId,
        provider: provider.address,
        commitment: {
          mmr_root: commit.mmr_root,
          start_seq: BigInt(commit.start_seq),
          leaf_count: BigInt(commit.leaf_count),
        },
        target: { leaf_index: BigInt(i % Number(commit.leaf_count || 1)), chunk_index: 0n },
        nonce: BigInt(commit.nonce),
        provider_signature: { type: "Sr25519", value: commit.provider_signature } as never,
      }).decodedCall,
    );

    const balancesBefore = await providerBalances(api, provider);

    const batchResult = await submitTx(
      api.tx.Utility.force_batch({ calls }),
      challenger.signer,
      { label: `force_batch(${count} challenge_offchain)`, mode: "best", timeoutMs: 600_000 },
    );
    const createdEvents = api.event.StorageProvider.ChallengeCreated.filter(
      batchResult.events as never,
    );
    const created = createdEvents.length;
    if (created === 0) throw new Error("no challenges created; check provider/agreement setup");
    const createdBlockHash = (batchResult as { block?: { hash?: string } }).block?.hash;
    if (!createdBlockHash) throw new Error("force_batch result carried no block hash");
    const createdHeader = await papi._request<{ number: string }>("chain_getHeader", [
      createdBlockHash,
    ]);
    const createdHeight = Number(createdHeader.number);
    // Deadline comes from the event, not `relay_now + challengeTimeout`: it is
    // the pallet's own `respond_by` (a RELAY block, the pallet's anchor clock),
    // immune to any timeout-read skew. All challenges made in one block share it.
    const deadline = Number(createdEvents[0].payload.respond_by);

    console.log(
      `  ${created}/${count} ChallengeCreated in block #${createdHeight}; ` +
        `deadline relay block #${deadline}`,
    );
    if (created < count) {
      console.log(
        `  ⚠️  only ${created} landed. force_batch skips over-weight tail items, so the ` +
          `block ceiling is below ${count}.`,
      );
    }

    const nextIndexAtDeadline = await api.query.StorageProvider.NextChallengeIndex.getValue(
      deadline,
      READ_OPTS,
    );
    const createWeight = await readBlockWeight(api, createdBlockHash);

    // ── Wait out the deadline on the relay clock, then track the sweep ───────
    console.log(
      `Waiting for relay block #${deadline} (undefended, so all get slashed; ` +
        `sweep budget ${SWEEP_SLASH_BUDGET}/block → ~${Math.ceil(created / SWEEP_SLASH_BUDGET)} sweep blocks)...`,
    );
    await waitForRelayBlock(papi, api, deadline);
    const arrivalHeight = Number(await api.query.System.Number.getValue(READ_OPTS));

    // The sweep starts at the first parachain block whose predecessor's relay
    // parent exceeds the deadline — right around `arrivalHeight`. Scan a few
    // blocks back in case the wait observed the relay tick late, then follow
    // the chain until the slash count reconciles or the sweep cursor proves
    // the deadline key fully drained.
    const sweepBlocks: SweepBlock[] = [];
    let totalSlashed = 0;
    let scanHeight = Math.max(createdHeight + 1, arrivalHeight - 8);
    let lastScannedHash = "";
    while (totalSlashed < created) {
      await waitForBlock(papi, scanHeight - 1, { logEvery: 0, timeoutMs: 300_000 });
      const hash = await blockHashAt(papi, scanHeight);
      lastScannedHash = hash;
      const events = await api.query.System.Events.getValue({ at: hash });
      const slashes = countSlashEvents(events as unknown as any[]);
      if (slashes > 0) {
        const [weight, tPrev, tThis] = await Promise.all([
          readBlockWeight(api, hash),
          timestampAt(api, await blockHashAt(papi, scanHeight - 1)),
          timestampAt(api, hash),
        ]);
        sweepBlocks.push({ height: scanHeight, hash, slashes, blockTimeMs: tThis - tPrev, weight });
        totalSlashed += slashes;
        console.log(`  sweep block #${scanHeight}: ${slashes} slashes (${totalSlashed}/${created})`);
      } else if (scanHeight >= arrivalHeight) {
        // No slashes here although the deadline has passed: if the sweep
        // cursor has moved past the deadline key, the drain is over and the
        // remaining count will never reconcile — stop instead of spinning.
        const cursor = await api.query.StorageProvider.LastSweptChallengeBlock.getValue({
          at: hash,
        });
        if (cursor != null && Number(cursor) >= deadline) {
          console.log(
            `  ⚠️  sweep cursor at relay #${Number(cursor)} ≥ deadline with only ` +
              `${totalSlashed}/${created} slashes seen; stopping scan.`,
          );
          break;
        }
      }
      scanHeight++;
    }

    const measureHash = lastScannedHash || (await blockHashAt(papi, scanHeight - 1));
    const balancesAfter = await providerBalances(api, provider);

    // Residual: the sweep should have drained the prefix and dropped the
    // index allocator.
    const residual = await api.query.StorageProvider.Challenges.getEntries(deadline, {
      at: measureHash,
    });
    const nextIndexAfter = await api.query.StorageProvider.NextChallengeIndex.getValue(deadline, {
      at: measureHash,
    });

    // ── Report ───────────────────────────────────────────────────────────────
    const maxSweep = sweepBlocks.reduce(
      (m, b) => (b.weight.totalProof > m ? b.weight.totalProof : m),
      0n,
    );
    const result = {
      params: { count, challengeTimeout, maxPerDeadline, createdHeight, deadline },
      creation: {
        challengesCreated: created,
        nextChallengeIndexAtDeadline: Number(nextIndexAtDeadline),
        blockWeight: serializeWeight(createWeight),
        blockPovPct: Number(createWeight.totalProof) / MAX_POV,
      },
      sweep: {
        slashEvents: totalSlashed,
        sweepBlockCount: sweepBlocks.length,
        maxBlockPovPct: Number(maxSweep) / MAX_POV,
        blocks: sweepBlocks.map((b) => ({
          height: b.height,
          slashes: b.slashes,
          blockTimeMs: b.blockTimeMs,
          blockWeight: serializeWeight(b.weight),
          blockPovPct: Number(b.weight.totalProof) / MAX_POV,
          mandatoryPovPct: Number(b.weight.mandatory.proof) / MAX_POV,
        })),
      },
      economics: {
        providerStakeBefore: balancesBefore.stake.toString(),
        providerStakeAfter: balancesAfter.stake.toString(),
        providerReservedBefore: balancesBefore.reserved.toString(),
        providerReservedAfter: balancesAfter.reserved.toString(),
      },
      residual: {
        remainingChallengesAtDeadline: residual.length,
        nextChallengeIndexAfter: Number(nextIndexAfter),
      },
    };

    console.log("\n── Results ──────────────────────────────────────────────");
    console.log(
      `Created:        ${created}/${count} (NextChallengeIndex=${Number(nextIndexAtDeadline)})`,
    );
    console.log(
      `Creation block: PoV ${fmtMiB(createWeight.totalProof)} (${pct(result.creation.blockPovPct)} of PoV)`,
    );
    console.log(
      `Sweep:          ${totalSlashed} slashes over ${sweepBlocks.length} block(s) ` +
        `(budget ${SWEEP_SLASH_BUDGET}/block)`,
    );
    for (const b of sweepBlocks) {
      console.log(
        `  #${b.height}: ${b.slashes} slashes, block time ${b.blockTimeMs}ms, ` +
          `PoV ${fmtMiB(b.weight.totalProof)} (${pct(Number(b.weight.totalProof) / MAX_POV)}), ` +
          `mandatory ${fmtMiB(b.weight.mandatory.proof)}, ` +
          `ref ${(Number(b.weight.totalRefTime) / 1e9).toFixed(1)}ms of 2000ms`,
      );
    }
    console.log(
      `Provider stake: ${balancesBefore.stake} → ${balancesAfter.stake} (slashed on timeout)`,
    );
    console.log(
      `Residual:       ${residual.length} challenges left at deadline, ` +
        `NextChallengeIndex ${Number(nextIndexAfter)} (both should be 0)`,
    );

    const ok =
      created === count &&
      totalSlashed === created &&
      residual.length === 0 &&
      Number(nextIndexAfter) === 0;
    console.log(ok ? "\n✅ swept clean, counts reconcile" : "\n⚠️  see mismatches above");

    if (opts.jsonPath) {
      writeFileSync(opts.jsonPath, JSON.stringify(result, null, 2));
      console.log(`\nWrote ${opts.jsonPath}`);
    }

    try {
      await restore();
    } catch {}
  } finally {
    disconnect({ papi, api });
  }
}

function serializeWeight(w: BlockWeight) {
  return {
    normal: { refTime: w.normal.refTime.toString(), proof: w.normal.proof.toString() },
    operational: {
      refTime: w.operational.refTime.toString(),
      proof: w.operational.proof.toString(),
    },
    mandatory: { refTime: w.mandatory.refTime.toString(), proof: w.mandatory.proof.toString() },
    totalRefTime: w.totalRefTime.toString(),
    totalProof: w.totalProof.toString(),
  };
}

main().then(
  () => process.exit(0),
  (err) => {
    console.error(`❌ ${err?.message ?? err}`);
    process.exit(1);
  },
);
