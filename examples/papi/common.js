// SPDX-License-Identifier: Apache-2.0

/**
 * Shared helpers for the PAPI examples in this directory.
 *
 * Importable from any example via `import { ... } from "./common.js"`.
 */

import { createClient } from "polkadot-api";
import { getWsProvider } from "polkadot-api/ws";
import { getPolkadotSigner } from "polkadot-api/signer";
import { sr25519CreateDerive } from "@polkadot-labs/hdkd";
import {
  DEV_PHRASE,
  entropyToMiniSecret,
  mnemonicToEntropy,
  ss58Address,
  ss58Decode,
} from "@polkadot-labs/hdkd-helpers";
import { parachain } from "@polkadot-api/descriptors";

const devMiniSecret = entropyToMiniSecret(mnemonicToEntropy(DEV_PHRASE));
const deriveSr25519 = sr25519CreateDerive(devMiniSecret);

/**
 * Default CLI arguments shared by examples that talk to both the chain and
 * the provider node. Layout: `node script.js [chain_ws] [provider_url]
 * [provider_seed] [client_seed]`.
 *
 * Centralised here so every example reads the same defaults — change them
 * once and all the demos pick them up.
 */
export function parseProviderClientArgs(argv = process.argv) {
  return {
    chainWs: argv[2] || "ws://127.0.0.1:2222",
    providerUrl: argv[3] || "http://127.0.0.1:3333",
    providerSeed: argv[4] || "//Alice",
    clientSeed: argv[5] || "//Bob",
  };
}

export function makeSigner(seed) {
  const keyPair = deriveSr25519(seed);
  return {
    signer: getPolkadotSigner(keyPair.publicKey, "Sr25519", keyPair.sign),
    address: ss58Address(keyPair.publicKey),
    publicKey: keyPair.publicKey,
    seed,
  };
}

export function toHex(bytes) {
  const arr = bytes instanceof Uint8Array ? bytes : new Uint8Array(bytes);
  return "0x" + Array.from(arr, (b) => b.toString(16).padStart(2, "0")).join("");
}

export function hexToBytes(hex) {
  const h = hex.startsWith("0x") ? hex.slice(2) : hex;
  const bytes = new Uint8Array(h.length / 2);
  for (let i = 0; i < bytes.length; i++) {
    bytes[i] = parseInt(h.substr(i * 2, 2), 16);
  }
  return bytes;
}

export async function connect(chainWs) {
  const papi = createClient(getWsProvider(chainWs));
  const api = papi.getTypedApi(parachain);
  return { papi, api };
}

/**
 * Verify the chain's runtime is reachable by reading a constant. Catches the
 * "RPC handshake completed but the runtime metadata isn't loaded yet" window
 * that occurs right after zombienet startup, before any submit would fail
 * cryptically downstream.
 */
export async function waitForChainReady(
  api,
  { maxRetries = 10, retryDelayMs = 2_000 } = {}
) {
  for (let attempt = 1; attempt <= maxRetries; attempt++) {
    try {
      const version = api.constants.System.Version;
      // Reading the property may throw if metadata isn't ready yet.
      // Touching `.spec_version` forces resolution.
      const spec = await version();
      console.log(
        `✅ Chain ready: ${spec.spec_name} v${spec.spec_version}`
      );
      return true;
    } catch (error) {
      if (attempt === maxRetries) {
        throw new Error(
          `Chain not ready after ${maxRetries} attempts: ${error?.message ?? error}`
        );
      }
      console.log(
        `⏳ Chain not ready (attempt ${attempt}/${maxRetries}): ${error?.message ?? error}`
      );
      await new Promise((r) => setTimeout(r, retryDelayMs));
    }
  }
  return false;
}

/**
 * Poll `System.Number` until it crosses zero. Distinct from
 * `waitForNextBlock`: this guards the case where the chain isn't producing
 * blocks at all yet (zombienet warmup), where a subscription-based wait would
 * block forever with no events to react to.
 */
