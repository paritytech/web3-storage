/**
 * Transaction submission with explicit "doneness" control.
 *
 * Canonical semantics (established by the finalization fixes in the PAPI E2E
 * suite — see d3de2d9, 391f8bf, 2bd19cf, 18e1416):
 *
 * - Default mode is `"best"` (in-block inclusion), never finalization: ~6x
 *   faster per tx and read-your-writes works as long as reads target the best
 *   block (see {@link READ_OPTS}).
 * - `"finalized"` is opt-in, for txs whose effect a LATER tx references by an
 *   id that embeds the creation block (e.g. challenge ids): a best-block
 *   reorg would roll the id out from under the follow-up.
 * - Events are extracted from the tx result (see {@link requireOneEvent}),
 *   never from `api.event.X.watch()` — typed event watches observe only
 *   finalized blocks and race in-block submission.
 */

import type { PolkadotClient, TxEvent } from "polkadot-api";
import type { PolkadotSigner } from "polkadot-api/signer";
import type { Observable } from "rxjs";

/**
 * Default per-transaction timeout. 180s ≈ 30 blocks at 6s — enough headroom
 * for a tx to be included even under CI load, but bounded so a stuck mempool
 * surfaces as a clear error instead of an indefinite hang that later looks
 * like a missing event downstream.
 */
export const DEFAULT_TX_TIMEOUT_MS = 180_000;

/**
 * Options for storage reads (`getValue` / `getEntries` / `watchValue`).
 * `submitTx` returns at in-block inclusion, so reads must target the best
 * block to see a just-written value (the default finalized head lags).
 * Single switch: flip to "finalized" here to trade read-your-writes for
 * reorg-safety everywhere at once.
 */
export const READ_OPTS = { at: "best" } as const;

/** Transaction "doneness" thresholds, loosest to strictest. */
export type TxMode = "in-pool" | "best" | "finalized";

/** @deprecated legacy aliases for {@link TxMode} values */
export const TX_MODE_IN_POOL: TxMode = "in-pool";
export const TX_MODE_IN_BLOCK: TxMode = "best";
export const TX_MODE_FINALIZED_BLOCK: TxMode = "finalized";

/** Minimal structural view of a PAPI transaction (kept loose on purpose). */
export interface SubmittableTx {
  signSubmitAndWatch(
    signer: PolkadotSigner,
    txOpts?: Record<string, unknown>,
  ): Observable<TxEvent>;
  getBareTx?(): Promise<unknown>;
}

export interface TxStatusUpdate {
  phase: "signed" | "in-pool" | "best" | "finalized" | "retry-stale";
  label: string;
  /** True for the event the chosen mode resolves at. */
  final?: boolean;
  txHash?: string;
  blockHash?: string;
  attempt?: number;
  maxAttempts?: number;
}

export type TxStatusListener = (update: TxStatusUpdate) => void;

/**
 * Default status listener: the log lines the demos/E2E suites have always
 * printed. Pass `onStatus: null` for silence (UIs), or your own listener.
 */
const consoleStatusListener: TxStatusListener = (u) => {
  if (u.phase === "retry-stale") {
    console.warn(
      `  ⚠ ${u.label}: stale nonce (attempt ${u.attempt}/${u.maxAttempts}), ` +
        `waiting for next block…`,
    );
    return;
  }
  if (!u.final) return; // intermediate progress is for UI listeners
  switch (u.phase) {
    case "in-pool":
      console.log(`📦 ${u.label} broadcasted txHash=${u.txHash}`);
      break;
    case "best":
      console.log(`📦 ${u.label} included in block ${u.blockHash}`);
      break;
    case "finalized":
      console.log(`📦 ${u.label} finalized in block ${u.blockHash}`);
      break;
  }
};

/** Dispatch failure surfaced as a typed error so callers can match on it. */
export class TxDispatchError extends Error {
  readonly dispatchError: unknown;
  constructor(label: string, dispatchError: unknown) {
    super(`${label} dispatch failed: ${formatDispatchError(dispatchError)}`);
    this.name = "TxDispatchError";
    this.dispatchError = dispatchError;
  }
}

export function formatDispatchError(err: unknown): string {
  if (!err || typeof err !== "object") return String(err);
  const e = err as { type?: string; value?: { type?: string; value?: { type?: string } } };
  let s = e.type ?? "DispatchError";
  if (e.value?.type) s += `::${e.value.type}`;
  if (e.value?.value?.type) s += `::${e.value.value.type}`;
  return s;
}

