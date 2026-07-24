// SPDX-License-Identifier: Apache-2.0

/**
 * Chain-state waits built on PAPI's `watchValue` instead of polling.
 *
 * `watchValue` replays the current value on subscribe and then emits on every
 * change at the chosen block view — so there is no missed-event window (the
 * race that `api.event.X.watch()` has with in-block submission) and no poll
 * interval to tune. All waits observe the *best* block, consistent with the
 * suite's in-block tx submission (see tx.ts).
 */

import type { PolkadotClient } from "polkadot-api";
import { filter, firstValueFrom, map, switchMap, timeout, TimeoutError } from "rxjs";
import type { Observable } from "rxjs";

import type { ParachainApi } from "./address.js";
import { READ_OPTS } from "./tx.js";

export interface WaitOpts {
  /** `null` disables the timeout (the wait is then bounded by the caller). */
  timeoutMs?: number | null;
  /** What is being waited for — used in the timeout error message. */
  description?: string;
  /**
   * Called every `tickMs` while waiting, with elapsed milliseconds. Lets UIs
   * drive progress text without coupling message strings to the sdk.
   */
  onTick?: (elapsedMs: number) => void;
  tickMs?: number;
}

/**
 * Resolve with the first emission of `source` that satisfies `predicate`;
 * reject with a descriptive error after `timeoutMs`.
 */
export async function firstMatch<T>(
  source: Observable<T>,
  predicate: (value: T) => boolean,
  { timeoutMs = 60_000, description = "condition", onTick, tickMs = 1_000 }: WaitOpts = {},
): Promise<T> {
  const started = Date.now();
  const ticker = onTick
    ? setInterval(() => onTick(Date.now() - started), tickMs)
    : undefined;
  const matches = source.pipe(filter(predicate));
  try {
    return await firstValueFrom(
      timeoutMs == null ? matches : matches.pipe(timeout({ first: timeoutMs })),
    );
  } catch (err) {
    if (err instanceof TimeoutError) {
      throw new Error(`Timed out after ${timeoutMs}ms waiting for ${description}`);
    }
    throw err;
  } finally {
    if (ticker) clearInterval(ticker);
  }
}

type BucketValue = NonNullable<
  Awaited<ReturnType<ParachainApi["query"]["StorageProvider"]["Buckets"]["getValue"]>>
>;

/**
 * Wait until `bucketId` has at least one primary provider (a provider
 * accepted the agreement the bucket was created with) and return the bucket.
 */
export async function waitForPrimaryProvider(
  api: ParachainApi,
  bucketId: bigint,
  { timeoutMs = 150_000, onTick }: WaitOpts = {},
): Promise<BucketValue> {
  const hit = await firstMatch(
    api.query.StorageProvider.Buckets.watchValue(bucketId, READ_OPTS),
    ({ value }) => !!value && value.primary_providers.length > 0,
    {
      timeoutMs,
      onTick,
      description: `bucket ${bucketId} to gain a primary provider`,
    },
  );
  return hit.value as BucketValue;
}

/**
 * Wait until the chain advances to a new best block.
 *
 * Run this once at the top of a script before submitting any extrinsic when
 * the provider node's background coordinators sign with the same dev key the
 * script uses: aligning the first submit to a fresh block boundary shrinks
 * the `(account, nonce)` collision window ("Usurped" drops).
 */
export async function waitForNextBlock(papi: PolkadotClient): Promise<void> {
  const current = await firstValueFrom(
    papi.bestBlocks$.pipe(map((blocks) => blocks[blocks.length - 1].number)),
  );
  await firstMatch(
    papi.bestBlocks$.pipe(map((blocks) => blocks[blocks.length - 1].number)),
    (n) => n > current,
    { timeoutMs: 120_000, description: "the next best block" },
  );
}

/**
 * Wait until the chain's best head is strictly greater than `target`.
 *
 * Callers that need to land an extrinsic at a specific block window use this
 * to time their submission so the runtime sees the block range they computed
 * against.
 */
export async function waitForBlock(
  papi: PolkadotClient,
  target: number,
  { logEvery = 5, timeoutMs }: { logEvery?: number; timeoutMs?: number } = {},
): Promise<void> {
  await firstMatch(
    papi.bestBlocks$.pipe(
      map((blocks) => blocks[blocks.length - 1].number),
      map((n) => {
        if (logEvery > 0 && n % logEvery === 0) {
          console.log("    head=#%d (target > %d)", n, target);
        }
        return n;
      }),
    ),
    (n) => n > target,
    // No default timeout: waiting out a long block window is the point here.
    { timeoutMs: timeoutMs ?? null, description: `best block > ${target}` },
  );
}

/**
 * Current relay-chain block number, read from
 * `ParachainSystem.LastRelayChainBlockNumber` at the best block.
 *
 * Every on-chain duration (timeouts, checkpoint windows, `valid_until`,
 * `CommitmentPayload.nonce` recency) is measured in relay-chain blocks, not
 * parachain blocks — snapshot this, never `System.Number`, when building a
 * nonce or window-timed extrinsic.
 */
export async function currentRelayBlock(api: ParachainApi): Promise<number> {
  return Number(
    await api.query.ParachainSystem.LastRelayChainBlockNumber.getValue(READ_OPTS),
  );
}

/**
 * Wait until the relay-chain block anchored to the parachain's best head is
 * strictly greater than `target`.
 *
 * The relay-block counterpart of [`waitForBlock`]: checkpoint windows and
 * timeouts elapse on the relay clock, so window-timed extrinsics must wait on
 * this one.
 */
export async function waitForRelayBlock(
  papi: PolkadotClient,
  api: ParachainApi,
  target: number,
  { logEvery = 5, timeoutMs }: { logEvery?: number; timeoutMs?: number } = {},
): Promise<void> {
  await firstMatch(
    papi.bestBlocks$.pipe(
      switchMap(() => currentRelayBlock(api)),
      map((n) => {
        if (logEvery > 0 && n % logEvery === 0) {
          console.log("    relay=#%d (target > %d)", n, target);
        }
        return n;
      }),
    ),
    (n) => n > target,
    // No default timeout: waiting out a long block window is the point here.
    { timeoutMs: timeoutMs ?? null, description: `relay block > ${target}` },
  );
}