export async function waitForBlockProduction(api, { timeoutSec = 300 } = {}) {
  const pollIntervalMs = 2_000;
  const deadline = Date.now() + timeoutSec * 1_000;
  while (Date.now() < deadline) {
    try {
      const n = await api.query.System.Number.getValue(READ_OPTS);
      const blockNumber = typeof n === "bigint" ? Number(n) : Number(n);
      if (blockNumber > 0) {
        console.log(`✅ Chain producing blocks (head=#${blockNumber})`);
        return blockNumber;
      }
      console.log(`⏳ Block number is ${blockNumber}, waiting...`);
    } catch (error) {
      console.log(
        `⏳ Cannot query block number yet: ${error?.message ?? error}`
      );
    }
    await new Promise((r) => setTimeout(r, pollIntervalMs));
  }
  throw new Error(`Chain did not produce any blocks within ${timeoutSec}s`);
}

/**
 * Sequence multiple transactions from the same signer without racing the
 * RPC's nonce cache. Construct once after reading the on-chain nonce, then
 * pass `{ nonce: nm.next() }` as `txOpts` when submitting back-to-back txs.
 *
 * Ported from polkadot-bulletin-chain/examples/common.js NonceManager.
 */
export class NonceManager {
  constructor(initialNonce) {
    this.nonce = BigInt(initialNonce);
  }
  /** Return the current nonce, then advance. */
  next() {
    const current = this.nonce;
    this.nonce += 1n;
    return current;
  }
  /** Read the on-chain nonce for `address` and build a manager from it. */
  static async forAccount(api, address) {
    // Best block: a finalized read lags and returns a stale (too-low) nonce.
    const account = await api.query.System.Account.getValue(address, READ_OPTS);
    return new NonceManager(account.nonce);
  }
}

/**
 * Wait until the chain advances to a new best block.
 *
 * Run this once at the top of every example before submitting any extrinsic.
 * The provider node runs background coordinators (agreement auto-accept,
 * checkpoint) signed by the same dev key the demo uses, so two transactions
 * can land in the pool with the same `(account, nonce)` and one is dropped as
 * "Usurped". Aligning the demo's first submit to a fresh block boundary
 * shrinks the collision window enough that the demos run cleanly.
 */
export async function waitForNextBlock(papi) {
  return new Promise((resolve) => {
    let initial = null;
    let sub;
    sub = papi.bestBlocks$.subscribe((blocks) => {
      const block = blocks[blocks.length - 1];
      if (initial === null) {
        initial = block.number;
        return;
      }
      if (block.number > initial) {
        sub?.unsubscribe();
        resolve();
      }
    });
  });
}

/**
 * Wait until the chain's best head is strictly greater than `target`.
 *
 * Examples that need to land an extrinsic at a specific block window (e.g.
 * `provider_checkpoint`, `report_missed_checkpoint`) use this to time their
 * submission so the runtime sees the block range they computed against.
 */
export async function waitForBlock(papi, target, { logEvery = 5 } = {}) {
  await new Promise((resolve) => {
    let sub;
    sub = papi.bestBlocks$.subscribe((blocks) => {
      const block = blocks[blocks.length - 1];
      if (logEvery > 0 && block.number % logEvery === 0) {
        console.log("    head=#%d (target > %d)", block.number, target);
      }
      if (block.number > target) {
        sub?.unsubscribe();
        resolve();
      }
    });
  });
}

/**
 * Default per-transaction timeout. 180s ≈ 30 blocks at 6s — enough headroom
 * for a tx to be included even under CI load, but bounded so a stuck mempool
 * surfaces as a clear error instead of an indefinite hang that later looks
 * like a missing event downstream.
 */
export const DEFAULT_TX_TIMEOUT_MS = 180_000;

/**
 * Options for storage reads (`getValue` / `getEntries`). `submitTx` returns at
 * in-block inclusion, so reads must target the best block to see a just-written
 * value (the default finalized head lags by ~6 blocks). Single switch for the
 * suite: flip to "finalized" here to trade read-your-writes for reorg-safety.
 */
export const READ_OPTS = { at: "best" };

/**
 * Transaction "doneness" thresholds for `waitForTransaction`. Pick the
 * loosest one your caller can tolerate — earlier modes return faster but
 * before forks could roll the tx back.
 */
export const TX_MODE_IN_POOL = "in-tx-pool"; // broadcasted, not yet in a block
export const TX_MODE_IN_BLOCK = "in-block"; // included in best block (default)
export const TX_MODE_FINALIZED_BLOCK = "finalized-block"; // included in finalized block

