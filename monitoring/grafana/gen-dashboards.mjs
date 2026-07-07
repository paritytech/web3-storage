import { writeFileSync, mkdirSync } from "node:fs";

const OUT = process.argv[2];
mkdirSync(OUT, { recursive: true });

const DS = { type: "prometheus", uid: "prometheus" };
let idSeq = 1;

// target: {expr, legend?}
function panel(title, targets, opts = {}) {
  const {
    type = "timeseries",
    unit = "short",
    w = 12,
    h = 8,
    x = 0,
    y = 0,
    desc = "",
    max = null,
    stack = false,
  } = opts;
  const fieldConfig = {
    defaults: {
      unit,
      custom:
        type === "timeseries"
          ? { drawStyle: "line", fillOpacity: stack ? 30 : 10, stacking: { mode: stack ? "normal" : "none" } }
          : {},
      ...(max != null ? { max } : {}),
    },
    overrides: [],
  };
  return {
    id: idSeq++,
    title,
    description: desc,
    type,
    datasource: DS,
    gridPos: { h, w, x, y },
    fieldConfig,
    options: type === "stat" ? { reduceOptions: { calcs: ["lastNotNull"] }, textMode: "value_and_name" } : {},
    targets: targets.map((t, i) => ({
      refId: String.fromCharCode(65 + i),
      datasource: DS,
      expr: t.expr,
      legendFormat: t.legend ?? "",
    })),
  };
}

function dashboard(uid, title, panels) {
  return {
    uid,
    title,
    tags: ["web3-storage", "stress"],
    timezone: "browser",
    schemaVersion: 39,
    version: 1,
    refresh: "10s",
    time: { from: "now-30m", to: "now" },
    templating: { list: [] },
    panels,
  };
}

// ── Chain health ─────────────────────────────────────────────────────────────
const chainHealth = dashboard("w3s-chain-health", "Web3 Storage — Chain Health", [
  panel(
    "Block height (best vs finalized)",
    [
      { expr: 'substrate_block_height{job="parachain-node",status="best"}', legend: "best" },
      { expr: 'substrate_block_height{job="parachain-node",status="finalized"}', legend: "finalized" },
    ],
    { x: 0, y: 0, desc: "Parachain best and finalized height." },
  ),
  panel(
    "Finality lag (blocks)",
    [
      {
        expr: 'substrate_block_height{job="parachain-node",status="best"} - substrate_block_height{job="parachain-node",status="finalized"}',
        legend: "lag",
      },
    ],
    { x: 12, y: 0, desc: "best - finalized. A growing lag means finalization is stalling." },
  ),
  panel(
    "Block production rate (blocks/s)",
    [{ expr: 'rate(substrate_block_height{job="parachain-node",status="best"}[1m])', legend: "best/s" }],
    { x: 0, y: 8, desc: "~0.167/s at a healthy 6s block time; a dip means slow or stalled production." },
  ),
  panel(
    "Txpool ready transactions",
    [{ expr: 'substrate_ready_transactions_number{job="parachain-node"}', legend: "ready" }],
    { x: 12, y: 8 },
  ),
  panel(
    "Block PoV (proof_size) by class",
    [{ expr: 'web3storage_block_weight_proof_size', legend: "{{class}}" }],
    {
      x: 0,
      y: 16,
      unit: "bytes",
      max: 5 * 1024 * 1024,
      desc: "On-chain block weight from the exporter. The mandatory class spikes at a challenge-deadline block (the on_finalize slash sweep). Max line = 5 MiB PoV budget.",
    },
  ),
  panel(
    "Block ref_time by class (ms)",
    [{ expr: 'web3storage_block_weight_ref_time / 1e9', legend: "{{class}}" }],
    { x: 12, y: 16, unit: "ms", max: 2000, desc: "2000 ms is the block ref_time budget." },
  ),
  panel(
    "Peers",
    [{ expr: 'substrate_sub_libp2p_peers_count{job="parachain-node"}', legend: "peers" }],
    { x: 0, y: 24, h: 6 },
  ),
]);

// ── Economics ────────────────────────────────────────────────────────────────
const economics = dashboard("w3s-economics", "Web3 Storage — Economics", [
  panel("Provider stake", [{ expr: "web3storage_provider_stake", legend: "{{provider}}" }], {
    x: 0,
    y: 0,
    desc: "Bonded stake per provider. Drops to 0 when fully slashed.",
  }),
  panel(
    "Provider free balance",
    [{ expr: "web3storage_provider_free_balance", legend: "{{provider}}" }],
    { x: 12, y: 0 },
  ),
  panel(
    "Provider reserved balance",
    [{ expr: "web3storage_provider_reserved_balance", legend: "{{provider}}" }],
    { x: 0, y: 8 },
  ),
  panel(
    "Challenge slashes (cumulative)",
    [{ expr: "web3storage_challenge_slashed_total", legend: "slashed" }],
    { x: 12, y: 8, desc: "Total challenges resolved by slash (timeout or invalid response)." },
  ),
  panel(
    "Checkpoint rewards pending",
    [{ expr: "web3storage_checkpoint_rewards_pending_total", legend: "rewards" }],
    { x: 0, y: 16 },
  ),
  panel(
    "Checkpoint pool balance",
    [{ expr: "web3storage_checkpoint_pool_total", legend: "pool" }],
    { x: 12, y: 16 },
  ),
]);

// ── Protocol activity ────────────────────────────────────────────────────────
const activity = dashboard("w3s-protocol-activity", "Web3 Storage — Protocol Activity", [
  panel(
    "Agreements (active vs expired)",
    [
      { expr: "web3storage_agreements_active", legend: "active" },
      { expr: "web3storage_agreements_expired", legend: "expired (unswept)" },
    ],
    { x: 0, y: 0, stack: true, desc: "Expired-but-unswept is the lazy-cleanup residual (#177)." },
  ),
  panel(
    "Pending challenges",
    [{ expr: "web3storage_challenges_pending_total", legend: "pending" }],
    { x: 12, y: 0, desc: "Unresolved challenges across all deadlines. Spikes during saturation." },
  ),
  panel(
    "Challenge lifecycle (cumulative)",
    [
      { expr: "web3storage_challenge_created_total", legend: "created" },
      { expr: "web3storage_challenge_defended_total", legend: "defended" },
      { expr: "web3storage_challenge_slashed_total", legend: "slashed" },
    ],
    { x: 0, y: 8 },
  ),
  panel(
    "Challenge creation rate (/s)",
    [{ expr: "rate(web3storage_challenge_created_total[1m])", legend: "created/s" }],
    { x: 12, y: 8 },
  ),
  panel(
    "Checkpoints (cumulative)",
    [
      { expr: "web3storage_provider_checkpoint_submitted_total", legend: "submitted" },
      { expr: "web3storage_checkpoint_miss_penalized_total", legend: "miss-penalized" },
    ],
    { x: 0, y: 16 },
  ),
  panel(
    "Providers / buckets",
    [
      { expr: "web3storage_providers_total", legend: "providers" },
      { expr: "web3storage_buckets_total", legend: "buckets" },
    ],
    { x: 12, y: 16 },
  ),
]);

for (const d of [chainHealth, economics, activity]) {
  writeFileSync(`${OUT}/${d.uid}.json`, JSON.stringify(d, null, 2) + "\n");
  console.log(`wrote ${d.uid}.json (${d.panels.length} panels)`);
}
