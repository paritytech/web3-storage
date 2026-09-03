// SPDX-License-Identifier: Apache-2.0

/**
 * Domain-state Prometheus exporter for paritytech/web3-storage#267.
 *
 * The chain nodes already expose standard Substrate Prometheus metrics (block
 * time, weight, txpool). What no metric covers is the pallet's own domain
 * state: provider balances and stake, agreement counts, pending challenges,
 * checkpoint accruals, and per-class block weight (the on_finalize sweep spike
 * that Substrate's own Prometheus does not expose). This sidecar polls that
 * state per finalized block and exports it as Prometheus gauges, plus counters
 * accumulated from lifecycle events, so a stress run (see
 * `challenge-saturation.ts`) is observable on a dashboard. It is the first
 * consumer of the telemetry work in #214 and the scrape target the
 * `monitoring/` stack (a follow-up) will chart.
 *
 * Per-provider metrics are labelled; their cardinality is bounded by the small
 * `MaxPrimaryProviders` / `MaxMembers`. Aggregate counts (agreements,
 * challenges) are exported as totals, not per-entry, so a saturation run does
 * not blow up label cardinality.
 *
 * Values are float64 (Prometheus' only numeric type), so balances above 2^53
 * lose low-order precision. Fine for monitoring; do not treat as exact.
 *
 * Usage:
 *   pnpm --filter web3-storage-papi-demo run chain-exporter -- [options]
 *
 *   --chain-rpc <url>   default ws://127.0.0.1:2222
 *   --port <n>          metrics HTTP port, default 9700
 */

import { createServer } from "node:http";
import { parseArgs } from "node:util";

import {
  connect,
  syncSs58Prefix,
  waitForChainReady,
  type ChainConnection,
  type ParachainApi,
} from "@web3-storage/sdk";

function parseCli() {
  const { values } = parseArgs({
    options: {
      "chain-rpc": { type: "string", default: "ws://127.0.0.1:2222" },
      port: { type: "string", default: "9700" },
      help: { type: "boolean", default: false },
    },
  });
  if (values.help) {
    console.log(USAGE);
    process.exit(0);
  }
  const port = Number(values.port);
  if (!Number.isInteger(port) || port < 1 || port > 65535) {
    throw new Error(`--port must be 1..65535, got ${values.port}`);
  }
  return { chainRpc: values["chain-rpc"], port };
}

const USAGE = `chain-exporter: Prometheus exporter for web3-storage pallet domain state

  --chain-rpc <url>   default ws://127.0.0.1:2222
  --port <n>          metrics HTTP port, default 9700

Serves /metrics (Prometheus text) and /health.`;

// ── Metric registry ──────────────────────────────────────────────────────────

type Sample = { labels: Record<string, string>; value: number };

interface Metric {
  help: string;
  type: "gauge" | "counter";
  samples: Sample[];
}

const PREFIX = "web3storage_";

/** Minimal Prometheus registry: gauges are replaced each scrape, counters persist. */
class Registry {
  private metrics = new Map<string, Metric>();

  private ensure(name: string, type: Metric["type"], help: string): Metric {
    let m = this.metrics.get(name);
    if (!m) {
      m = { help, type, samples: [] };
      this.metrics.set(name, m);
    }
    return m;
  }

  /** Replace a gauge's samples for this scrape. */
  setGauge(name: string, value: number, labels: Record<string, string> = {}) {
    const m = this.ensure(name, "gauge", GAUGE_HELP[name] ?? name);
    m.samples.push({ labels, value });
  }

  clearGauge(name: string) {
    const m = this.metrics.get(name);
    if (m && m.type === "gauge") m.samples = [];
  }

  incCounter(name: string, by = 1) {
    const m = this.ensure(name, "counter", COUNTER_HELP[name] ?? name);
    if (m.samples.length === 0) m.samples.push({ labels: {}, value: 0 });
    m.samples[0].value += by;
  }

  render(): string {
    const lines: string[] = [];
    for (const [name, m] of this.metrics) {
      const full = PREFIX + name;
      lines.push(`# HELP ${full} ${m.help}`);
      lines.push(`# TYPE ${full} ${m.type}`);
      for (const s of m.samples) {
        const labels = Object.entries(s.labels)
          .map(([k, v]) => `${k}="${escapeLabel(v)}"`)
          .join(",");
        lines.push(`${full}${labels ? `{${labels}}` : ""} ${s.value}`);
      }
    }
    return lines.join("\n") + "\n";
  }
}

