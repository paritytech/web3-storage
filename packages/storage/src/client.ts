// SPDX-License-Identifier: Apache-2.0

/**
 * Chain connection plus the pre-subscription readiness guards.
 *
 * `waitForChainReady` / `waitForBlockProduction` stay polling-based on
 * purpose: they cover the zombienet-warmup window where the chain produces no
 * blocks (or no metadata) yet, so a subscription-based wait would simply hang
 * with no events to react to. Everything downstream of "the chain is alive"
 * uses subscriptions/watchValue instead — see waits.ts.
 */

import { createClient, type PolkadotClient } from "polkadot-api";
import { getWsProvider } from "polkadot-api/ws";
import { parachain } from "@polkadot-api/descriptors";

import { setSs58Prefix, type ParachainApi } from "./address.js";
import { READ_OPTS } from "./tx.js";

export interface ChainConnection {
  /** The raw PAPI client (bestBlocks$, finalizedBlock$, destroy, …). */
  papi: PolkadotClient;
  /** The typed parachain API. */
  api: ParachainApi;
}

export function connect(chainWs: string): ChainConnection {
  const papi = createClient(getWsProvider(chainWs));
  const api = papi.getTypedApi(parachain);
  return { papi, api };
}

export function disconnect({ papi }: ChainConnection): void {
  papi.destroy();
}

/**
 * Adopt the connected runtime's SS58 prefix for address rendering. Affects
 * signers created *after* the call (addresses are computed at construction).
 */
export async function syncSs58Prefix(api: ParachainApi): Promise<number> {
  const prefix = await api.constants.System.SS58Prefix();
  setSs58Prefix(prefix);
  return prefix;
}

/**
 * Verify the chain's runtime is reachable by reading a constant. Catches the
 * "RPC handshake completed but the runtime metadata isn't loaded yet" window
 * that occurs right after zombienet startup, before any submit would fail
 * cryptically downstream.
 */
export async function waitForChainReady(
  api: ParachainApi,
  { maxRetries = 10, retryDelayMs = 2_000 } = {},
): Promise<boolean> {
  for (let attempt = 1; attempt <= maxRetries; attempt++) {
    try {
      const spec = await api.constants.System.Version();
      console.log(`✅ Chain ready: ${spec.spec_name} v${spec.spec_version}`);
      return true;
    } catch (error) {
      if (attempt === maxRetries) {
        throw new Error(
          `Chain not ready after ${maxRetries} attempts: ${(error as Error)?.message ?? error}`,
        );
      }
      console.log(
        `⏳ Chain not ready (attempt ${attempt}/${maxRetries}): ${(error as Error)?.message ?? error}`,
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
export async function waitForBlockProduction(
  api: ParachainApi,
  { timeoutSec = 300 } = {},
): Promise<number> {
  const pollIntervalMs = 2_000;
  const deadline = Date.now() + timeoutSec * 1_000;
  while (Date.now() < deadline) {
    try {
      const n = await api.query.System.Number.getValue(READ_OPTS);
      const blockNumber = Number(n);
      if (blockNumber > 0) {
        console.log(`✅ Chain producing blocks (head=#${blockNumber})`);
        return blockNumber;
      }
      console.log(`⏳ Block number is ${blockNumber}, waiting...`);
    } catch (error) {
      console.log(
        `⏳ Cannot query block number yet: ${(error as Error)?.message ?? error}`,
      );
    }
    await new Promise((r) => setTimeout(r, pollIntervalMs));
  }
  throw new Error(`Chain did not produce any blocks within ${timeoutSec}s`);
}
