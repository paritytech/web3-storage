// SPDX-License-Identifier: Apache-2.0

/**
 * GC soak test: drives the provider's garbage collector through repeated
 * prune cycles and bucket teardowns for a configurable period.
 *
 * Manual only. It is not in `examples/papi/e2e/`, so `just e2e` and CI never run it.
 *
 * Accounts: //Alice (provider), //Bob //Dave //Eve //Ferdie (one client lane each)
 *
 * Needs a chain (`just start-e2e-chain`) and a provider with an empty database,
 * started with a short GC rescan interval so retired buckets are erased in
 * seconds instead of the default 10 minutes:
 *
 *   GC_SCAN_INTERVAL=30 just PROVIDER_STORAGE_PATH=/tmp/soak-db start-provider
 *   just gc-soak --duration 30m --lanes 2 --provider-log /tmp/provider.log
 *
 * Usage: node --import tsx examples/papi/soak/gc-soak.ts [options]
 */

import assert from "node:assert";
import { execFileSync } from "node:child_process";
import { closeSync, openSync, readSync, statSync } from "node:fs";
import { parseArgs } from "node:util";
import {
  DEFAULT_CHUNK_SIZE,
  downloadChunk,
  endAgreement,
  ensureProviderRegistered,
  fetchBucketUsage,
  fetchCheckpointSignature,
  makeSigner,
  providerFetch,
  pruneAndCheckpoint,
  READ_OPTS,
  submitClientCheckpoint,
  uploadChunk,
  type ChainSigner,
  type ParachainApi,
} from "@web3-storage/sdk";
import { ensureSoleAcceptingProvider } from "../support.js";
import { getFree, negotiateAndEstablish, setupChain } from "../e2e/helpers.js";

/** Client accounts, one per lane. //Alice is the provider. */
const LANE_SEEDS = ["//Bob", "//Dave", "//Eve", "//Ferdie"];

/** Agreement length in anchor blocks, as in workflow 17. */
const AGREEMENT_DURATION_BLOCKS = 600;

const HELP = `GC soak test (manual, not run in CI)

Options:
  --chain <ws>              chain RPC (default ws://127.0.0.1:2222)
  --provider <url>          provider HTTP URL (default http://127.0.0.1:3333)
  --lanes <1-4>             parallel client accounts (default 2)
  --cycles-per-bucket <n>   prune cycles before a bucket is retired (default 5)
  --chunks-per-cycle <n>    chunks uploaded per cycle, at least 2 (default 3)
  --chunk-bytes <n>         bytes per chunk, from 64 up to the chunk size limit (default 100)
  --duration <30m|4h|90s>   stop starting epochs after this long (default 30m unless --epochs is set)
  --epochs <n>              stop after n epochs
  --gc-scan-interval <s>    the provider's GC_SCAN_INTERVAL in seconds (default 30)
  --erase-timeout <s>       max wait for one erasure (default 120)
  --provider-log <path>     provider log file; a "GC reconcile skipped" line fails the run
  --report <path>           append one JSON line per cycle and per barrier
  --help                    print this text
`;

interface SoakOptions {
  chainWs: string;
  providerUrl: string;
  lanes: number;
  cyclesPerBucket: number;
  chunksPerCycle: number;
  chunkBytes: number;
  durationMs: number | null;
  epochs: number | null;
  gcScanIntervalSec: number;
  eraseTimeoutMs: number;
  providerLog: string | null;
  reportPath: string | null;
}

function intOption(name: string, raw: string, min: number, max = Number.MAX_SAFE_INTEGER): number {
  const value = Number(raw);
  if (!Number.isInteger(value) || value < min || value > max) {
    const range = max === Number.MAX_SAFE_INTEGER ? `at least ${min}` : `from ${min} to ${max}`;
    throw new Error(`--${name} must be an integer ${range}, got "${raw}"`);
  }
  return value;
}