function escapeLabel(v: string): string {
  return v.replace(/\\/g, "\\\\").replace(/\n/g, "\\n").replace(/"/g, '\\"');
}

const GAUGE_HELP: Record<string, string> = {
  finalized_block: "Latest finalized block number scraped",
  providers_total: "Registered storage providers",
  buckets_total: "Layer-0 buckets",
  agreements_total: "Storage agreements (all)",
  agreements_active: "Storage agreements not past expires_at",
  agreements_expired: "Storage agreements past expires_at, not yet swept",
  challenges_pending_total: "Unresolved challenges across all deadlines",
  checkpoint_rewards_pending_total: "Sum of unclaimed checkpoint rewards (planck)",
  checkpoint_pool_total: "Sum of checkpoint pool balances (planck)",
  block_weight_ref_time: "Latest finalized block ref_time by dispatch class (picoseconds)",
  block_weight_proof_size: "Latest finalized block proof_size (PoV) by dispatch class (bytes)",
  provider_stake: "Provider bonded stake (planck)",
  provider_committed_bytes: "Provider committed bytes",
  provider_free_balance: "Provider free balance (planck)",
  provider_reserved_balance: "Provider reserved balance (planck)",
  provider_pending_challenges: "Unresolved challenges against a provider",
  provider_challenges_received: "Lifetime challenges received (provider stats)",
  provider_challenges_failed: "Lifetime challenges failed (provider stats)",
  provider_deregistering: "1 if the provider has announced deregistration",
  exporter_last_scrape_block: "Block number of the last successful scrape",
};

const COUNTER_HELP: Record<string, string> = {
  challenge_created_total: "ChallengeCreated events observed",
  challenge_defended_total: "ChallengeDefended events observed",
  challenge_slashed_total: "ChallengeSlashed events observed",
  provider_checkpoint_submitted_total: "ProviderCheckpointSubmitted events observed",
  checkpoint_miss_penalized_total: "CheckpointMissPenalized events observed",
  provider_registered_total: "ProviderRegistered events observed",
  exporter_scrape_errors_total: "Per-block scrape failures",
};

/** StorageProvider event type -> counter name. */
const EVENT_COUNTERS: Record<string, string> = {
  ChallengeCreated: "challenge_created_total",
  ChallengeDefended: "challenge_defended_total",
  ChallengeSlashed: "challenge_slashed_total",
  ProviderCheckpointSubmitted: "provider_checkpoint_submitted_total",
  CheckpointMissPenalized: "checkpoint_miss_penalized_total",
  ProviderRegistered: "provider_registered_total",
};

// ── Scraping ─────────────────────────────────────────────────────────────────

const num = (b: bigint | number): number => (typeof b === "bigint" ? Number(b) : b);

/** Snapshot the domain-state gauges at a finalized block hash. */
async function scrapeGauges(
  api: ParachainApi,
  reg: Registry,
  at: string,
  blockNumber: number,
): Promise<void> {
  const [providers, agreements, challenges, buckets, rewards, pools, blockWeight] =
    await Promise.all([
      api.query.StorageProvider.Providers.getEntries({ at }),
      api.query.StorageProvider.StorageAgreements.getEntries({ at }),
      api.query.StorageProvider.Challenges.getEntries({ at }),
      api.query.StorageProvider.Buckets.getEntries({ at }),
      api.query.StorageProvider.CheckpointRewards.getEntries({ at }),
      api.query.StorageProvider.CheckpointPool.getEntries({ at }),
      api.query.System.BlockWeight.getValue({ at }),
    ]);

  for (const name of PER_PROVIDER_GAUGES) reg.clearGauge(name);
  for (const { keyArgs, value } of providers) {
    const provider = keyArgs[0];
    const [account, pending] = await Promise.all([
      api.query.System.Account.getValue(provider, { at }),
      api.query.StorageProvider.PendingChallenges.getValue(provider, { at }),
    ]);
    const labels = { provider };
    reg.setGauge("provider_stake", num(value.stake), labels);
    reg.setGauge("provider_committed_bytes", num(value.committed_bytes), labels);
    reg.setGauge("provider_free_balance", num(account.data.free), labels);
    reg.setGauge("provider_reserved_balance", num(account.data.reserved), labels);
    reg.setGauge("provider_pending_challenges", num(pending), labels);
    reg.setGauge("provider_challenges_received", value.stats.challenges_received, labels);
    reg.setGauge("provider_challenges_failed", value.stats.challenges_failed, labels);
    reg.setGauge("provider_deregistering", value.deregister_at != null ? 1 : 0, labels);
  }

  let active = 0;
  for (const { value } of agreements) {
    if (value.expires_at > blockNumber) active++;
  }
  const rewardsTotal = rewards.reduce((sum, e) => sum + e.value, 0n);
  const poolTotal = pools.reduce((sum, e) => sum + e.value, 0n);

  reg.clearGauge("finalized_block");
  reg.setGauge("finalized_block", blockNumber);
  reg.clearGauge("providers_total");
  reg.setGauge("providers_total", providers.length);
  reg.clearGauge("buckets_total");
  reg.setGauge("buckets_total", buckets.length);
  reg.clearGauge("agreements_total");
  reg.setGauge("agreements_total", agreements.length);
  reg.clearGauge("agreements_active");
  reg.setGauge("agreements_active", active);
  reg.clearGauge("agreements_expired");
  reg.setGauge("agreements_expired", agreements.length - active);
  reg.clearGauge("challenges_pending_total");
  reg.setGauge("challenges_pending_total", challenges.length);
  reg.clearGauge("checkpoint_rewards_pending_total");
  reg.setGauge("checkpoint_rewards_pending_total", num(rewardsTotal));
  reg.clearGauge("checkpoint_pool_total");
  reg.setGauge("checkpoint_pool_total", num(poolTotal));

  // Per-class weight of the latest finalized block. Substrate's own Prometheus
  // does not expose this; it is the signal that spikes at a challenge-deadline
  // block (the on_finalize slash sweep is charged to the mandatory class). See
  // challenge-saturation.ts.
  reg.clearGauge("block_weight_ref_time");
  reg.clearGauge("block_weight_proof_size");
  for (const cls of ["normal", "operational", "mandatory"] as const) {
    reg.setGauge("block_weight_ref_time", num(blockWeight[cls].ref_time), { class: cls });
    reg.setGauge("block_weight_proof_size", num(blockWeight[cls].proof_size), { class: cls });
  }
}

const PER_PROVIDER_GAUGES = [
  "provider_stake",
  "provider_committed_bytes",
  "provider_free_balance",
  "provider_reserved_balance",
  "provider_pending_challenges",
  "provider_challenges_received",
  "provider_challenges_failed",
  "provider_deregistering",
];

/** Count lifecycle events in one block into the persistent counters. */
async function scrapeEvents(api: ParachainApi, reg: Registry, at: string): Promise<void> {
  const records = (await api.query.System.Events.getValue({ at })) as unknown as Array<{
    event: { type: string; value: { type: string } };
  }>;
  for (const r of records) {
    if (r.event?.type !== "StorageProvider") continue;
    const counter = EVENT_COUNTERS[r.event.value?.type];
    if (counter) reg.incCounter(counter);
  }
}

// ── Main ─────────────────────────────────────────────────────────────────────

async function main() {
  const opts = parseCli();
  const reg = new Registry();
  // Register counters at 0 up front so they exist before their first event;
  // `rate()` over a counter that only appears mid-run reads as a gap.
  for (const name of Object.values(EVENT_COUNTERS)) reg.incCounter(name, 0);
  reg.incCounter("exporter_scrape_errors_total", 0);

  const conn = connect(opts.chainRpc);
  const { papi, api } = conn;
  await waitForChainReady(api);
  await syncSs58Prefix(api);

  const server = createServer((req, res) => {
    if (req.url === "/metrics") {
      res.writeHead(200, { "content-type": "text/plain; version=0.0.4" });
      res.end(reg.render());
    } else if (req.url === "/health") {
      res.writeHead(200, { "content-type": "application/json" });
      res.end(JSON.stringify({ status: "ok" }));
    } else {
      res.writeHead(404).end("not found\n");
    }
  });
  server.listen(opts.port, () => {
    console.log(`chain-exporter on :${opts.port}/metrics, chain ${opts.chainRpc}`);
  });

  // Gauges track the latest finalized block; counters catch up over every block
  // since the last one so a finalization jump does not drop events.
  let lastEventBlock = -1;
  // Skip a block if the previous scrape is still running: overlapping runs would
  // corrupt the cleared-then-set gauge samples. The event catch-up loop reads
  // from lastEventBlock+1, so a skipped block's events are counted next time.
  let scraping = false;
  const sub = papi.finalizedBlock$.subscribe({
    next: async (block) => {
      if (scraping) return;
      scraping = true;
      try {
        await scrapeGauges(api, reg, block.hash, block.number);
        if (lastEventBlock < 0) lastEventBlock = block.number - 1;
        for (let n = lastEventBlock + 1; n <= block.number; n++) {
          const hash =
            n === block.number
              ? block.hash
              : await papi._request<string>("chain_getBlockHash", [n]);
          if (hash) await scrapeEvents(api, reg, hash);
        }
        lastEventBlock = block.number;
        reg.clearGauge("exporter_last_scrape_block");
        reg.setGauge("exporter_last_scrape_block", block.number);
      } catch (err) {
        reg.incCounter("exporter_scrape_errors_total");
        console.error(`scrape failed at #${block.number}: ${(err as Error)?.message ?? err}`);
      } finally {
        scraping = false;
      }
    },
    error: (err) => console.error(`finalizedBlock$ error: ${err}`),
  });

  const shutdown = () => {
    sub.unsubscribe();
    server.close();
    (conn as ChainConnection).papi.destroy();
    process.exit(0);
  };
  process.on("SIGINT", shutdown);
  process.on("SIGTERM", shutdown);
}

main().catch((err) => {
  console.error(`❌ ${err?.message ?? err}`);
  process.exit(1);
});