export interface SubmitOpts {
  /** Doneness threshold to resolve at. Default: `"best"` (in-block). */
  mode?: TxMode;
  timeoutMs?: number;
  /** Human-readable tag used in logs and error messages. */
  label?: string;
  /** Explicit nonce (NonceManager interop); passed through to PAPI. */
  nonce?: bigint;
  /**
   * Retries on `Invalid::Stale` (nonce too low) — happens when back-to-back
   * CI tests reuse the same signing account and the RPC's nonce view lags the
   * best block. Default 3; UIs should pass 0 (a user retry is the right UX).
   */
  retryStale?: number;
  /** Status listener; `undefined` = console logging, `null` = silent. */
  onStatus?: TxStatusListener | null;
  /** Required for unsigned submission (`signer === null`). */
  client?: PolkadotClient;
  /** Extra options forwarded verbatim to `signSubmitAndWatch`. */
  txOpts?: Record<string, unknown>;
}

const STALE_RETRY_DELAY_MS = 6_500; // ~1 block time

const MODE_MATCH: Record<TxMode, (ev: TxEvent) => boolean> = {
  "in-pool": (ev) => ev.type === "broadcasted",
  best: (ev) => ev.type === "txBestBlocksState" && ev.found,
  finalized: (ev) => ev.type === "finalized",
};

function statusFor(
  phase: TxStatusUpdate["phase"],
  label: string,
  ev: TxEvent,
): TxStatusUpdate {
  const anyEv = ev as TxEvent & { txHash?: string; block?: { hash?: string } };
  return {
    phase,
    label,
    txHash: anyEv.txHash,
    blockHash: anyEv.block?.hash,
  };
}

/** Map a PAPI tx event to a status phase (undefined = not user-relevant). */
function PHASE_FOR_EVENT(ev: TxEvent): TxStatusUpdate["phase"] | undefined {
  switch (ev.type) {
    case "signed":
      return "signed";
    case "broadcasted":
      return "in-pool";
    case "txBestBlocksState":
      return ev.found ? "best" : undefined;
    case "finalized":
      return "finalized";
    default:
      return undefined;
  }
}