function durationOption(raw: string): number {
  const match = /^(\d+)([smh])$/.exec(raw);
  if (!match || Number(match[1]) === 0) {
    throw new Error(`--duration must look like 90s, 30m or 4h, got "${raw}"`);
  }
  const unitMs = { s: 1_000, m: 60_000, h: 3_600_000 }[match[2] as "s" | "m" | "h"];
  return Number(match[1]) * unitMs;
}

/** Returns null when `--help` was requested. */
function parseOptions(argv: string[]): SoakOptions | null {
  const { values } = parseArgs({
    args: argv,
    options: {
      chain: { type: "string", default: "ws://127.0.0.1:2222" },
      provider: { type: "string", default: "http://127.0.0.1:3333" },
      lanes: { type: "string", default: "2" },
      "cycles-per-bucket": { type: "string", default: "5" },
      "chunks-per-cycle": { type: "string", default: "3" },
      "chunk-bytes": { type: "string", default: "100" },
      duration: { type: "string" },
      epochs: { type: "string" },
      "gc-scan-interval": { type: "string", default: "30" },
      "erase-timeout": { type: "string", default: "120" },
      "provider-log": { type: "string" },
      report: { type: "string" },
      help: { type: "boolean", default: false },
    },
  });
  if (values.help) return null;

  const epochs = values.epochs === undefined ? null : intOption("epochs", values.epochs, 1);
  const durationMs =
    values.duration !== undefined
      ? durationOption(values.duration)
      : epochs === null
        ? durationOption("30m")
        : null;
  return {
    chainWs: values.chain!,
    providerUrl: values.provider!,
    lanes: intOption("lanes", values.lanes!, 1, LANE_SEEDS.length),
    cyclesPerBucket: intOption("cycles-per-bucket", values["cycles-per-bucket"]!, 1),
    chunksPerCycle: intOption("chunks-per-cycle", values["chunks-per-cycle"]!, 2),
    // Room for the unique tag in each payload, and for the one byte over quota
    // that the over-quota upload adds, below the chunk size limit.
    chunkBytes: intOption("chunk-bytes", values["chunk-bytes"]!, 64, DEFAULT_CHUNK_SIZE - 1),
    durationMs,
    epochs,
    gcScanIntervalSec: intOption("gc-scan-interval", values["gc-scan-interval"]!, 1),
    eraseTimeoutMs: intOption("erase-timeout", values["erase-timeout"]!, 1) * 1_000,
    providerLog: values["provider-log"] ?? null,
    reportPath: values.report ?? null,
  };
}

/**
 * Quota of each bucket: one surviving chunk from the previous cycle plus the
 * chunks of the current cycle, so every cycle after the first fills it exactly.
 */
function bucketMaxBytes(opts: SoakOptions): bigint {
  return BigInt((opts.chunksPerCycle + 1) * opts.chunkBytes);
}

interface ProviderStats {
  totalBuckets: bigint;
  totalNodes: bigint;
  totalBytes: bigint;
}

/** Provider-wide counters from `GET /stats`. */
async function fetchProviderStats(providerUrl: string): Promise<ProviderStats> {
  const stats = await providerFetch(providerUrl, "/stats");
  return {
    totalBuckets: BigInt(stats.total_buckets),
    totalNodes: BigInt(stats.total_nodes),
    totalBytes: BigInt(stats.total_bytes),
  };
}

/**
 * Fail before any workload when the provider already holds data (the
 * teardown check expects the provider to end each epoch empty) or when a lane
 * cannot pay for the run. Returns the cost of one bucket's agreement.
 */