const TX_MODE_CONFIG = {
  [TX_MODE_IN_POOL]: {
    match: (ev) => ev.type === "broadcasted",
    log: (label, ev) => `📦 ${label} broadcasted txHash=${ev.txHash}`,
  },
  [TX_MODE_IN_BLOCK]: {
    match: (ev) => ev.type === "txBestBlocksState" && ev.found,
    log: (label, ev) => `📦 ${label} included in block ${ev.block.hash}`,
  },
  [TX_MODE_FINALIZED_BLOCK]: {
    match: (ev) => ev.type === "finalized",
    log: (label, ev) => `📦 ${label} finalized in block ${ev.block.hash}`,
  },
};

function formatDispatchError(err) {
  if (!err || typeof err !== "object") return String(err);
  let s = err.type ?? "DispatchError";
  if (err.value?.type) s += `::${err.value.type}`;
  if (err.value?.value?.type) s += `::${err.value.value.type}`;
  return s;
}

/**
 * Subscribe to a transaction observable and resolve when it reaches `txMode`,
 * reject on dispatch error or after `timeoutMs`. Pattern ported from
 * polkadot-bulletin-chain/examples/api.js — gives explicit "doneness" control
 * and a hard timeout so a stuck mempool can't hang the example forever.
 *
 * Pass `signer = null` for unsigned txs (uses `tx.getBareTx()` +
 * `client.submitAndWatch`). For signed txs, pass the PAPI client as `client`
 * only if you need it; it's unused otherwise.
 */
export async function waitForTransaction(
  tx,
  signer,
  label,
  txMode = TX_MODE_IN_BLOCK,
  timeoutMs = DEFAULT_TX_TIMEOUT_MS,
  client = null,
  txOpts = {}
) {
  const config = TX_MODE_CONFIG[txMode];
  if (!config) throw new Error(`Unhandled txMode: ${txMode}`);

  let observable;
  if (signer === null) {
    if (Object.keys(txOpts).length > 0) {
      throw new Error(
        `${label}: txOpts not supported for unsigned transactions ` +
          `(getBareTx accepts no options). See polkadot-api/polkadot-api#760.`
      );
    }
    if (!client) {
      throw new Error(
        `${label}: unsigned submission requires the PAPI client (3rd-to-last arg).`
      );
    }
    const bareTx = await tx.getBareTx();
    observable = client.submitAndWatch(bareTx);
  } else {
    observable = tx.signSubmitAndWatch(signer, txOpts);
  }

  return new Promise((resolve, reject) => {
    let resolved = false;
    let sub;
    const cleanup = () => {
      resolved = true;
      clearTimeout(timer);
      if (sub) sub.unsubscribe();
    };
    const timer = setTimeout(() => {
      if (!resolved) {
        cleanup();
        reject(
          new Error(
            `${label}: timed out after ${timeoutMs}ms waiting for ${txMode}`
          )
        );
      }
    }, timeoutMs);

    sub = observable.subscribe({
      next: (ev) => {
        if (resolved) return;
        // PAPI surfaces dispatch errors on the in-block / finalized events as
        // `ev.ok === false`. Catch them here so callers don't have to.
        if (
          (ev.type === "txBestBlocksState" && ev.found && ev.ok === false) ||
          (ev.type === "finalized" && ev.ok === false)
        ) {
          cleanup();
          reject(
            new Error(
              `${label} dispatch failed: ${formatDispatchError(ev.dispatchError)}`
            )
          );
          return;
        }
        if (config.match(ev)) {
          console.log(config.log(label, ev));
          cleanup();
          resolve(ev);
        }
      },
      error: (err) => {
        if (resolved) return;
        cleanup();
        reject(new Error(`${label} stream error: ${err}`));
      },
    });
  });
}

/**
 * Sign + submit a tx, assert it dispatched OK, return the in-block event
 * (`{ ok, events, ... }`). Resolves at inclusion, not finalization — ~6x faster
 * per tx, and the demos read what they need from the events. PAPI doesn't throw
 * on a dispatch error (only on an invalid tx), so the `ok === false` check in
 * `waitForTransaction` is what surfaces failures here.
 *
 * Best blocks can be reorged, so for a tx whose effect a LATER tx references by
 * id (e.g. create a challenge, then respond to it) use `submitTxFinalized`.
 *
 * Retries up to `STALE_RETRIES` times on `Invalid::Stale` (nonce too low).
 * This happens when back-to-back CI tests reuse the same signing account and
 * the PAPI client's nonce view lags behind the chain's best block.
 */