/** Single submission attempt: subscribe, resolve at `mode`, bounded by timeout. */
function submitOnce(
  tx: SubmittableTx,
  signer: PolkadotSigner | null,
  mode: TxMode,
  label: string,
  timeoutMs: number,
  onStatus: TxStatusListener | null,
  client?: PolkadotClient,
  txOpts: Record<string, unknown> = {},
): Promise<TxEvent> {
  const observablePromise: Promise<Observable<TxEvent>> = (async () => {
    if (signer === null) {
      if (Object.keys(txOpts).length > 0) {
        throw new Error(
          `${label}: txOpts not supported for unsigned transactions ` +
            `(getBareTx accepts no options). See polkadot-api/polkadot-api#760.`,
        );
      }
      if (!client) {
        throw new Error(
          `${label}: unsigned submission requires opts.client (the PAPI client).`,
        );
      }
      if (!tx.getBareTx) throw new Error(`${label}: tx has no getBareTx()`);
      const bareTx = await tx.getBareTx();
      return client.submitAndWatch(bareTx as never) as Observable<TxEvent>;
    }
    return tx.signSubmitAndWatch(signer, txOpts);
  })();

  return new Promise((resolve, reject) => {
    let resolved = false;
    let sub: { unsubscribe(): void } | undefined;
    const cleanup = () => {
      resolved = true;
      clearTimeout(timer);
      sub?.unsubscribe();
    };
    const timer = setTimeout(() => {
      if (!resolved) {
        cleanup();
        reject(
          new Error(`${label}: timed out after ${timeoutMs}ms waiting for ${mode}`),
        );
      }
    }, timeoutMs);

    observablePromise.then(
      (observable) => {
        if (resolved) return;
        sub = observable.subscribe({
          next: (ev) => {
            if (resolved) return;
            // PAPI surfaces dispatch errors on the in-block / finalized events
            // as `ev.ok === false` (it only throws on an *invalid* tx). Catch
            // them here so callers don't have to.
            if (
              (ev.type === "txBestBlocksState" && ev.found && ev.ok === false) ||
              (ev.type === "finalized" && ev.ok === false)
            ) {
              cleanup();
              reject(new TxDispatchError(label, ev.dispatchError));
              return;
            }
            const final = MODE_MATCH[mode](ev);
            // Stream every lifecycle event so UI listeners can show progress;
            // the default console listener only prints the final one.
            const phase = PHASE_FOR_EVENT(ev);
            if (phase) onStatus?.({ ...statusFor(phase, label, ev), final });
            if (final) {
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
      },
      (err) => {
        if (resolved) return;
        cleanup();
        reject(err);
      },
    );
  });
}

/**
 * Sign + submit a tx, assert it dispatched OK, return the event the chosen
 * mode resolved at (`{ ok, events, … }` for "best"/"finalized").
 *
 * Third argument: either a label string (the common case in demos/E2E) or a
 * {@link SubmitOpts} object. A fourth positional `timeoutMs` is honored for
 * backwards compatibility with the label form.
 */
export async function submitTx(
  tx: SubmittableTx,
  signer: PolkadotSigner | null,
  optsOrLabel: SubmitOpts | string = {},
  positionalTimeoutMs?: number,
): Promise<TxEvent & { events: unknown[] }> {
  const opts: SubmitOpts =
    typeof optsOrLabel === "string"
      ? { label: optsOrLabel, timeoutMs: positionalTimeoutMs }
      : optsOrLabel;
  const {
    mode = "best",
    timeoutMs = DEFAULT_TX_TIMEOUT_MS,
    label = "tx",
    retryStale = 3,
    client,
    txOpts = {},
  } = opts;
  const onStatus = opts.onStatus === undefined ? consoleStatusListener : opts.onStatus;
  const fullTxOpts =
    opts.nonce !== undefined ? { ...txOpts, nonce: opts.nonce } : txOpts;

  const attempts = Math.max(1, retryStale);
  for (let attempt = 1; ; attempt++) {
    try {
      return (await submitOnce(
        tx,
        signer,
        mode,
        label,
        timeoutMs,
        onStatus,
        client,
        fullTxOpts,
      )) as TxEvent & { events: unknown[] };
    } catch (err) {
      const isStale = /Invalid.*Stale|Stale.*nonce/i.test((err as Error).message ?? "");
      if (!isStale || attempt >= attempts) throw err;
      onStatus?.({ phase: "retry-stale", label, attempt, maxAttempts: attempts });
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
  tx: SubmittableTx,
  signer: PolkadotSigner | null,
  optsOrLabel: SubmitOpts | string = {},
  positionalTimeoutMs?: number,
): Promise<TxEvent & { events: unknown[] }> {
  const opts: SubmitOpts =
    typeof optsOrLabel === "string"
      ? { label: optsOrLabel, timeoutMs: positionalTimeoutMs }
      : optsOrLabel;
  return submitTx(tx, signer, { ...opts, mode: "finalized" });
}

/**
 * Compatibility shim for the historical positional signature. New code should
 * call {@link submitTx} with {@link SubmitOpts}.
 */
export async function waitForTransaction(
  tx: SubmittableTx,
  signer: PolkadotSigner | null,
  label: string,
  txMode: TxMode = "best",
  timeoutMs: number = DEFAULT_TX_TIMEOUT_MS,
  client: PolkadotClient | null = null,
  txOpts: Record<string, unknown> = {},
): Promise<TxEvent> {
  return submitTx(tx, signer, {
    mode: txMode,
    label,
    timeoutMs,
    retryStale: 1,
    client: client ?? undefined,
    txOpts,
  });
}

/**
 * Sequence multiple transactions from the same signer without racing the
 * RPC's nonce cache. Construct once after reading the on-chain nonce, then
 * pass `{ nonce: nm.next() }` when submitting back-to-back txs.
 */
export class NonceManager {
  private nonce: bigint;
  constructor(initialNonce: bigint | number) {
    this.nonce = BigInt(initialNonce);
  }
  /** Return the current nonce, then advance. */
  next(): bigint {
    const current = this.nonce;
    this.nonce += 1n;
    return current;
  }
  /** Read the on-chain nonce for `address` and build a manager from it. */
  static async forAccount(
    api: { query: { System: { Account: { getValue(address: string, opts?: unknown): Promise<{ nonce: number }> } } } },
    address: string,
  ): Promise<NonceManager> {
    // Best block: a finalized read lags and returns a stale (too-low) nonce.
    const account = await api.query.System.Account.getValue(address, READ_OPTS);
    return new NonceManager(account.nonce);
  }
}

/**
 * Event descriptor shape exposed by the typed API (`api.event.X.Y`); its
 * `filter` returns `PalletEvent<T>` records, payload under `.payload`.
 */
export interface EventDescriptor<T> {
  filter(events: never): Array<{ payload: T }>;
}

/** Extract the payload of exactly one `name` event from a tx result's events. */
export function requireOneEvent<T>(
  events: unknown[],
  eventDescriptor: EventDescriptor<T>,
  name: string,
): T {
  const matched = eventDescriptor.filter(events as never);
  if (matched.length !== 1) {
    throw new Error(`Expected exactly 1 ${name} event, got ${matched.length}`);
  }
  return matched[0].payload;
}