async function preflight(
  api: ParachainApi,
  opts: SoakOptions,
  provider: ChainSigner,
  lanes: ChainSigner[],
): Promise<bigint> {
  const stats = await fetchProviderStats(opts.providerUrl);
  if (stats.totalBuckets !== 0n || stats.totalNodes !== 0n || stats.totalBytes !== 0n) {
    throw new Error(
      `provider ${opts.providerUrl} is not empty (${stats.totalBuckets} buckets, ` +
        `${stats.totalNodes} nodes, ${stats.totalBytes} bytes): start it with a fresh storage path`,
    );
  }

  const info = await api.query.StorageProvider.Providers.getValue(provider.address, READ_OPTS);
  const pricePerByte = info?.settings?.price_per_byte ?? 1n;
  const epochCost = pricePerByte * bucketMaxBytes(opts) * BigInt(AGREEMENT_DURATION_BLOCKS);
  const required = epochCost * BigInt(opts.epochs ?? 1);
  console.log(`  Agreement cost per bucket: ${epochCost}; each lane needs at least ${required}`);
  for (const [index, lane] of lanes.entries()) {
    const free = await getFree(api, lane);
    console.log(`  Lane ${index} (${LANE_SEEDS[index]}) free balance: ${free}`);
    if (free < required) {
      throw new Error(`lane ${index} (${LANE_SEEDS[index]}) has ${free}, needs at least ${required}`);
    }
  }
  return epochCost;
}

interface SoakContext {
  api: ParachainApi;
  opts: SoakOptions;
  provider: ChainSigner;
  /** Aborted when any lane fails, so the other lanes stop at their next step. */
  signal: AbortSignal;
  /** Distinguishes chunk payloads of different runs: nodes are shared across buckets. */
  runId: string;
  /** Cost of one bucket's agreement. */
  agreementCost: bigint;
  records: CycleRecord[];
  barriers: BarrierRecord[];
}

interface Lane {
  index: number;
  client: ChainSigner;
  bucketId: bigint | null;
  /** Bytes the provider must report as used on the current bucket. */
  liveBytes: bigint;
  /** The newest chunk of the previous cycle: still live, pruned by the next cycle. */
  survivor: string | null;
}

interface CycleRecord {
  kind: "cycle";
  lane: number;
  epoch: number;
  cycle: number;
  bucketId: string;
  uploadedBytes: number;
  eraseLatencyMs: number;
  usedBytesAfter: string;
}

interface BarrierRecord {
  kind: "barrier";
  epoch: number;
  drainMs: number;
  providerRssKb: number | null;
  gcSkippedLines: number;
  gcWarnLines: number;
}

/** An invariant break, with the epoch (and lane and cycle, when it has them) it happened in. */
class SoakFailure extends Error {
  constructor(
    message: string,
    readonly where: { lane?: number; epoch: number; cycle?: number },
  ) {
    super(message);
  }
}

/** Poll `fn` every `stepMs` until it returns a value; fail after `timeoutMs` or when the run aborts. */
async function pollUntil<T>(
  ctx: SoakContext,
  what: string,
  timeoutMs: number,
  stepMs: number,
  fn: () => Promise<T | null | false>,
): Promise<T> {
  const deadline = Date.now() + timeoutMs;
  for (;;) {
    ctx.signal.throwIfAborted();
    const value = await fn();
    if (value) return value;
    if (Date.now() > deadline) throw new Error(`timed out after ${timeoutMs} ms waiting for ${what}`);
    await new Promise((resolve) => setTimeout(resolve, stepMs));
  }
}

/** Unique, fixed-size chunk payload. */
function chunkPayload(ctx: SoakContext, lane: Lane, epoch: number, cycle: number, index: number, size = ctx.opts.chunkBytes) {
  return `${ctx.runId}-${lane.index}.${epoch}.${cycle}.${index}-`.padEnd(size, "x");
}

async function openBucket(ctx: SoakContext, lane: Lane): Promise<void> {
  const { bucketId } = await negotiateAndEstablish(
    ctx.api,
    ctx.opts.providerUrl,
    lane.client,
    ctx.provider,
    { maxBytes: bucketMaxBytes(ctx.opts), duration: AGREEMENT_DURATION_BLOCKS },
    true, // finalize: uploads follow immediately
  );
  lane.bucketId = bucketId;
  lane.liveBytes = 0n;
  lane.survivor = null;
}