const STALE_RETRIES = 3;
const STALE_RETRY_DELAY_MS = 6_500; // ~1 block time

export async function submitTx(
  tx,
  signer,
  label,
  timeoutMs = DEFAULT_TX_TIMEOUT_MS
) {
  for (let attempt = 1; attempt <= STALE_RETRIES; attempt++) {
    try {
      return await waitForTransaction(tx, signer, label, TX_MODE_IN_BLOCK, timeoutMs);
    } catch (err) {
      const isStale = /Invalid.*Stale|Stale.*nonce/i.test(err.message);
      if (!isStale || attempt === STALE_RETRIES) throw err;
      console.warn(
        `  ⚠ ${label}: stale nonce (attempt ${attempt}/${STALE_RETRIES}), ` +
        `waiting ${STALE_RETRY_DELAY_MS}ms for next block…`
      );
      await new Promise((r) => setTimeout(r, STALE_RETRY_DELAY_MS));
    }
  }
}

/**
 * Like `submitTx` but waits for FINALIZATION (~6 blocks slower). Use only when
 * a later tx references this one's effect by id: a challenge id embeds its
 * creation block, so an in-block creation that gets reorged makes the response
 * fail with `ChallengeNotFound`. Finalizing pins the id.
 */
export async function submitTxFinalized(
  tx,
  signer,
  label,
  timeoutMs = DEFAULT_TX_TIMEOUT_MS
) {
  return waitForTransaction(
    tx,
    signer,
    label,
    TX_MODE_FINALIZED_BLOCK,
    timeoutMs
  );
}

export async function providerFetch(providerUrl, path, opts = {}) {
  const url = new URL(path, providerUrl);
  if (opts.params) {
    for (const [k, v] of Object.entries(opts.params))
      url.searchParams.set(k, v);
  }
  const resp = await fetch(url, {
    method: opts.method || "GET",
    headers: opts.body ? { "Content-Type": "application/json" } : undefined,
    body: opts.body ? JSON.stringify(opts.body) : undefined,
  });
  if (!resp.ok) throw new Error(`${path}: ${resp.status} ${await resp.text()}`);
  return resp.json();
}

/**
 * Ensure a provider is registered and configured to accept primary agreements.
 *
 * Re-used by the s3 and drive examples so they can be run standalone (without
 * having to run full-flow.js first to register the provider).
 */
export async function ensureProviderRegistered(api, provider, providerUrl, {
  pricePerByte = 1n,
  maxDuration = 100_000,
} = {}) {
  // Import lazily to break the common.js <-> api.js circular import. Both
  // modules finish initialization before this function ever runs.
  const { registerProvider, updateProviderSettings } = await import("./api.js");
  const existing = await api.query.StorageProvider.Providers.getValue(
    provider.address,
    READ_OPTS
  );
  if (!existing) {
    console.log("  Registering provider", provider.address);
    await registerProvider(api, provider, providerUrl);
  } else {
    // Provider already registered (likely by an earlier demo on the same
    // chain). Sanity-check that the on-chain public_key matches the keyring's
    // — if it doesn't, off-chain signatures produced by the provider node
    // will fail to verify and the failure surfaces as a missing event much
    // later. Better to fail loudly here.
    const onChainKey = existing.public_key?.asBytes
      ? existing.public_key.asBytes()
      : existing.public_key;
    if (!bytesEq(onChainKey, provider.publicKey)) {
      throw new Error(
        `Provider ${provider.address} is already registered with a different public_key. ` +
          `Restart the chain, or run this script with a fresh provider seed.`
      );
    }
  }
  // Always (re)apply settings so price/acceptance are correct for this demo.
  await updateProviderSettings(api, provider, {
    min_duration: 10,
    max_duration: maxDuration,
    price_per_byte: pricePerByte,
    accepting_primary: true,
    replica_sync_price: undefined,
    accepting_extensions: true,
    max_capacity: 0n,
  });
}

function bytesEq(a, b) {
  if (!a || !b || a.length !== b.length) return false;
  for (let i = 0; i < a.length; i++) if (a[i] !== b[i]) return false;
  return true;
}

