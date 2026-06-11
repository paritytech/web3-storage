/**
 * Worker-scoped PAPI client singleton plus thin submit/wait wrappers over
 * @web3-storage/sdk. The singleton is test-infra concern (one socket per
 * Playwright worker, keyed by WS URL); everything chain-semantic — submission
 * modes, dispatch-error handling, watch-based waits — lives in the sdk.
 */

import { createClient, type PolkadotClient } from "polkadot-api";
import { getWsProvider } from "polkadot-api/ws";
import { parachain } from "@polkadot-api/descriptors";
import type { PolkadotSigner } from "polkadot-api/signer";
import {
  firstMatch,
  submitTx,
  type ParachainApi,
  type SubmittableTx,
} from "@web3-storage/sdk";

export type { ParachainApi };

const DEFAULT_WS = process.env.CHAIN_WS ?? "ws://127.0.0.1:2222";

let cachedClient: PolkadotClient | null = null;
let cachedApi: ParachainApi | null = null;
let cachedWs: string | null = null;

export function getClient(wsUrl: string = DEFAULT_WS): PolkadotClient {
  if (cachedClient && cachedWs === wsUrl) return cachedClient;
  if (cachedClient) cachedClient.destroy();
  cachedClient = createClient(getWsProvider(wsUrl));
  cachedApi = cachedClient.getTypedApi(parachain);
  cachedWs = wsUrl;
  return cachedClient;
}

export function getApi(wsUrl: string = DEFAULT_WS): ParachainApi {
  getClient(wsUrl);
  return cachedApi!;
}

export function disconnect(): void {
  if (cachedClient) {
    try {
      cachedClient.destroy();
    } catch {
      // already destroyed
    }
  }
  cachedClient = null;
  cachedApi = null;
  cachedWs = null;
}

/**
 * Sign + submit an extrinsic, wait for FINALIZATION, throw on dispatch error.
 * Test setup paths keep finalized semantics: globalSetup writes state that
 * whole suites depend on, and a reorged-out registration would fail every
 * downstream test with non-local errors.
 */
export async function submitExtrinsic(tx: SubmittableTx, signer: PolkadotSigner) {
  return submitTx(tx, signer, { mode: "finalized", retryStale: 1, onStatus: null });
}

/**
 * Sign + submit, resolve at best-block inclusion (~6s vs ~24s finalized).
 * For cleanup paths where the caller doesn't need finality guarantees and
 * just wants the chain to acknowledge the tx.
 */
export async function submitExtrinsicBestBlock(
  tx: SubmittableTx,
  signer: PolkadotSigner,
): Promise<void> {
  await submitTx(tx, signer, { mode: "best", retryStale: 1, onStatus: null });
}

/**
 * Wait until the chain has FINALIZED a block at or after `target`.
 * Useful when a test needs to wait out a checkpoint window etc.
 */
export async function waitForBlock(target: number, timeoutMs = 120_000): Promise<void> {
  await firstMatch(getClient().finalizedBlock$, (b) => b.number >= target, {
    timeoutMs,
    description: `finalized block >= ${target}`,
  });
}

export async function getBlockNumber(): Promise<number> {
  const block = await getClient().getFinalizedBlock();
  return block.number;
}

/**
 * Latest *best* block number (head of the best chain). Unlike the finalized
 * head, this advances as soon as a block is authored and is immune to
 * finality lag, so it's the right signal for "is the chain alive" liveness
 * checks that must not flake on a healthy-but-not-yet-finalizing chain.
 */
export async function getBestBlockNumber(): Promise<number> {
  const blocks = await firstMatch(getClient().bestBlocks$, (bs) => bs.length > 0, {
    timeoutMs: 30_000,
    description: "a best block",
  });
  return blocks[0].number;
}