/** End the lane's agreement. The GC then condemns and erases the bucket. */
async function retireBucket(ctx: SoakContext, lane: Lane): Promise<void> {
  const free = await getFree(ctx.api, lane.client);
  assert.ok(free >= ctx.agreementCost, `lane ${lane.index} cannot pay for the next bucket (${free} left)`);
  await endAgreement(ctx.api, lane.client, ctx.provider, lane.bucketId!, "Pay", undefined, {
    mode: "finalized",
  });
  lane.bucketId = null;
}

/**
 * One prune cycle on the lane's bucket: fill the quota, checkpoint, check the
 * usage and the over-quota refusal, prune every chunk but the newest, then
 * wait for the GC to erase the pruned bytes.
 */
async function runCycle(ctx: SoakContext, lane: Lane, epoch: number, cycle: number): Promise<CycleRecord> {
  const { api, opts, provider } = ctx;
  const bucketId = lane.bucketId!;
  const chunkBytes = BigInt(opts.chunkBytes);
  const maxBytes = bucketMaxBytes(opts);

  const hashes: string[] = [];
  let newestLeaf = 0;
  for (let i = 0; i < opts.chunksPerCycle; i++) {
    const { hash, commit } = await uploadChunk(
      opts.providerUrl,
      bucketId,
      chunkPayload(ctx, lane, epoch, cycle, i),
      lane.client,
    );
    hashes.push(hash);
    // `leaf_indices` counts from the commitment's `start_seq`.
    newestLeaf = Number(commit.start_seq) + commit.leaf_indices[0];
    lane.liveBytes += chunkBytes;
  }
  const checkpoint = await fetchCheckpointSignature(opts.providerUrl, bucketId, lane.client);
  await submitClientCheckpoint(api, lane.client, bucketId, [{ provider, commit: checkpoint }]);

  // The first checkpoint of a bucket arms its quota on the provider.
  const usage = await pollUntil(ctx, "quota sync", 90_000, 1_000, async () => {
    const u = await fetchBucketUsage(opts.providerUrl, bucketId, lane.client);
    return u.quotaSynced ? u : null;
  });
  assert.strictEqual(usage.maxBytes, maxBytes, "quota should equal the agreement max_bytes");
  assert.strictEqual(usage.usedBytes, lane.liveBytes, "used bytes should equal the stored chunks");

  await assert.rejects(
    uploadChunk(
      opts.providerUrl,
      bucketId,
      chunkPayload(ctx, lane, epoch, cycle, opts.chunksPerCycle, Number(maxBytes - lane.liveBytes) + 1),
      lane.client,
    ),
    /quota_exceeded/,
    "a chunk larger than the remaining capacity must be refused",
  );

  const pruned = lane.survivor ? [lane.survivor, ...hashes.slice(0, -1)] : hashes.slice(0, -1);
  const survivor = hashes[hashes.length - 1];
  await pruneAndCheckpoint(api, opts.providerUrl, lane.client, provider, bucketId, BigInt(newestLeaf));

  const erasureStart = Date.now();
  const after = await pollUntil(ctx, "erasure and quota reclaim", opts.eraseTimeoutMs, 1_000, async () => {
    const u = await fetchBucketUsage(opts.providerUrl, bucketId, lane.client);
    return u.usedBytes === chunkBytes ? u : null;
  });
  const eraseLatencyMs = Date.now() - erasureStart;
  lane.liveBytes = chunkBytes;
  lane.survivor = survivor;

  for (const hash of pruned) {
    await assert.rejects(
      downloadChunk(opts.providerUrl, bucketId, hash),
      /404|not_found/,
      "an erased chunk must not be downloadable",
    );
  }
  await downloadChunk(opts.providerUrl, bucketId, survivor);

  const record: CycleRecord = {
    kind: "cycle",
    lane: lane.index,
    epoch,
    cycle,
    bucketId: String(bucketId),
    uploadedBytes: opts.chunksPerCycle * opts.chunkBytes,
    eraseLatencyMs,
    usedBytesAfter: String(after.usedBytes),
  };
  console.log(
    `  lane ${lane.index} epoch ${epoch} cycle ${cycle} bucket ${bucketId}: erased in ${eraseLatencyMs} ms, used ${after.usedBytes}/${maxBytes}`,
  );
  return record;
}