export function fmtRole(role) {
  if (!role) return "?";
  if (typeof role === "string") return role;
  return role.type ?? JSON.stringify(role);
}

export async function printBucketMembers(api, bucketId, label = "members") {
  const bucket = await api.query.StorageProvider.Buckets.getValue(
    bucketId,
    READ_OPTS
  );
  console.log(`  [${label}] bucket ${bucketId}:`);
  for (const m of bucket.members) {
    console.log(`    - ${m.account}  role=${fmtRole(m.role)}`);
  }
  return bucket.members;
}

/**
 * Compare two SS58 addresses by their underlying public key.
 *
 * PAPI encodes AccountIds with the runtime's SS58 prefix (Polkadot-style
 * `1…` strings on this parachain), while hdkd's `ss58Address` defaults to
 * the substrate-default prefix 42 (`5…` strings). Same key, different
 * string, so string equality fails.
 */
export function sameAddress(a, b) {
  try {
    const [aBytes] = ss58Decode(a);
    const [bBytes] = ss58Decode(b);
    if (aBytes.length !== bBytes.length) return false;
    for (let i = 0; i < aBytes.length; i++) {
      if (aBytes[i] !== bBytes[i]) return false;
    }
    return true;
  } catch {
    return false;
  }
}

/**
 * Dev seeds we know how to sign for. Used by `ensureSoleAcceptingProvider`
 * to silence any non-target provider that may have been registered by an
 * earlier demo on the same chain (e.g. CI starts a second provider node
 * keyed to //Charlie on a different port).
 */
const KNOWN_DEV_SEEDS = [
  "//Alice",
  "//Bob",
  "//Charlie",
  "//Dave",
  "//Eve",
  "//Ferdie",
];

/**
 * Make `keep` the only `accepting_primary` provider among the dev keys.
 *
 * Agreements are now opened against a provider the caller chose explicitly
 * (it negotiates signed terms with one specific node), so selection is no
 * longer ambiguous. But demos still query/score providers and assume a
 * specific node signed the checkpoint; a stray second accepting provider
 * left over from an earlier workflow can make those reads non-deterministic.
 * Silencing the others keeps the chain state predictable across the suite.
 *
 * This helper iterates the known dev seeds, finds any provider that is
 * currently registered and `accepting_primary`, and (if it isn't the keep
 * target) flips `accepting_primary` to false. Returns an async `restore`
 * function that puts each toggled provider back to its original settings.
 *
 * If an unknown (non-dev-key) provider is accepting, this throws — we
 * can't sign for it, so determinism can't be guaranteed and the caller
 * should learn about that explicitly rather than flake later.
 */
export async function ensureSoleAcceptingProvider(api, keep) {
  const toggled = [];
  const others = await api.query.StorageProvider.Providers.getEntries(READ_OPTS);
  for (const { keyArgs, value: info } of others) {
    const account = keyArgs[0];
    if (sameAddress(account, keep.address)) continue;
    if (!info.settings.accepting_primary) continue;
    const seed = KNOWN_DEV_SEEDS.find((s) =>
      sameAddress(makeSigner(s).address, account)
    );
    if (!seed) {
      throw new Error(
        `Provider ${account} is registered with accepting_primary=true but ` +
          `is not a known dev key — cannot silence it to make auto-matching ` +
          `deterministic. Add its seed to KNOWN_DEV_SEEDS or stop the demo.`
      );
    }
    const signer = makeSigner(seed);
    const original = info.settings;
    await submitTx(
      api.tx.StorageProvider.update_provider_settings({
        settings: { ...original, accepting_primary: false },
      }),
      signer.signer,
      `disable accepting_primary for ${seed}`
    );
    toggled.push({ signer, original, seed });
  }
  return async function restore() {
    for (const { signer, original, seed } of toggled) {
      await submitTx(
        api.tx.StorageProvider.update_provider_settings({ settings: original }),
        signer.signer,
        `restore accepting_primary for ${seed}`
      );
    }
  };
}

export function requireOneEvent(events, eventDescriptor, name) {
  const matched = eventDescriptor.filter(events);
  if (matched.length !== 1) {
    throw new Error(
      `Expected exactly 1 ${name} event, got ${matched.length}`
    );
  }
  // PAPI 2.x wraps each event as { original, payload }; callers want the payload.
  return matched[0].payload;
}