/** One lane of an epoch: open a bucket, run its cycles and end its agreement. */
async function runLane(ctx: SoakContext, lane: Lane, epoch: number, abort: AbortController): Promise<void> {
  let cycle: number | undefined;
  try {
    await openBucket(ctx, lane);
    for (cycle = 0; cycle < ctx.opts.cyclesPerBucket; cycle++) {
      ctx.signal.throwIfAborted();
      ctx.records.push(await runCycle(ctx, lane, epoch, cycle));
    }
    cycle = undefined;
    await retireBucket(ctx, lane);
  } catch (err) {
    if (ctx.signal.aborted) return; // another lane failed first
    abort.abort();
    const message = err instanceof Error ? err.message : String(err);
    throw new SoakFailure(message, { lane: lane.index, epoch, cycle });
  }
}

/** Listening process of a local provider URL, or null when it cannot be found. */
function providerPid(providerUrl: string): number | null {
  const { hostname, port } = new URL(providerUrl);
  if (!["127.0.0.1", "localhost", "[::1]"].includes(hostname) || !port) return null;
  try {
    const out = execFileSync("lsof", ["-ti", `tcp:${port}`, "-sTCP:LISTEN"], { encoding: "utf8" });
    const pid = Number(out.split("\n")[0]);
    return Number.isInteger(pid) && pid > 0 ? pid : null;
  } catch {
    return null;
  }
}

function rssKb(pid: number): number | null {
  try {
    const kb = Number(execFileSync("ps", ["-o", "rss=", "-p", String(pid)], { encoding: "utf8" }));
    return Number.isFinite(kb) && kb > 0 ? kb : null;
  } catch {
    return null;
  }
}

/** Counts the garbage collector's warnings appended to the provider log since the last read. */
class LogTail {
  private offset: number;

  constructor(private readonly path: string) {
    this.offset = statSync(path).size;
  }

  readNew(): { gcSkipped: number; gcWarn: number } {
    const size = statSync(this.path).size;
    if (size < this.offset) this.offset = 0; // the log was truncated
    const buffer = Buffer.alloc(size - this.offset);
    const fd = openSync(this.path, "r");
    try {
      readSync(fd, buffer, 0, buffer.length, this.offset);
    } finally {
      closeSync(fd);
    }
    this.offset = size;
    let gcSkipped = 0;
    let gcWarn = 0;
    // Strip the colour codes the provider writes.
    for (const line of buffer.toString("utf8").replace(/\x1b\[[0-9;]*m/g, "").split("\n")) {
      if (!line.includes("provider_garbage_collector")) continue;
      if (line.includes("GC reconcile skipped")) gcSkipped++;
      if (/\b(WARN|ERROR)\b/.test(line)) gcWarn++;
    }
    return { gcSkipped, gcWarn };
  }
}

/**
 * After an epoch's agreements ended: wait for the provider to hold no bucket,
 * node or byte, which means the GC condemned and erased every retired bucket.
 */
async function drainBarrier(ctx: SoakContext, epoch: number, log: LogTail | null): Promise<BarrierRecord> {
  const timeoutMs = (2 * ctx.opts.gcScanIntervalSec + 60) * 1_000;
  const seen: { stats: ProviderStats | null } = { stats: null };
  const started = Date.now();
  try {
    await pollUntil(ctx, "the provider to drain", timeoutMs, 2_000, async () => {
      const stats = (seen.stats = await fetchProviderStats(ctx.opts.providerUrl));
      return stats.totalBuckets === 0n && stats.totalNodes === 0n && stats.totalBytes === 0n;
    });
  } catch (err) {
    const left = seen.stats;
    const remaining = left
      ? `${left.totalBuckets} buckets, ${left.totalNodes} nodes, ${left.totalBytes} bytes remain`
      : "no /stats response";
    throw new SoakFailure(`${err instanceof Error ? err.message : err} (${remaining})`, { epoch });
  }
  const drainMs = Date.now() - started;

  const pid = providerPid(ctx.opts.providerUrl);
  const { gcSkipped, gcWarn } = log ? log.readNew() : { gcSkipped: 0, gcWarn: 0 };
  const record: BarrierRecord = {
    kind: "barrier",
    epoch,
    drainMs,
    providerRssKb: pid === null ? null : rssKb(pid),
    gcSkippedLines: gcSkipped,
    gcWarnLines: gcWarn,
  };
  console.log(
    `  epoch ${epoch} drained in ${drainMs} ms; provider RSS ${record.providerRssKb ?? "unavailable"} KB; ` +
      `GC warnings ${gcWarn}, skipped reconciles ${gcSkipped}`,
  );
  if (gcSkipped > 0) {
    throw new SoakFailure(`the provider log has ${gcSkipped} "GC reconcile skipped" line(s)`, { epoch });
  }
  return record;
}

/** All lanes in parallel. Rejects with the first failure. */
async function runEpoch(ctx: SoakContext, lanes: Lane[], epoch: number, abort: AbortController): Promise<void> {
  const results = await Promise.allSettled(lanes.map((lane) => runLane(ctx, lane, epoch, abort)));
  for (const result of results) {
    if (result.status === "rejected") throw result.reason;
  }
}

async function main() {
  const opts = parseOptions(process.argv.slice(2));
  if (!opts) {
    console.log(HELP);
    return;
  }

  const provider = makeSigner("//Alice");
  const clients = LANE_SEEDS.slice(0, opts.lanes).map((seed) => makeSigner(seed));

  const { papi, api } = await setupChain(opts.chainWs);
  await ensureProviderRegistered(api, provider, opts.providerUrl);
  const restore = await ensureSoleAcceptingProvider(api, provider);
  try {
    const agreementCost = await preflight(api, opts, provider, clients);
    const log = opts.providerLog ? new LogTail(opts.providerLog) : null;

    const abort = new AbortController();
    const ctx: SoakContext = {
      api,
      opts,
      provider,
      signal: abort.signal,
      runId: Date.now().toString(36),
      agreementCost,
      records: [],
      barriers: [],
    };
    const lanes: Lane[] = clients.map((client, index) => ({
      index,
      client,
      bucketId: null,
      liveBytes: 0n,
      survivor: null,
    }));
    const startedAt = Date.now();
    for (let epoch = 0; ; epoch++) {
      if (opts.epochs !== null && epoch >= opts.epochs) break;
      if (opts.durationMs !== null && epoch > 0 && Date.now() - startedAt >= opts.durationMs) break;
      await runEpoch(ctx, lanes, epoch, abort);
      ctx.barriers.push(await drainBarrier(ctx, epoch, log));
    }
  } catch (err) {
    if (err instanceof SoakFailure) {
      const { lane, epoch, cycle } = err.where;
      console.error(
        `FAILED epoch ${epoch}` +
          (lane === undefined ? "" : ` lane ${lane}`) +
          (cycle === undefined ? "" : ` cycle ${cycle}`),
      );
    }
    throw err;
  } finally {
    try {
      await restore();
    } catch {}
    papi.destroy();
  }
}

main()
  .catch((err) => {
    console.error(`error: ${err instanceof Error ? err.message : err}`);
    process.exitCode = 1;
  })
  .finally(() => {
    process.exit(process.exitCode || 0);
  });
